//! The two dashboards (APP-31 worker, APP-32 witness and admin): rows
//! built from a ledger rebuilt under the store lock, rendered as the
//! design system's `Totals` + `TaskViews`: one set of occurrences in
//! four layouts (Gantt, List, Table, Kanban), the same four views
//! Vikunja offers, picked with the `ViewSwitch`.

use std::collections::BTreeMap;

use maud::{html, Markup};

use super::i18n::Msgs;
use super::pages::{self, Status, Variant};
use crate::auth::{Account, Role};
use crate::ledger::{DebtId, EventKind, Ledger, Outcome, TaskId, Timestamp, WitnessId};

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
    /// Vikunja start dates (Gantt bar start). Empty for the admin, whose
    /// fake tasks have none.
    pub starts: &'a BTreeMap<TaskId, Timestamp>,
    /// Now, for the Gantt's today line and overrun hatch.
    pub today: Timestamp,
}

/// The four layouts of `TaskViews`, in the `ViewSwitch` order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum View {
    Gantt,
    List,
    Table,
    Kanban,
}

impl View {
    /// Every view, in switch order (fixed by the design system).
    pub const ALL: [View; 4] = [View::Gantt, View::List, View::Table, View::Kanban];

    /// The `?view=` value and cookie value.
    pub fn key(self) -> &'static str {
        match self {
            View::Gantt => "gantt",
            View::List => "list",
            View::Table => "table",
            View::Kanban => "kanban",
        }
    }

    /// The view named `key`, if it is one.
    pub fn parse(key: &str) -> Option<View> {
        View::ALL.into_iter().find(|v| v.key() == key)
    }

    fn label(self, m: &Msgs) -> &'static str {
        match self {
            View::Gantt => m.view_gantt,
            View::List => m.view_list,
            View::Table => m.view_table,
            View::Kanban => m.view_kanban,
        }
    }
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
    let label = label(ctx, task);
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
                (events_of(m, ledger, o))
                @if let Some(err) = error { p.row__error role="alert" { (err) } }
            }
            div.at-row__act { (action_form(ctx, ledger, d, true)) }
        }
    }
}

/// The task's title for the worker, `Task 47` for everyone else (APP-32).
fn label(ctx: &Ctx<'_>, task: TaskId) -> String {
    match ctx.titles {
        Some(titles) => titles.get(&task).cloned().unwrap_or_else(|| ctx.m.task(task.0)),
        None => ctx.m.task(task.0),
    }
}

/// Every `EventLine` of occurrence `o`, in append order.
fn events_of(m: &Msgs, ledger: &Ledger, o: usize) -> Markup {
    let events = ledger.events();
    let deadline = ledger.occurrences().deadline[o];
    html! {
        @for e in (0..events.occurrence.len()).filter(|&e| events.occurrence[e].0 as usize == o) {
            (event_line(m, events.kind[e], events.new_due[e], deadline, events.at[e]))
        }
    }
}

// DECISION: only the list swaps its row in place over htmx (the action
// fragment is a list row); the table and board post a plain form and
// come back to `/`, which reopens the remembered view.
/// The row's one button as a form, or nothing.
fn action_form(ctx: &Ctx<'_>, ledger: &Ledger, d: Option<usize>, htmx: bool) -> Markup {
    let Some((url, text)) = d.and_then(|d| action(ctx, ledger, d)) else {
        return html! {};
    };
    html! {
        @if htmx {
            form method="post" action=(url) hx-post=(url) hx-target="closest li" hx-swap="outerHTML" {
                (pages::button(text, Variant::Solid, true, false, None))
            }
        } @else {
            form method="post" action=(url) {
                (pages::button(text, Variant::Solid, true, false, None))
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

/// One occurrence as every view sees it.
struct Item {
    o: usize,
    d: Option<usize>,
    st: Status,
    task: TaskId,
    deadline: Timestamp,
}

/// Every occurrence, late first, then sent, upcoming, settled; by
/// deadline inside each.
fn items(ledger: &Ledger) -> Vec<Item> {
    let by_occ = debt_by_occurrence(ledger);
    let occ = ledger.occurrences();
    let mut out: Vec<Item> = (0..occ.task.len())
        .map(|o| Item {
            o,
            d: by_occ[o],
            st: status(ledger, o, by_occ[o]),
            task: occ.task[o],
            deadline: occ.deadline[o],
        })
        .collect();
    out.sort_by_key(|i| (rank(i.st), i.deadline, i.o));
    out
}

/// The dashboard body: banner (admin), totals, then `TaskViews` in
/// `view`. `error` puts a rejection message under one occurrence's row
/// (plain form posts); only the list shows it, so an error always
/// renders the list.
pub(super) fn dashboard(ctx: &Ctx<'_>, ledger: &Ledger, view: View, error: Option<(usize, &str)>) -> Markup {
    let m = ctx.m;
    let items = items(ledger);
    let view = if error.is_some() { View::List } else { view };
    let role = if ctx.viewer == Viewer::Worker { "worker" } else { "witness" };
    html! {
        div.page__stack data-view=(role) {
            @if ctx.viewer == Viewer::Admin {
                div.page__pinned { (pages::banner(true, m.fake_label, m.fake_text)) }
            }
            // DECISION: witnesses see the same three ledger totals as the
            // worker; the design system opens both dashboards with Totals.
            (pages::totals(m, &ledger.totals(), false))
            div.at-views data-layout=(view.key()) {
                div.at-views__bar {
                    span.at-label { (m.tasks) " " span.at-index { (items.len()) } }
                    (switch(m, view))
                }
                @if items.is_empty() {
                    p.at-muted { (m.no_tasks) }
                } @else {
                    @match view {
                        View::Gantt => (gantt(ctx, ledger, &items)),
                        View::List => (list(ctx, ledger, &items, error)),
                        View::Table => (table(ctx, ledger, &items)),
                        View::Kanban => (kanban(ctx, ledger, &items)),
                    }
                }
            }
            (pages::banner(false, m.info_label, m.info_text))
        }
    }
}

// DECISION: the switch is a row of links (`/?view=…`), not buttons that
// swap markup client-side: the page works without JS, and the server
// remembers the choice in a cookie.
/// The `ViewSwitch`.
fn switch(m: &Msgs, on: View) -> Markup {
    html! {
        nav.at-switch aria-label=(m.view_label) {
            @for v in View::ALL {
                a class={ "at-switch__b" @if v == on { " is-on" } }
                    href={ "/?view=" (v.key()) }
                    aria-current=[(v == on).then_some("page")]
                    style="display:inline-flex;align-items:center;text-decoration:none" {
                    (v.label(m))
                }
            }
        }
    }
}

/// `TaskList`.
fn list(ctx: &Ctx<'_>, ledger: &Ledger, items: &[Item], error: Option<(usize, &str)>) -> Markup {
    html! {
        ul.at-list {
            @for i in items {
                (row(ctx, ledger, i.o, i.d, error.filter(|e| e.0 == i.o).map(|e| e.1)))
            }
        }
    }
}

/// The amount text of debt row `d`.
fn money(m: &Msgs, ledger: &Ledger, d: usize) -> String {
    m.amount(ledger.debts().amount[d])
}

/// The two marks of debt row `d`.
fn checks(m: &Msgs, ledger: &Ledger, d: usize) -> Markup {
    let debts = ledger.debts();
    html! {
        span.at-checks {
            (pages::check(m, m.st_sent, debts.sent_at[d].is_some()))
            (pages::check(m, m.st_received, debts.received_at[d].is_some()))
        }
    }
}

/// `TaskTable`: No., Task (worker only), Deadline, Status, Amount,
/// Claimed by, Marks, action.
fn table(ctx: &Ctx<'_>, ledger: &Ledger, items: &[Item]) -> Markup {
    let m = ctx.m;
    let worker = ctx.titles.is_some();
    html! {
        div.at-tablewrap {
            table.at-table {
                thead {
                    tr {
                        th scope="col" { (m.col_no) }
                        @if worker { th scope="col" { (m.col_task) } }
                        th scope="col" { (m.col_deadline) }
                        th scope="col" { (m.col_status) }
                        th scope="col" { (m.col_amount) }
                        th scope="col" { (m.col_claimed) }
                        th scope="col" { (m.col_marks) }
                        th scope="col" {}
                    }
                }
                tbody {
                    @for i in items {
                        tr class={ "at-table__row--" (i.st.class()) } id={ "occ-" (i.o) } {
                            td.at-mono { (i.task.0) }
                            @if worker {
                                td.at-table__title { (label(ctx, i.task)) (events_of(m, ledger, i.o)) }
                            }
                            td.at-mono { (m.date(i.deadline)) }
                            td { (pages::tag(i.st.class(), i.st.word(m))) }
                            @if let Some(d) = i.d {
                                td.at-mono { (money(m, ledger, d)) }
                                td { (owner_text(ctx, ledger.debts().owner[d])) }
                                td { (checks(m, ledger, d)) }
                            } @else {
                                td {} td {} td {}
                            }
                            td.at-table__act { (action_form(ctx, ledger, i.d, false)) }
                        }
                    }
                }
            }
        }
    }
}

// DECISION: the design system's board has four fixed columns; met tasks
// (done in time, no debt) have no debt state, so they get a fifth "Met"
// column at the end instead of disappearing from the board, as Vikunja's
// board keeps a Done column. The grid is widened to five inline.
/// `Kanban`: Upcoming, Late, Sent, Confirmed, then Met.
fn kanban(ctx: &Ctx<'_>, ledger: &Ledger, items: &[Item]) -> Markup {
    let m = ctx.m;
    let cols = [
        (Status::Upcoming, "upcoming", m.st_upcoming),
        (Status::Late, "late", m.st_late),
        (Status::Sent, "sent", m.st_sent),
        (Status::Received, "received", m.confirmed),
        (Status::Met, "met", m.st_met),
    ];
    html! {
        div.at-kanban style="grid-template-columns:repeat(5,minmax(232px,1fr))" {
            @for (n, (st, class, title)) in cols.into_iter().enumerate() {
                @let col: Vec<&Item> = items.iter().filter(|i| i.st == st).collect();
                section class={ "at-col at-col--" (class) } {
                    header.at-col__head {
                        span.at-label { span.at-index { "/0." (n + 1) } " " (title) }
                        span.at-mono { (col.len()) }
                    }
                    @if col.is_empty() {
                        p.at-muted { (m.none) }
                    }
                    @for i in col {
                        article.at-card id={ "occ-" (i.o) } {
                            div.at-card__top {
                                span.at-mono { (m.date(i.deadline)) }
                                @if let Some(d) = i.d { span.at-mono { (money(m, ledger, d)) } }
                            }
                            div.at-card__title { (label(ctx, i.task)) }
                            @if let Some(d) = i.d {
                                div.at-card__meta {
                                    span.at-muted { (owner_text(ctx, ledger.debts().owner[d])) }
                                    (checks(m, ledger, d))
                                }
                            }
                            (events_of(m, ledger, i.o))
                            (action_form(ctx, ledger, i.d, false))
                        }
                    }
                }
            }
        }
    }
}

const DAY_SECS: i64 = 24 * 3600;

/// Whole UTC days since the epoch.
fn day(t: Timestamp) -> i64 {
    t.0.div_euclid(DAY_SECS)
}

/// Where Vikunja last moved occurrence `o`'s date, if it did.
fn moved_to(ledger: &Ledger, o: usize) -> Option<Timestamp> {
    let ev = ledger.events();
    (0..ev.occurrence.len())
        .rev()
        .find(|&e| ev.occurrence[e].0 as usize == o)
        .filter(|&e| matches!(ev.kind[e], EventKind::MovedLater | EventKind::MovedEarlier))
        .and_then(|e| ev.new_due[e])
}

/// A day count as a float for positions.
#[allow(clippy::cast_precision_loss)] // day counts are far below 2^52
fn days_f(n: i64) -> f64 {
    n as f64
}

/// `x` days out of `n` as a CSS percentage.
fn pct(x: f64, n: i64) -> String {
    format!("{:.4}%", x / days_f(n) * 100.0)
}

// DECISION: the range fits every task plus one day before and six after,
// like the design system's Gantt, but settled occurrences (met,
// received) older than 60 days do not stretch it: the grid starts at the
// earlier of 60 days ago and the oldest open one (late, sent, upcoming).
// Bars starting earlier are clipped at the left.
const GANTT_BACK_DAYS: i64 = 60;

/// `Gantt`: one row per occurrence, a bar from the Vikunja start date
/// (one day when there is none) to the locked deadline, the lock tick,
/// a hatched overrun while late, a diamond where Vikunja moved the date.
fn gantt(ctx: &Ctx<'_>, ledger: &Ledger, items: &[Item]) -> Markup {
    let m = ctx.m;
    let today = day(ctx.today);
    let start_of = |i: &Item| ctx.starts.get(&i.task).map(|&s| day(s).min(day(i.deadline)));
    let (mut lo, mut hi) = (today, today);
    for i in items {
        let d = day(i.deadline);
        lo = lo.min(d).min(start_of(i).unwrap_or(d));
        hi = hi.max(d).max(moved_to(ledger, i.o).map_or(d, day));
    }
    let open = items
        .iter()
        .filter(|i| !matches!(i.st, Status::Met | Status::Received))
        .map(|i| start_of(i).unwrap_or(day(i.deadline)))
        .min()
        .unwrap_or(today);
    let from = (lo - 1).max((today - GANTT_BACK_DAYS).min(open - 1));
    let to = hi + 6;
    let n = to - from + 1;
    let fx = |x: i64| days_f(x - from);
    let now = fx(today) + 0.5;
    let head = if ctx.titles.is_some() { m.col_task } else { m.col_task_no };
    html! {
        div.at-gantt role="group" aria-label=(m.view_gantt) {
            div.at-gantt__grid style={ "--days:" (n) ";min-width:calc(var(--gantt-label) + " (n) " * var(--day))" } {
                div class="at-gantt__row at-gantt__row--head" {
                    div.at-gantt__label { span.at-label { (head) } }
                    div class="at-gantt__track at-gantt__days" {
                        @for k in 0..n {
                            @let date = time::OffsetDateTime::from_unix_timestamp((from + k) * DAY_SECS).ok();
                            @let mon = date.is_some_and(|t| t.weekday() == time::Weekday::Monday);
                            div class={ "at-gantt__day" @if mon { " is-mon" } @if from + k == today { " is-today" } } {
                                @if let Some(t) = date {
                                    @if k == 0 || t.day() == 1 {
                                        span.at-gantt__month { (m.month(t.month())) }
                                    }
                                    span { (t.day()) }
                                }
                            }
                        }
                    }
                }
                @for i in items {
                    @let d = fx(day(i.deadline));
                    @let s = fx(start_of(i).unwrap_or(day(i.deadline)).max(from));
                    @let moved = moved_to(ledger, i.o).map(|t| fx(day(t)) + 0.5);
                    @let over = i.st == Status::Late && now > d + 1.0;
                    @let note = match (i.st, i.d) {
                        (Status::Upcoming, _) => m.date(i.deadline),
                        (_, Some(dd)) => format!("{} · {}", i.st.word(m), money(m, ledger, dd)),
                        (st, None) => st.word(m).to_owned(),
                    };
                    @let note_at = (d + 1.0).max(if over { now } else { 0.0 }).max(moved.map_or(0.0, |x| x + 0.5));
                    div class={ "at-gantt__row at-gantt__row--" (i.st.class()) } id={ "occ-" (i.o) } {
                        div.at-gantt__label {
                            i.at-gantt__dot aria-hidden="true" {}
                            span.at-gantt__title { (label(ctx, i.task)) }
                        }
                        div.at-gantt__track {
                            i.at-gantt__today style={ "left:" (pct(now, n)) } {}
                            @if d >= 0.0 {
                                i.at-gantt__bar style={ "left:" (pct(s, n)) ";width:" (pct(d - s + 1.0, n)) } {}
                            }
                            @if over {
                                i.at-gantt__over style={ "left:" (pct((d + 1.0).max(0.0), n)) ";width:" (pct(now - (d + 1.0).max(0.0), n)) } {}
                            }
                            @if d >= 0.0 {
                                i.at-gantt__lock style={ "left:" (pct(d + 1.0, n)) } title=(m.locked(i.deadline)) {}
                            }
                            @if let Some(x) = moved.filter(|&x| x >= 0.0) {
                                i.at-gantt__moved style={ "left:" (pct(x, n)) } {}
                            }
                            span.at-gantt__note style={ "left:" (pct(note_at.max(0.0), n)) } { (note) }
                        }
                    }
                }
            }
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
