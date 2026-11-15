//! Pure domain logic: recorded deadlines, met/late decisions, debts,
//! claims, marks and the amount, held in memory as structure-of-arrays
//! tables. No I/O.
//!
//! Uses: nothing internal.
//! SRS: APP-3..APP-24, NFR-2.
//!
//! # Contract shape: decide, persist, apply
//!
//! Every operation takes `&self` and the current time, checks its rule,
//! and returns [`Change`] values without touching the state. The caller
//! persists the changes (one transaction, `store`), then feeds each one
//! to [`Ledger::apply`], the only mutator. Loading from disk is the same
//! `apply` over the stored changes in order. So memory never runs ahead
//! of the file (Review Focus 4), and `notify` reacts to the same values.
//!
//! The ledger knows no accounts. Callers pass the ids of the real
//! witnesses (admin excluded, APP-20) where a rule needs them, and check
//! "is this the worker" themselves (`auth`).

/// Seconds since the Unix epoch, UTC (§5.1). Eight bytes, so it packs
/// into the table columns without the 16 bytes of `time::OffsetDateTime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

/// Vikunja task id, shown to users as `#<id>` (glossary: task number).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub i64);

/// Row index into [`OccurrenceTable`]. Assigned in recording order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OccurrenceId(pub u32);

/// Row index into [`DebtTable`]. Assigned in creation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DebtId(pub u32);

/// A real witness, by account id. The ledger never sees the admin
/// witness's id (APP-20).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WitnessId(pub i64);

/// An amount in CHF centimes (1000 = 10 CHF). Integer, never float.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Centimes(pub u32);

impl Centimes {
    /// The default amount of a new debt: 10 CHF (APP-15).
    pub const DEFAULT_AMOUNT: Centimes = Centimes(1000);
}

/// How Vikunja repeats a task. Mirrors Vikunja's `repeat_mode`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    /// Not recurring, or repeats every `repeat_after_secs` from the due date.
    Interval = 0,
    /// Repeats on the same day each month.
    Monthly = 1,
    /// Repeats `repeat_after_secs` after the moment it was marked done.
    FromDone = 2,
}

/// One task as one poll saw it. Built by `sync` from the `vikunja`
/// client's records. Fields ordered largest first (NFR-2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Current Vikunja due date. `None` when unset (Vikunja's
    /// `0001-01-01T00:00:00Z` sentinel is mapped to `None` before this).
    pub due: Option<Timestamp>,
    /// Vikunja's `done_at`, when set. Server-stamped on every "mark
    /// done", including recurrence advances, which is how APP-8 tells an
    /// advance from a manual move.
    pub done_at: Option<Timestamp>,
    /// Task id.
    pub task: TaskId,
    /// Vikunja's `repeat_after`, in seconds. 0 = not recurring unless
    /// `repeat_mode` is [`RepeatMode::Monthly`] (APP-8).
    pub repeat_after_secs: u32,
    /// Vikunja's `repeat_mode` (APP-8).
    pub repeat_mode: RepeatMode,
    /// Vikunja's `done` flag.
    pub done: bool,
    /// The task's project is archived (APP-3).
    pub archived: bool,
}

/// Every task the token can see, from one complete, successful poll.
/// A task tracked before and absent here is treated as deleted (APP-6),
/// so a partial poll must never be passed in (Review Focus 2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// One entry per visible task, any order.
    pub tasks: Vec<Observation>,
}

/// The met/late decision of an occurrence (APP-9, APP-10).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Recorded deadline not yet decided.
    Pending = 0,
    /// Done in time.
    Met = 1,
    /// Not done in time; has exactly one debt (APP-11).
    Late = 2,
}

/// What happened to a tracked task's due date (APP-6, APP-7).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// Due date moved later (or set again, later than recorded).
    MovedLater = 0,
    /// Due date moved earlier; the recorded deadline follows (APP-7).
    MovedEarlier = 1,
    /// Due date removed.
    Removed = 2,
    /// Task no longer visible to the token.
    Deleted = 3,
}

/// One ledger fact, as returned by operations, persisted by `store`
/// and applied by [`Ledger::apply`]. Insert-only (APP-13, DB-2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A new occurrence with its recorded deadline (APP-4, APP-8).
    Recorded {
        /// New row id.
        occurrence: OccurrenceId,
        /// Its task.
        task: TaskId,
        /// Recorded deadline.
        deadline: Timestamp,
        /// When it was first seen.
        at: Timestamp,
    },
    /// The task's done flag, as seen by a poll before the deadline,
    /// changed. Kept so APP-9's "last observation before the deadline"
    /// survives a restart (DB-3).
    SeenDone {
        /// The occurrence.
        occurrence: OccurrenceId,
        /// New done flag.
        done: bool,
        /// Poll time.
        at: Timestamp,
    },
    /// A deadline event (APP-6, APP-7). For `MovedEarlier`, `new_due`
    /// becomes the recorded deadline.
    Event {
        /// The occurrence.
        occurrence: OccurrenceId,
        /// What happened.
        kind: EventKind,
        /// Vikunja's due date after the change; `None` for `Removed`
        /// and `Deleted`.
        new_due: Option<Timestamp>,
        /// Poll time.
        at: Timestamp,
    },
    /// The one-time decision (APP-9, APP-10). Never `Pending`.
    Decided {
        /// The occurrence.
        occurrence: OccurrenceId,
        /// Met or late.
        outcome: Outcome,
        /// Decision time.
        at: Timestamp,
    },
    /// A debt for a late occurrence (APP-11, APP-12).
    DebtCreated {
        /// New row id.
        debt: DebtId,
        /// The late occurrence.
        occurrence: OccurrenceId,
        /// Amount in force at the recorded deadline.
        amount: Centimes,
        /// Creation time.
        at: Timestamp,
    },
    /// A debt got its owner (APP-17, APP-18).
    Claimed {
        /// The debt.
        debt: DebtId,
        /// New owner.
        witness: WitnessId,
        /// `true` under APP-17.
        automatic: bool,
        /// Claim time.
        at: Timestamp,
    },
    /// The worker set the sent mark (APP-21).
    MarkedSent {
        /// The debt.
        debt: DebtId,
        /// Mark time.
        at: Timestamp,
    },
    /// The owner witness set the received mark (APP-22).
    MarkedReceived {
        /// The debt.
        debt: DebtId,
        /// Mark time.
        at: Timestamp,
    },
    /// The worker proposed a new amount (APP-16). Replaces any pending
    /// proposal and clears its approvals.
    AmountProposed {
        /// Proposed amount.
        amount: Centimes,
        /// Proposal time.
        at: Timestamp,
    },
    /// A real witness approved the pending proposal (APP-16).
    AmountApproved {
        /// Approver.
        witness: WitnessId,
        /// Approval time.
        at: Timestamp,
    },
    /// The pending proposal came into force: every real witness approved
    /// (APP-16). Applies to debts whose recorded deadline is at or after
    /// `at`.
    AmountInForce {
        /// New amount.
        amount: Centimes,
        /// Moment it took effect.
        at: Timestamp,
    },
}

/// Why an operation was refused. The state is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleError {
    /// No debt with this id.
    UnknownDebt,
    /// The witness is not in the real-witness list (APP-20).
    NotRealWitness,
    /// The debt already has an owner (APP-18).
    AlreadyClaimed,
    /// The debt has no owner yet (APP-21).
    NotClaimed,
    /// The witness does not own this debt (APP-22).
    NotOwner,
    /// Received needs sent first (APP-22).
    NotSent,
    /// The mark is already set and cannot be set again or unset (APP-23).
    AlreadyMarked,
    /// No amount proposal is pending (APP-16).
    NoProposal,
    /// This witness already approved the pending proposal.
    AlreadyApproved,
    /// Proposed amount is zero or equal to the amount in force.
    InvalidAmount,
}

/// Occurrences, one row per recorded deadline. Columns, all the same
/// length, indexed by [`OccurrenceId`] (NFR-2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OccurrenceTable {
    /// Recorded deadline (APP-4, APP-7).
    pub deadline: Vec<Timestamp>,
    /// Task id.
    pub task: Vec<TaskId>,
    /// Met/late decision (APP-9).
    pub outcome: Vec<Outcome>,
}

/// Deadline events, one row per event, in append order (APP-6, APP-7).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventTable {
    /// Vikunja's due date after the change.
    pub new_due: Vec<Option<Timestamp>>,
    /// Poll time.
    pub at: Vec<Timestamp>,
    /// The occurrence it belongs to.
    pub occurrence: Vec<OccurrenceId>,
    /// What happened.
    pub kind: Vec<EventKind>,
}

/// Debts, one row per debt, indexed by [`DebtId`] (APP-11..APP-24).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DebtTable {
    /// Owner witness, once claimed (APP-17, APP-18).
    pub owner: Vec<Option<WitnessId>>,
    /// Sent mark time (APP-21).
    pub sent_at: Vec<Option<Timestamp>>,
    /// Received mark time (APP-22).
    pub received_at: Vec<Option<Timestamp>>,
    /// Creation time.
    pub created_at: Vec<Timestamp>,
    /// The late occurrence.
    pub occurrence: Vec<OccurrenceId>,
    /// Amount, fixed at creation (APP-13).
    pub amount: Vec<Centimes>,
}

/// A pending amount proposal (APP-16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// Proposed amount.
    pub amount: Centimes,
    /// When it was proposed.
    pub proposed_at: Timestamp,
    /// Real witnesses who approved so far.
    pub approvals: Vec<WitnessId>,
}

/// The worker's three totals (APP-24, glossary). Unclaimed debts count
/// in `owed` (APP-19).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Sum of all debts.
    pub owed: Centimes,
    /// Sum of debts with a sent mark.
    pub sent: Centimes,
    /// Sum of debts with a received mark.
    pub confirmed: Centimes,
}

/// The whole ledger state in memory. Rebuilt at startup by applying the
/// stored changes in order (DB-3).
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    occurrences: OccurrenceTable,
    events: EventTable,
    debts: DebtTable,
    // DECISION: bookkeeping the views do not need lives in private
    // columns beside the pub tables, same length as `occurrences`, so the
    // pub contract stays as frozen and `apply` still rebuilds all of it.
    tracking: Tracking,
    // DECISION: a BTreeMap, not a HashMap, so two ledgers built from the
    // same changes have the same Debug output (db_3 compares it).
    current: std::collections::BTreeMap<TaskId, OccurrenceId>,
}

/// Private per-occurrence columns, indexed by [`OccurrenceId`].
#[derive(Clone, Debug, Default)]
struct Tracking {
    /// `Recorded.at`: the moment the occurrence was first seen (APP-8).
    recorded_at: Vec<Timestamp>,
    /// Last done flag seen by a poll before the deadline (APP-9).
    seen_done: Vec<bool>,
    /// Last due-date state seen, to log each change once (APP-6, APP-7).
    last_seen: Vec<Seen>,
}

/// What the last poll showed of an occurrence's task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// Tracked, with this due date.
    Due(Timestamp),
    /// Visible, but its due date was removed.
    NoDue,
    /// Not visible as a tracked task any more.
    Gone,
}

/// Row id for the next row of a column of length `len`.
fn next_id(len: usize) -> u32 {
    // DECISION: u32 rows are a frozen contract choice; four billion
    // occurrences or debts is out of reach for one worker, so overflow
    // is a bug worth a panic rather than a silent wrap.
    u32::try_from(len).expect("ledger table exceeds u32 rows")
}

fn idx(id: u32) -> usize {
    id as usize
}

/// The due date of a tracked task (APP-3), `None` when not tracked.
fn tracked_due(o: &Observation) -> Option<Timestamp> {
    if o.archived { None } else { o.due }
}

impl Ledger {
    /// An empty ledger, amount 10 CHF (APP-15).
    pub fn new() -> Self {
        Self::default()
    }

    /// Mutate the state by one change. The only mutator; used after a
    /// change is persisted and when loading from disk (DB-3).
    pub fn apply(&mut self, change: &Change) {
        match *change {
            Change::Recorded { occurrence, task, deadline, at } => {
                debug_assert_eq!(idx(occurrence.0), self.occurrences.task.len());
                self.occurrences.deadline.push(deadline);
                self.occurrences.task.push(task);
                self.occurrences.outcome.push(Outcome::Pending);
                self.tracking.recorded_at.push(at);
                self.tracking.seen_done.push(false);
                self.tracking.last_seen.push(Seen::Due(deadline));
                self.current.insert(task, occurrence);
            }
            _ => todo!(),
        }
    }

    /// Compare one complete snapshot with the state at `now` and return
    /// every resulting change, in apply order: new recorded deadlines,
    /// done-flag changes, deadline events, recurrence advances,
    /// decisions for deadlines that passed, debts, auto-claims.
    /// `real_witnesses` decides APP-17 vs APP-18.
    /// SRS: APP-3..APP-12, APP-17, APP-18.
    pub fn observe(
        &self,
        snapshot: &Snapshot,
        now: Timestamp,
        real_witnesses: &[WitnessId],
    ) -> Vec<Change> {
        let _ = real_witnesses;
        let mut out = Vec::new();
        let mut next_occ = next_id(self.occurrences.task.len());
        for o in &snapshot.tasks {
            if self.current.contains_key(&o.task) {
                continue;
            }
            if let Some(due) = tracked_due(o) {
                if due > now {
                    out.push(Change::Recorded {
                        occurrence: OccurrenceId(next_occ),
                        task: o.task,
                        deadline: due,
                        at: now,
                    });
                    next_occ += 1;
                }
            }
        }
        out
    }

    /// A real witness claims an unclaimed debt (APP-18, APP-19, APP-20).
    ///
    /// # Errors
    ///
    /// `UnknownDebt`, `NotRealWitness`, `AlreadyClaimed`.
    pub fn claim(
        &self,
        debt: DebtId,
        witness: WitnessId,
        real_witnesses: &[WitnessId],
        now: Timestamp,
    ) -> Result<Change, RuleError> {
        let _ = (debt, witness, real_witnesses, now);
        todo!()
    }

    /// The worker marks a claimed debt as sent. The caller has checked
    /// that the actor is the worker (APP-21, APP-23).
    ///
    /// # Errors
    ///
    /// `UnknownDebt`, `NotClaimed`, `AlreadyMarked`.
    pub fn mark_sent(&self, debt: DebtId, now: Timestamp) -> Result<Change, RuleError> {
        let _ = (debt, now);
        todo!()
    }

    /// The owner witness marks a sent debt as received (APP-22, APP-23).
    ///
    /// # Errors
    ///
    /// `UnknownDebt`, `NotOwner`, `NotSent`, `AlreadyMarked`.
    pub fn mark_received(
        &self,
        debt: DebtId,
        witness: WitnessId,
        now: Timestamp,
    ) -> Result<Change, RuleError> {
        let _ = (debt, witness, now);
        todo!()
    }

    /// The worker proposes a new amount (APP-16). The caller has checked
    /// that the actor is the worker.
    ///
    /// # Errors
    ///
    /// `InvalidAmount`.
    pub fn propose_amount(&self, amount: Centimes, now: Timestamp) -> Result<Change, RuleError> {
        let _ = (amount, now);
        todo!()
    }

    /// A real witness approves the pending proposal. Returns
    /// `AmountApproved`, followed by `AmountInForce` when this was the
    /// last approval needed (APP-16).
    ///
    /// # Errors
    ///
    /// `NotRealWitness`, `NoProposal`, `AlreadyApproved`.
    pub fn approve_amount(
        &self,
        witness: WitnessId,
        real_witnesses: &[WitnessId],
        now: Timestamp,
    ) -> Result<Vec<Change>, RuleError> {
        let _ = (witness, real_witnesses, now);
        todo!()
    }

    /// Owed, sent and confirmed totals (APP-24).
    pub fn totals(&self) -> Totals {
        todo!()
    }

    /// The amount a debt created now would carry (APP-15, APP-16).
    pub fn amount(&self) -> Centimes {
        todo!()
    }

    /// The pending amount proposal, if any (APP-16).
    pub fn proposal(&self) -> Option<&Proposal> {
        todo!()
    }

    /// Read access to the occurrence columns, for the views (APP-31, APP-32).
    pub fn occurrences(&self) -> &OccurrenceTable {
        &self.occurrences
    }

    /// Read access to the deadline-event columns (APP-31, APP-32).
    pub fn events(&self) -> &EventTable {
        &self.events
    }

    /// Read access to the debt columns (APP-24, APP-31, APP-32).
    pub fn debts(&self) -> &DebtTable {
        &self.debts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn nfr_2_column_element_sizes() {
        assert_eq!(size_of::<Timestamp>(), 8);
        assert_eq!(size_of::<TaskId>(), 8);
        assert_eq!(size_of::<OccurrenceId>(), 4);
        assert_eq!(size_of::<DebtId>(), 4);
        assert_eq!(size_of::<Centimes>(), 4);
        assert_eq!(size_of::<Outcome>(), 1);
        assert_eq!(size_of::<EventKind>(), 1);
        assert_eq!(size_of::<Observation>(), 48);
    }

    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;

    fn t(secs: i64) -> Timestamp {
        Timestamp(secs)
    }

    /// A plain, non-recurring, open task.
    fn obs(task: i64, due: Option<i64>) -> Observation {
        Observation {
            due: due.map(Timestamp),
            done_at: None,
            task: TaskId(task),
            repeat_after_secs: 0,
            repeat_mode: RepeatMode::Interval,
            done: false,
            archived: false,
        }
    }

    /// A task marked done at `done_at`.
    fn done(mut o: Observation, done_at: i64) -> Observation {
        o.done = true;
        o.done_at = Some(t(done_at));
        o
    }

    /// A daily recurring task.
    fn daily(task: i64, due: i64, done_at: Option<i64>) -> Observation {
        let mut o = obs(task, Some(due));
        o.repeat_after_secs = u32::try_from(DAY).unwrap();
        o.done_at = done_at.map(Timestamp);
        o
    }

    fn snap(tasks: Vec<Observation>) -> Snapshot {
        Snapshot { tasks }
    }

    /// Observe and apply every change, as `sync` does.
    fn poll(l: &mut Ledger, s: &Snapshot, now: i64, witnesses: &[WitnessId]) -> Vec<Change> {
        let changes = l.observe(s, t(now), witnesses);
        for c in &changes {
            l.apply(c);
        }
        changes
    }

    fn apply_all(l: &mut Ledger, changes: &[Change]) {
        for c in changes {
            l.apply(c);
        }
    }

    // ledger-01
    #[test]
    fn nfr_2_tables_are_columnar() {
        let mut l = Ledger::new();
        let n = 5;
        let tasks = (0..n).map(|i| obs(i, Some(DAY + i))).collect();
        poll(&mut l, &snap(tasks), 0, &[]);
        let o = l.occurrences();
        assert_eq!(o.deadline.len(), 5);
        assert_eq!(o.task.len(), 5);
        assert_eq!(o.outcome.len(), 5);
        assert_eq!(o.task[3], TaskId(3));
        assert_eq!(o.deadline[3], t(DAY + 3));
        assert!(o.outcome.iter().all(|&x| x == Outcome::Pending));
    }

    #[test]
    fn app_3_task_without_due_date_not_tracked() {
        let l = Ledger::new();
        let changes = l.observe(&snap(vec![obs(1, None)]), t(0), &[]);
        assert!(changes.is_empty());
    }

    #[test]
    fn app_3_archived_project_not_tracked() {
        let l = Ledger::new();
        let mut o = obs(1, Some(DAY));
        o.archived = true;
        let changes = l.observe(&snap(vec![o]), t(0), &[]);
        assert!(changes.is_empty());
    }

    // ledger-02
    #[test] fn app_4_records_future_deadline() { todo!() }
    #[test] fn app_5_past_due_at_first_sight_no_record() { todo!() }
    #[test] fn app_6_moved_later_keeps_recorded_and_logs_event() { todo!() }
    #[test] fn app_6_removed_due_keeps_recorded_and_logs_event() { todo!() }
    #[test] fn app_6_deleted_task_keeps_recorded_and_logs_event() { todo!() }
    #[test] fn app_7_moved_earlier_updates_recorded_and_logs_event() { todo!() }

    // ledger-03
    #[test] fn app_8_recurrence_advance_closes_and_opens_occurrence() { todo!() }
    #[test] fn app_8_recurrence_advance_is_not_a_deadline_event() { todo!() }
    #[test] fn app_8_manual_move_of_recurring_task_is_a_deadline_event() { todo!() }
    #[test] fn app_8_advance_while_down_detected_from_done_at() { todo!() }

    // ledger-04
    #[test] fn app_9_done_before_deadline_is_met() { todo!() }
    #[test] fn app_9_done_after_deadline_is_late() { todo!() }
    #[test] fn app_9_seen_done_then_reopened_after_deadline_is_met() { todo!() }
    #[test] fn app_9_deleted_never_seen_done_is_late() { todo!() }
    #[test] fn app_9_decided_on_first_poll_after_deadline_even_if_late_poll() { todo!() }
    #[test] fn app_10_decision_never_revised() { todo!() }

    // ledger-05
    #[test] fn app_11_late_creates_one_debt_with_amount_in_force() { todo!() }
    #[test] fn app_12_moving_deadline_never_creates_second_debt() { todo!() }
    #[test] fn app_13_no_operation_removes_or_changes_debt() { todo!() }
    #[test] fn app_15_default_amount_is_10_chf() { todo!() }
    #[test] fn app_16_amount_needs_every_real_witness() { todo!() }
    #[test] fn app_16_new_amount_not_retroactive() { todo!() }

    // ledger-06
    #[test] fn app_17_single_witness_auto_claims() { todo!() }
    #[test] fn app_18_zero_witnesses_leaves_claimable() { todo!() }
    #[test] fn app_18_first_claim_wins_second_rejected() { todo!() }
    #[test] fn app_19_unclaimed_counts_in_owed() { todo!() }
    #[test] fn app_19_late_joiner_can_claim_old_debt() { todo!() }
    #[test] fn app_20_admin_not_counted_as_real_witness() { todo!() }
    #[test] fn app_21_only_claimed_debt_can_be_marked_sent() { todo!() }
    #[test] fn app_22_received_only_by_owner_after_sent() { todo!() }
    #[test] fn app_23_marks_cannot_be_unset() { todo!() }
    #[test] fn app_24_totals_owed_sent_confirmed() { todo!() }
    #[test] fn ledger_double_claim_same_witness_is_rejected_not_duplicated() { todo!() }

    // DB-3, Review Focus 4
    #[test] fn db_3_replaying_changes_rebuilds_same_state() { todo!() }
}
