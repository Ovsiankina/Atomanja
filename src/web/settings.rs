//! The settings screen: accounts and invite link (APP-26), amount
//! proposal (APP-16), language (APP-39), notifications (APP-33).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use maud::{html, Markup};

use super::handlers::{page, require};
use super::i18n::{msgs, Msgs};
use super::pages::{self, Field, Variant};
use super::session::{internal, lock};
use super::AppState;
use crate::auth::{self, Account, Action, Database, Role};
use crate::ledger::{Ledger, WitnessId};
use crate::store::{Kind, Store, StoreError};

/// A freshly created invite link and what happened to its email.
pub(super) struct InviteOutcome {
    pub link: String,
    /// `(address, sent)` when the worker entered an address.
    pub mailed: Option<(String, bool)>,
}

/// `GET /settings`.
pub(super) async fn settings(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let actor = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    render(&state, &actor.account, None, None)
}

/// The full settings page for `account`, from its role's file (DEV-3).
/// `amount_error` explains a refused amount action (plain form posts).
pub(super) fn render(
    state: &AppState,
    account: &Account,
    invite: Option<&InviteOutcome>,
    amount_error: Option<&str>,
) -> Response {
    let m = msgs(account.lang);
    let body = match account.role.database() {
        Database::Ledger => body(&*lock(&state.ledger), account, invite, amount_error),
        Database::Dev => body(&*lock(&state.dev), account, invite, amount_error),
    };
    match body {
        Ok(b) => page(m, m.nav_settings, "settings", &b),
        Err(e) => internal(&e),
    }
}

/// The settings sections over `store`.
fn body<K: Kind>(
    store: &Store<K>,
    account: &Account,
    invite: Option<&InviteOutcome>,
    amount_error: Option<&str>,
) -> Result<Markup, StoreError> {
    let m = msgs(account.lang);
    let accounts = store.accounts()?;
    let ledger = store.load_ledger()?;
    Ok(html! {
        div.page__stack data-view="settings" {
            @if account.role == Role::Admin {
                div.page__pinned { (pages::banner(true, m.fake_label, m.fake_text)) }
            }
            (pages::section(1, m.amount_per_task, &amount_block(m, &ledger, &accounts, account, amount_error)))
            (pages::section(2, m.accounts, &account_panel(m, &accounts, account, invite)))
        }
    })
}

/// The `AmountProposal`: the amount in force, the open proposal with one
/// vote row per real witness, the worker's form and the witness's Agree
/// button (APP-15, APP-16). Its `id` is the htmx target of both actions.
///
/// DECISION: no Decline button. The ledger records approvals only; a
/// proposal stays open until every real witness agrees or the worker
/// proposes another amount, so "decline" would set nothing.
pub(super) fn amount_block(
    m: &Msgs,
    ledger: &Ledger,
    accounts: &[Account],
    me: &Account,
    error: Option<&str>,
) -> Markup {
    let current = m.number(ledger.amount());
    let witnesses: Vec<&Account> = accounts.iter().filter(|a| a.role.is_real_witness()).collect();
    let proposal = ledger.proposal();
    let agreed = |a: &Account| proposal.is_some_and(|p| p.approvals.contains(&WitnessId(a.id.0)));
    html! {
        section.at-proposal id="amount" {
            @if let Some(p) = proposal {
                p.at-label { (m.proposed_change) }
                p.at-proposal__n {
                    (current)
                    span.at-proposal__to aria-label="→" { "→" }
                    (m.number(p.amount))
                    span.at-unit { "CHF" }
                }
                p.at-muted { (m.agreed_count(witnesses.iter().filter(|a| agreed(a)).count(), witnesses.len())) }
                ul.at-people {
                    @for w in &witnesses {
                        li.at-person {
                            span.at-person__name { (w.email) }
                            @if agreed(w) { (pages::tag("received", m.vote_agreed)) }
                            @else { (pages::tag("upcoming", m.vote_pending)) }
                        }
                    }
                }
                @if auth::permits(me, Action::ApproveAmount) && !agreed(me) {
                    form.at-proposal__act method="post" action="/amount/approve"
                        hx-post="/amount/approve" hx-target="#amount" hx-swap="outerHTML" {
                        (pages::button(m.agree, Variant::Solid, false, false, None))
                    }
                }
            } @else {
                p.at-label { (m.amount_per_task) }
                p.at-proposal__n { (current) span.at-unit { "CHF" } }
            }
            @if let Some(e) = error { p.row__error role="alert" { (e) } }
            @if auth::permits(me, Action::ProposeAmount) {
                form.at-proposal__form method="post" action="/amount/propose"
                    hx-post="/amount/propose" hx-target="#amount" hx-swap="outerHTML" {
                    (Field { hint: Some(m.propose_hint), ..Field::new(m.propose_field, "amount", "number", "off") }.render())
                    (pages::button(m.propose, Variant::Outline, false, false, Some("→")))
                }
            }
        }
    }
}

/// The `AccountPanel`: the one worker, the witnesses, and for the worker
/// the invite block. Its `id` is the htmx target of `POST /invite`.
pub(super) fn account_panel(
    m: &Msgs,
    accounts: &[Account],
    me: &Account,
    invite: Option<&InviteOutcome>,
) -> Markup {
    html! {
        div.at-account id="accounts" {
            header.at-account__head {
                span.at-label { (m.accounts) }
                span.at-mono { (accounts.len()) }
            }
            ul.at-people {
                @for a in accounts {
                    li.at-person {
                        span.at-person__name {
                            (a.email)
                            @if a.id == me.id { span.at-index { " ·" } }
                        }
                        span.at-label { (match a.role {
                            Role::Worker => m.role_worker,
                            Role::Witness => m.role_witness,
                            Role::Admin => m.role_admin,
                        }) }
                        span.at-muted { (match a.role {
                            Role::Worker => m.owes_when_late,
                            Role::Witness => m.active,
                            Role::Admin => m.sees_fake,
                        }) }
                    }
                }
            }
            @if auth::permits(me, Action::CreateInvite) {
                div.at-account__invite {
                    @if let Some(inv) = invite {
                        (Field { readonly: true, required: false, value: &inv.link, hint: Some(m.invite_hint),
                                 ..Field::new(m.invite_link, "link", "text", "off") }.render())
                        @if let Some((to, sent)) = &inv.mailed {
                            @if *sent { p.at-muted role="status" { (m.mail_sent(to)) } }
                            @else { p.row__error role="alert" { (m.mail_failed) } }
                        }
                    }
                    form.page__form method="post" action="/invite" hx-post="/invite" hx-target="#accounts" hx-swap="outerHTML" {
                        (Field { required: false, hint: Some(m.invite_email_hint),
                                 ..Field::new(m.invite_email, "email", "email", "off") }.render())
                        (pages::button(if invite.is_some() { m.invite_another } else { m.invite_witness },
                                       Variant::Outline, false, false, Some("→")))
                    }
                }
            }
        }
    }
}
