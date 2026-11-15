//! The two dashboards (APP-31 worker, APP-32 witness and admin): rows
//! built from a ledger rebuilt under the store lock, rendered as the
//! design system's `Totals` + `TaskList`.

use std::collections::BTreeMap;

use maud::{html, Markup};

use super::i18n::Msgs;
use super::pages::{self, Status, Variant};
use crate::auth::{Account, Role};
use crate::ledger::{DebtId, EventKind, Ledger, Outcome, TaskId, WitnessId};

/// Who looks at the rows; decides titles and which button a row gets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Viewer {
    /// The worker: titles shown, "Mark sent" (APP-21, APP-31).
    Worker,
    /// A real witness: task numbers only, "Claim" / "Mark received"
    /// (APP-18, APP-22, APP-32).
    Witness(WitnessId),
    /// The admin witness: the witness view of fake data, no actions
    /// (APP-20, APP-30).
    Admin,
}

impl Viewer {
    /// The viewer for a signed-in account.
    pub fn of(account: &Account) -> Viewer {
        match account.role {
            Role::Worker => Viewer::Worker,
            Role::Witness => Viewer::Witness(WitnessId(account.id.0)),
            Role::Admin => Viewer::Admin,
        }
    }
}

/// Everything a row needs besides the ledger.
pub(super) struct Ctx<'a> {
    pub m: &'static Msgs,
    pub viewer: Viewer,
    /// Task titles: `Some` for the worker only. The witness and admin
    /// views are built with `None`, so no title can reach them (APP-32).
    pub titles: Option<&'a BTreeMap<TaskId, String>>,
    /// Account emails by id, to name an owner witness.
    pub names: BTreeMap<i64, String>,
}

impl Ctx<'_> {
    /// Emails by account id.
    pub fn names_of(accounts: &[Account]) -> BTreeMap<i64, String> {
        accounts.iter().map(|a| (a.id.0, a.email.clone())).collect()
    }
}

/// Row of the debt of each occurrence, if it has one (APP-11: at most one).
pub(super) fn debt_by_occurrence(ledger: &Ledger) -> Vec<Option<usize>> {
    let mut out = vec![None; ledger.occurrences().task.len()];
    for (d, occ) in ledger.debts().occurrence.iter().enumerate() {
        if let Some(slot) = out.get_mut(occ.0 as usize) {
            *slot = Some(d);
        }
    }
    out
}

/// Status of occurrence `o` with debt row `d`.
fn status(ledger: &Ledger, o: usize, d: Option<usize>) -> Status {
    let debts = ledger.debts();
    match (ledger.occurrences().outcome[o], d) {
        (Outcome::Pending, _) => Status::Upcoming,
        (Outcome::Met, _) => Status::Met,
        (Outcome::Late, Some(d)) if debts.received_at[d].is_some() => Status::Received,
        (Outcome::Late, Some(d)) if debts.sent_at[d].is_some() => Status::Sent,
        (Outcome::Late, _) => Status::Late,
    }
}

/// Late first, then sent, upcoming, settled; by deadline inside each.
fn rank(s: Status) -> u8 {
    match s {
        Status::Late => 0,
        Status::Sent => 1,
        Status::Upcoming => 2,
        Status::Met | Status::Received => 3,
    }
}

/// The one button a row offers this viewer, as `(url, label)`.
fn action(ctx: &Ctx<'_>, ledger: &Ledger, d: usize) -> Option<(String, &'static str)> {
    let debts = ledger.debts();
    let (owner, sent, received) = (debts.owner[d], debts.sent_at[d], debts.received_at[d]);
    match ctx.viewer {
        Viewer::Worker if owner.is_some() && sent.is_none() => {
            Some((format!("/debts/{d}/sent"), ctx.m.mark_sent))
        }
        Viewer::Witness(_) if owner.is_none() => Some((format!("/debts/{d}/claim"), ctx.m.claim)),
        Viewer::Witness(me) if owner == Some(me) && sent.is_some() && received.is_none() => {
            Some((format!("/debts/{d}/received"), ctx.m.mark_received))
        }
        _ => None,
    }
}

/// One `TaskList` row for occurrence `o`, with an optional rejection
/// message under it (a 409 answer to an action).
pub(super) fn row(ctx: &Ctx<'_>, ledger: &Ledger, o: usize, d: Option<usize>, error: Option<&str>) -> Markup {
    let m = ctx.m;
    let occ = ledger.occurrences();
    let task = occ.task[o];
    let deadline = occ.deadline[o];
    let st = status(ledger, o, d);
    let label = match ctx.titles {
        Some(titles) => titles.get(&task).cloned().unwrap_or_else(|| m.task(task.0)),
        None => m.task(task.0),
    };
    let events = ledger.events();
    html! {
        li class={ "at-row at-row--" (st.class()) } id={ "occ-" (o) } {
            div.at-row__when {
                span.at-mono { (m.date(deadline)) }
                @if ctx.titles.is_some() { span.at-index { "#" (task.0) } }
            }
            div.at-row__main {
                div.at-row__head {
                    span.at-row__title { (label) }
                    (pages::tag(st.class(), st.word(m)))
                }
                @if let Some(d) = d {
                    @let debts = ledger.debts();
                    div.at-row__debt {
                        span.at-mono { (m.amount(debts.amount[d])) }
                        span.at-muted { (owner_text(ctx, debts.owner[d])) }
                        span.at-checks {
                            (pages::check(m, m.st_sent, debts.sent_at[d].is_some()))
                            (pages::check(m, m.st_received, debts.received_at[d].is_some()))
                        }
                    }
                }
                @for e in (0..events.occurrence.len()).filter(|&e| events.occurrence[e].0 as usize == o) {
                    (event_line(m, events.kind[e], events.new_due[e], deadline, events.at[e]))
                }
                @if let Some(err) = error { p.row__error role="alert" { (err) } }
            }
            div.at-row__act {
                @if let Some((url, text)) = d.and_then(|d| action(ctx, ledger, d)) {
                    form method="post" action=(url) hx-post=(url) hx-target="closest li" hx-swap="outerHTML" {
                        (pages::button(text, Variant::Solid, true, false, None))
                    }
                }
            }
        }
    }
}

// DECISION: an owner is named by account email (accounts carry no display
// name); the reader's own claims read "you".
/// "Unclaimed", "Claimed by you", "Claimed by a@b".
fn owner_text(ctx: &Ctx<'_>, owner: Option<WitnessId>) -> String {
    let m = ctx.m;
    match owner {
        None => m.unclaimed.to_owned(),
        Some(w) if ctx.viewer == Viewer::Witness(w) => m.claimed_by(m.you),
        Some(w) => m.claimed_by(ctx.names.get(&w.0).map_or("?", String::as_str)),
    }
}

/// One `EventLine` (APP-6, APP-7): the sentence ends saying the
/// deadline still counts.
fn event_line(
    m: &Msgs,
    kind: EventKind,
    new_due: Option<crate::ledger::Timestamp>,
    recorded: crate::ledger::Timestamp,
    at: crate::ledger::Timestamp,
) -> Markup {
    let (glyph, text) = match (kind, new_due) {
        (EventKind::MovedLater, Some(to)) => ("↦", m.ev_moved(to, recorded)),
        (EventKind::MovedEarlier, Some(to)) => ("↦", m.ev_earlier(to)),
        (EventKind::Removed, _) | (EventKind::MovedLater | EventKind::MovedEarlier, None) => {
            ("∅", m.ev_removed.to_owned())
        }
        (EventKind::Deleted, _) => ("×", m.ev_deleted.to_owned()),
        (EventKind::Restored, _) => ("↦", m.ev_restored.to_owned()),
    };
    html! {
        p.at-event {
            span.at-event__g aria-hidden="true" { (glyph) }
            span { (text) }
            time.at-event__at { (m.date(at)) }
        }
    }
}

/// The dashboard body: banner (admin), totals, task list. `error` puts
/// a rejection message under one occurrence's row (plain form posts).
pub(super) fn dashboard(ctx: &Ctx<'_>, ledger: &Ledger, error: Option<(usize, &str)>) -> Markup {
    let m = ctx.m;
    let by_occ = debt_by_occurrence(ledger);
    let occ = ledger.occurrences();
    let mut order: Vec<usize> = (0..occ.task.len()).collect();
    order.sort_by_key(|&o| (rank(status(ledger, o, by_occ[o])), occ.deadline[o], o));
    let view = if ctx.viewer == Viewer::Worker { "worker" } else { "witness" };
    html! {
        div.page__stack data-view=(view) {
            @if ctx.viewer == Viewer::Admin {
                div.page__pinned { (pages::banner(true, m.fake_label, m.fake_text)) }
            }
            // DECISION: witnesses see the same three ledger totals as the
            // worker; the design system opens both dashboards with Totals.
            (pages::totals(m, &ledger.totals(), false))
            div.at-views {
                div.at-views__bar {
                    span.at-label { (m.tasks) " " span.at-index { (order.len()) } }
                }
                @if order.is_empty() {
                    p.at-muted { (m.no_tasks) }
                } @else {
                    ul.at-list {
                        @for o in order {
                            (row(ctx, ledger, o, by_occ[o], error.filter(|e| e.0 == o).map(|e| e.1)))
                        }
                    }
                }
            }
            (pages::banner(false, m.info_label, m.info_text))
        }
    }
}

/// The row of debt `debt` plus the refreshed totals (out of band): the
/// htmx answer to an action.
pub(super) fn debt_fragment(ctx: &Ctx<'_>, ledger: &Ledger, debt: DebtId, error: Option<&str>) -> Markup {
    let d = debt.0 as usize;
    let Some(o) = ledger.debts().occurrence.get(d).map(|o| o.0 as usize) else {
        return html! { p.row__error role="alert" { (error.unwrap_or(ctx.m.err_unknown_debt)) } };
    };
    html! {
        (row(ctx, ledger, o, Some(d), error))
        (pages::totals(ctx.m, &ledger.totals(), true))
    }
}
