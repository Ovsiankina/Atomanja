//! Debt and amount actions: claim, mark sent, mark received, propose,
//! approve. Each is an htmx POST answered with the re-rendered fragment,
//! or a plain form POST answered with a redirect (APP-16..APP-22).
//!
//! Every action is for the worker or a real witness only (`auth::permits`
//! rejects the admin first), so it always runs on the ledger file
//! (DEV-3). The store lock is held for the synchronous part only: rebuild
//! the ledger, check the rule, append, collect the notifications. Pushes
//! go out after the lock is released and never undo the action (SRS 5.1).

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::handlers::{page, require};
use super::i18n::{msgs, Msgs};
use super::session::{internal, is_htmx, lock, see_other, stamp};
use super::settings;
use super::views::{self, Ctx, Viewer};
use super::AppState;
use crate::auth::{self, Account, Action, Role};
use crate::ledger::{Centimes, Change, DebtId, Ledger, RuleError, WitnessId};
use crate::notify::{self, Message, Subscription};
use crate::store::{LedgerDb, Store, StoreError};

/// One notification and the devices it goes to.
type Push = (Message, Vec<Subscription>);

/// What an action left behind: the ledger after it, every account, and
/// the pushes to send or the rule that refused it.
struct Outcome {
    ledger: Ledger,
    accounts: Vec<Account>,
    result: Result<Vec<Push>, RuleError>,
}

/// 403 for an account whose role may not do this (APP-20, APP-21).
fn forbidden(m: &Msgs) -> Response {
    (StatusCode::FORBIDDEN, m.err_forbidden).into_response()
}

/// Persist `changes` (one transaction), apply them, and collect the
/// notifications they cause with each recipient's devices.
fn commit(
    store: &mut Store<LedgerDb>,
    mut ledger: Ledger,
    accounts: Vec<Account>,
    changes: &[Change],
    conflict: RuleError,
) -> Result<Outcome, StoreError> {
    match store.append(changes) {
        Ok(()) => {}
        // A concurrent writer won (Review Focus 3): nothing was written;
        // show the state as it now is.
        Err(StoreError::Conflict) => {
            return Ok(Outcome {
                ledger: store.load_ledger()?,
                accounts,
                result: Err(conflict),
            });
        }
        Err(e) => return Err(e),
    }
    for c in changes {
        ledger.apply(c);
    }
    let mut pushes = Vec::new();
    for message in notify::messages(changes, &ledger, &accounts) {
        let subs = store
            .subscriptions(message.to)?
            .into_iter()
            .map(|s| Subscription {
                endpoint: s.endpoint,
                p256dh: s.p256dh,
                auth: s.auth,
            })
            .collect();
        pushes.push((message, subs));
    }
    Ok(Outcome {
        ledger,
        accounts,
        result: Ok(pushes),
    })
}

/// Send the pushes in the background; gone devices are removed.
fn spawn_pushes(state: &AppState, pushes: Vec<Push>) {
    if pushes.is_empty() {
        return;
    }
    let notifier = state.notifier.clone();
    let store = state.ledger.clone();
    // DECISION: pushes run in a spawned task so a slow push service never
    // delays the answer; failures are counted and logged by `notify`.
    tokio::spawn(async move {
        for (message, subs) in pushes {
            let report = notifier.send(&message, &subs).await;
            if !report.gone.is_empty() {
                let mut store = lock(&store);
                for endpoint in &report.gone {
                    if let Err(e) = store.remove_subscription(endpoint) {
                        eprintln!("web: removing a gone device failed: {e}");
                    }
                }
            }
        }
    });
}

/// The three debt actions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DebtOp {
    Claim,
    Sent,
    Received,
}

/// Rule check, append and notifications for one debt action, under the
/// ledger store lock.
fn debt_change(
    store: &mut Store<LedgerDb>,
    actor: &Account,
    debt: DebtId,
    op: DebtOp,
) -> Result<Outcome, StoreError> {
    let ledger = store.load_ledger()?;
    let accounts = store.accounts()?;
    let me = WitnessId(actor.id.0);
    let now = stamp();
    let change = match op {
        DebtOp::Claim => ledger.claim(debt, me, &store.real_witnesses()?, now),
        DebtOp::Sent => ledger.mark_sent(debt, now),
        DebtOp::Received => ledger.mark_received(debt, me, now),
    };
    let conflict = if op == DebtOp::Claim {
        RuleError::AlreadyClaimed
    } else {
        RuleError::AlreadyMarked
    };
    match change {
        Ok(c) => commit(store, ledger, accounts, &[c], conflict),
        Err(e) => Ok(Outcome {
            ledger,
            accounts,
            result: Err(e),
        }),
    }
}

/// Shared body of the three debt routes.
fn debt_action(state: &AppState, headers: &HeaderMap, debt: u32, op: DebtOp) -> Response {
    let actor = match require(state, headers) {
        Ok(a) => a,
        Err(r) => return *r,
    };
    let m = msgs(actor.account.lang);
    let action = match op {
        DebtOp::Claim => Action::Claim,
        DebtOp::Sent => Action::MarkSent,
        DebtOp::Received => Action::MarkReceived,
    };
    if !auth::permits(&actor.account, action) {
        return forbidden(m);
    }
    let titles = (actor.account.role == Role::Worker).then(|| lock(&state.titles).clone());
    let starts = lock(&state.starts).clone();
    let debt = DebtId(debt);
    let outcome = debt_change(&mut lock(&state.ledger), &actor.account, debt, op);
    let Outcome {
        ledger,
        accounts,
        result,
    } = match outcome {
        Ok(o) => o,
        Err(e) => return internal(&e),
    };
    let error = match result {
        Ok(pushes) => {
            spawn_pushes(state, pushes);
            None
        }
        Err(RuleError::UnknownDebt) => {
            return (StatusCode::NOT_FOUND, m.err_unknown_debt).into_response();
        }
        Err(e) => Some(m.rule(e)),
    };
    let ctx = Ctx {
        m,
        viewer: Viewer::of(&actor.account),
        titles: titles.as_ref(),
        names: Ctx::names_of(&accounts),
        starts: &starts,
        today: stamp(),
    };
    let status = if error.is_some() {
        StatusCode::CONFLICT
    } else {
        StatusCode::OK
    };
    if is_htmx(headers) {
        return (status, views::debt_fragment(&ctx, &ledger, debt, error)).into_response();
    }
    match error {
        None => see_other("/"),
        Some(e) => {
            let occ = ledger.debts().occurrence[debt.0 as usize].0 as usize;
            let body = views::dashboard(&ctx, &ledger, views::View::List, Some((occ, e)));
            (status, page(m, m.nav_tasks, "tasks", &body)).into_response()
        }
    }
}

/// `POST /debts/{id}/claim` (APP-18, APP-19).
pub(super) async fn claim(
    State(state): State<AppState>,
    Path(debt): Path<u32>,
    headers: HeaderMap,
) -> Response {
    debt_action(&state, &headers, debt, DebtOp::Claim)
}

/// `POST /debts/{id}/sent` (APP-21, APP-23).
pub(super) async fn sent(
    State(state): State<AppState>,
    Path(debt): Path<u32>,
    headers: HeaderMap,
) -> Response {
    debt_action(&state, &headers, debt, DebtOp::Sent)
}

/// `POST /debts/{id}/received` (APP-22, APP-23).
pub(super) async fn received(
    State(state): State<AppState>,
    Path(debt): Path<u32>,
    headers: HeaderMap,
) -> Response {
    debt_action(&state, &headers, debt, DebtOp::Received)
}

/// The proposal form field: whole CHF.
#[derive(Deserialize)]
pub(super) struct AmountForm {
    #[serde(default)]
    amount: String,
}

// DECISION: proposals are whole CHF ("15"), converted to centimes; the
// design system's field is a whole number and no rule needs cents.
/// `"15"` as 1500 centimes; `None` for anything else.
fn parse_chf(raw: &str) -> Option<Centimes> {
    raw.trim()
        .parse::<u32>()
        .ok()
        .and_then(|chf| chf.checked_mul(100))
        .map(Centimes)
}

/// The two amount actions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AmountOp {
    Propose(Option<Centimes>),
    Approve,
}

/// Rule check and append for one amount action, under the store lock.
fn amount_change(
    store: &mut Store<LedgerDb>,
    actor: &Account,
    op: AmountOp,
) -> Result<Outcome, StoreError> {
    let ledger = store.load_ledger()?;
    let accounts = store.accounts()?;
    let now = stamp();
    let changes = match op {
        AmountOp::Propose(Some(amount)) => ledger.propose_amount(amount, now).map(|c| vec![c]),
        AmountOp::Propose(None) => Err(RuleError::InvalidAmount),
        AmountOp::Approve => {
            ledger.approve_amount(WitnessId(actor.id.0), &store.real_witnesses()?, now)
        }
    };
    match changes {
        Ok(cs) => commit(store, ledger, accounts, &cs, RuleError::AlreadyApproved),
        Err(e) => Ok(Outcome {
            ledger,
            accounts,
            result: Err(e),
        }),
    }
}

/// Shared body of the two amount routes (APP-16).
fn amount_action(state: &AppState, headers: &HeaderMap, op: AmountOp) -> Response {
    let actor = match require(state, headers) {
        Ok(a) => a,
        Err(r) => return *r,
    };
    let m = msgs(actor.account.lang);
    let action = match op {
        AmountOp::Propose(_) => Action::ProposeAmount,
        AmountOp::Approve => Action::ApproveAmount,
    };
    if !auth::permits(&actor.account, action) {
        return forbidden(m);
    }
    let outcome = amount_change(&mut lock(&state.ledger), &actor.account, op);
    let Outcome {
        ledger,
        accounts,
        result,
    } = match outcome {
        Ok(o) => o,
        Err(e) => return internal(&e),
    };
    // Amount changes notify nobody (APP-37); `result` holds no pushes.
    let error = result.err().map(|e| m.rule(e));
    let status = if error.is_some() {
        StatusCode::CONFLICT
    } else {
        StatusCode::OK
    };
    if is_htmx(headers) {
        let block = settings::amount_block(m, &ledger, &accounts, &actor.account, error);
        return (status, block).into_response();
    }
    match error {
        None => see_other("/settings"),
        Some(e) => {
            let mut r = settings::render(state, &actor.account, None, Some(e));
            *r.status_mut() = status;
            r
        }
    }
}

/// `POST /amount/propose`: the worker proposes a new amount (APP-16).
pub(super) async fn propose(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<AmountForm>,
) -> Response {
    amount_action(&state, &headers, AmountOp::Propose(parse_chf(&f.amount)))
}

/// `POST /amount/approve`: a real witness agrees; the amount comes into
/// force when every real witness has (APP-16).
pub(super) async fn approve(State(state): State<AppState>, headers: HeaderMap) -> Response {
    amount_action(&state, &headers, AmountOp::Approve)
}
