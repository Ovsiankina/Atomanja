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
    /// Due date set again, equal to the recorded deadline (APP-6).
    Restored = 4,
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
    /// Every `AmountInForce`, in order. Empty means the default amount.
    // DECISION: amounts stay integer centimes (`Centimes`), never float,
    // and the history is kept so a late decision uses the amount in force
    // at its recorded deadline, not at decision time (APP-11, APP-16).
    amounts: Vec<(Timestamp, Centimes)>,
    /// The pending proposal (APP-16).
    proposal: Option<Proposal>,
}

/// Private per-occurrence columns, indexed by [`OccurrenceId`].
#[derive(Clone, Debug, Default)]
struct Tracking {
    /// `Recorded.at`: the moment the occurrence was first seen (APP-8).
    recorded_at: Vec<Timestamp>,
    /// Every done-flag change seen by a poll before the deadline, as
    /// `(SeenDone.at, done)` in poll order (APP-9).
    // DECISION: the whole history, not just the last flag, because a
    // MovedEarlier event can pull the deadline before some of these polls;
    // APP-9 then needs the last flag seen at or before the new deadline.
    seen_done: Vec<Vec<(Timestamp, bool)>>,
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

/// Changes being built by one [`Ledger::observe`] call, with the row ids
/// they have claimed so far.
struct Batch {
    /// The single real witness, when there is exactly one (APP-17).
    auto_owner: Option<WitnessId>,
    out: Vec<Change>,
    next_occ: u32,
    next_debt: u32,
}

impl Batch {
    /// A task seen without a current occurrence (APP-4, APP-5).
    fn first_sight(&mut self, o: &Observation, now: Timestamp) {
        if let Some(due) = tracked_due(o)
            && due > now
        {
            self.record(o, due, now);
        }
    }

    /// The one-time met/late decision (APP-9, APP-10).
    /// A late one gets its single debt at `amount` (APP-11, APP-12).
    fn decide(&mut self, occurrence: OccurrenceId, met: bool, amount: Centimes, now: Timestamp) {
        let outcome = if met { Outcome::Met } else { Outcome::Late };
        self.out.push(Change::Decided {
            occurrence,
            outcome,
            at: now,
        });
        if !met {
            let debt = DebtId(self.next_debt);
            self.next_debt += 1;
            self.out.push(Change::DebtCreated {
                debt,
                occurrence,
                amount,
                at: now,
            });
            if let Some(witness) = self.auto_owner {
                self.out.push(Change::Claimed {
                    debt,
                    witness,
                    automatic: true,
                    at: now,
                });
            }
        }
    }

    /// Start a new occurrence.
    fn record(&mut self, o: &Observation, deadline: Timestamp, now: Timestamp) {
        let occurrence = OccurrenceId(self.next_occ);
        self.next_occ += 1;
        self.out.push(Change::Recorded {
            occurrence,
            task: o.task,
            deadline,
            at: now,
        });
        if o.done {
            self.out.push(Change::SeenDone {
                occurrence,
                done: true,
                at: now,
            });
        }
    }
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
            Change::Recorded {
                occurrence,
                task,
                deadline,
                at,
            } => {
                debug_assert_eq!(idx(occurrence.0), self.occurrences.task.len());
                self.occurrences.deadline.push(deadline);
                self.occurrences.task.push(task);
                self.occurrences.outcome.push(Outcome::Pending);
                self.tracking.recorded_at.push(at);
                self.tracking.seen_done.push(Vec::new());
                self.tracking.last_seen.push(Seen::Due(deadline));
                self.current.insert(task, occurrence);
            }
            Change::Event {
                occurrence,
                kind,
                new_due,
                at,
            } => {
                let i = idx(occurrence.0);
                self.events.new_due.push(new_due);
                self.events.at.push(at);
                self.events.occurrence.push(occurrence);
                self.events.kind.push(kind);
                self.tracking.last_seen[i] = match (kind, new_due) {
                    (EventKind::Deleted, _) => Seen::Gone,
                    (_, None) => Seen::NoDue,
                    (_, Some(d)) => Seen::Due(d),
                };
                // DECISION: a decided occurrence keeps its deadline; only a
                // pending one follows an earlier due date (APP-7, APP-10).
                if kind == EventKind::MovedEarlier
                    && self.occurrences.outcome[i] == Outcome::Pending
                    && let Some(d) = new_due
                {
                    self.occurrences.deadline[i] = d;
                }
            }
            Change::SeenDone {
                occurrence,
                done,
                at,
            } => {
                self.tracking.seen_done[idx(occurrence.0)].push((at, done));
            }
            Change::Decided {
                occurrence,
                outcome,
                ..
            } => {
                self.occurrences.outcome[idx(occurrence.0)] = outcome;
            }
            Change::DebtCreated {
                debt,
                occurrence,
                amount,
                at,
            } => {
                debug_assert_eq!(idx(debt.0), self.debts.occurrence.len());
                self.debts.owner.push(None);
                self.debts.sent_at.push(None);
                self.debts.received_at.push(None);
                self.debts.created_at.push(at);
                self.debts.occurrence.push(occurrence);
                self.debts.amount.push(amount);
            }
            Change::AmountProposed { amount, at } => {
                self.proposal = Some(Proposal {
                    amount,
                    proposed_at: at,
                    approvals: Vec::new(),
                });
            }
            Change::AmountApproved { witness, .. } => {
                if let Some(p) = self.proposal.as_mut() {
                    p.approvals.push(witness);
                }
            }
            Change::AmountInForce { amount, at } => {
                self.amounts.push((at, amount));
                self.proposal = None;
            }
            Change::Claimed { debt, witness, .. } => {
                self.debts.owner[idx(debt.0)] = Some(witness);
            }
            Change::MarkedSent { debt, at } => {
                self.debts.sent_at[idx(debt.0)] = Some(at);
            }
            Change::MarkedReceived { debt, at } => {
                self.debts.received_at[idx(debt.0)] = Some(at);
            }
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
        let mut batch = Batch {
            auto_owner: match real_witnesses {
                [only] => Some(*only),
                _ => None,
            },
            out: Vec::new(),
            next_occ: next_id(self.occurrences.task.len()),
            next_debt: next_id(self.debts.occurrence.len()),
        };
        // DECISION: tasks are handled one by one in task-id order (seen
        // tasks and vanished ones alike), each emitting its own changes in
        // dependency order. Deterministic whatever the snapshot order, and
        // every change only refers to rows created earlier in the list.
        let mut seen: std::collections::BTreeMap<TaskId, &Observation> =
            std::collections::BTreeMap::new();
        for o in &snapshot.tasks {
            seen.insert(o.task, o);
        }
        let mut tasks: Vec<TaskId> = seen.keys().copied().collect();
        tasks.extend(self.current.keys().filter(|k| !seen.contains_key(k)));
        tasks.sort_unstable();
        for task in tasks {
            let o = seen.get(&task).copied();
            match self.current.get(&task) {
                Some(&occ) => self.step_tracked(occ, o, now, &mut batch),
                None => {
                    if let Some(o) = o {
                        batch.first_sight(o, now);
                    }
                }
            }
        }
        batch.out
    }

    /// The amount in force at `moment` (APP-11, APP-16).
    fn amount_at(&self, moment: Timestamp) -> Centimes {
        self.amounts
            .iter()
            .rev()
            .find(|&&(at, _)| at <= moment)
            .map_or(Centimes::DEFAULT_AMOUNT, |&(_, a)| a)
    }

    /// The done flag of the last poll at or before `deadline` (APP-9).
    /// `false` when no poll up to then saw it done.
    fn seen_done_by(&self, i: usize, deadline: Timestamp) -> bool {
        self.tracking.seen_done[i]
            .iter()
            .rev()
            .find(|&&(at, _)| at <= deadline)
            .is_some_and(|&(_, done)| done)
    }

    /// The done flag of the most recent poll that saw this occurrence.
    fn seen_done_last(&self, i: usize) -> bool {
        self.tracking.seen_done[i]
            .last()
            .is_some_and(|&(_, done)| done)
    }

    /// APP-8: recurring, and marked done after the current occurrence
    /// was recorded. The caller has checked that the due date changed.
    fn is_advance(&self, i: usize, o: &Observation) -> bool {
        let recurring = o.repeat_after_secs > 0 || o.repeat_mode == RepeatMode::Monthly;
        recurring && o.done_at.is_some_and(|d| d > self.tracking.recorded_at[i])
    }

    /// One poll's changes for a task that already has an occurrence.
    fn step_tracked(
        &self,
        occ: OccurrenceId,
        o: Option<&Observation>,
        now: Timestamp,
        batch: &mut Batch,
    ) {
        let i = idx(occ.0);
        let recorded = self.occurrences.deadline[i];
        let seen = match o {
            None => Seen::Gone,
            Some(o) if o.archived => Seen::Gone,
            Some(o) => o.due.map_or(Seen::NoDue, Seen::Due),
        };
        let changed = seen != self.tracking.last_seen[i];
        if let (true, Some(o), Seen::Due(new_due)) = (changed, o, seen)
            && self.is_advance(i, o)
        {
            if self.occurrences.outcome[i] == Outcome::Pending {
                // The mark-done that advanced it is the done proof.
                let met =
                    o.done_at.is_some_and(|d| d <= recorded) || self.seen_done_by(i, recorded);
                batch.decide(occ, met, self.amount_at(recorded), now);
            }
            // DECISION: a new occurrence whose due date is already past
            // when first seen is not recorded, as APP-5 says for any
            // first sight; the closed one stays current until the next
            // advance into the future.
            if new_due > now {
                batch.record(o, new_due, now);
            }
            return;
        }
        let pending = self.occurrences.outcome[i] == Outcome::Pending;
        let mut deadline = recorded;
        let visible = o.filter(|o| !o.archived);
        if changed {
            let (kind, new_due) = match seen {
                Seen::Gone => (EventKind::Deleted, None),
                Seen::NoDue => (EventKind::Removed, None),
                // DECISION: "earlier" and "later" are measured against the
                // recorded deadline, not the previous due date, so the
                // recorded deadline only ever moves earlier (APP-7), and a
                // move back towards it is logged as MovedLater.
                // DECISION: a due date set to exactly the recorded deadline
                // is logged as Restored, never skipped: only an Event
                // updates last_seen on replay, so skipping it would leave a
                // later removal unlogged (APP-6).
                Seen::Due(d) if d < recorded => (EventKind::MovedEarlier, Some(d)),
                Seen::Due(d) if d == recorded => (EventKind::Restored, Some(d)),
                Seen::Due(d) => (EventKind::MovedLater, Some(d)),
            };
            if kind == EventKind::MovedEarlier && pending {
                deadline = new_due.unwrap_or(recorded);
            }
            batch.out.push(Change::Event {
                occurrence: occ,
                kind,
                new_due,
                at: now,
            });
        }
        if !pending {
            return;
        }
        // DECISION: a task deleted in Vikunja (or whose project was
        // archived) is gone from Atomanja too (APP-6a, David 2026-11-15):
        // while gone, its pending occurrence is never decided, so it can
        // never create a debt. If it comes back, APP-9 decides it at the
        // first poll that sees it again. A decided occurrence keeps its
        // outcome and debt (APP-10, APP-13): the early return above.
        if seen == Seen::Gone {
            return;
        }
        if now <= deadline {
            // DECISION: a task that vanished or was archived shows no done
            // flag, so the last one seen before it went is kept (APP-9).
            if let Some(o) = visible
                && o.done != self.seen_done_last(i)
            {
                batch.out.push(Change::SeenDone {
                    occurrence: occ,
                    done: o.done,
                    at: now,
                });
            }
            return;
        }
        // First successful poll after the deadline (APP-9).
        // DECISION: `done` without a `done_at` proves nothing about when;
        // Vikunja always stamps it, so this only guards odd data.
        let done_in_time =
            visible.is_some_and(|o| o.done && o.done_at.is_some_and(|d| d <= deadline));
        batch.decide(
            occ,
            done_in_time || self.seen_done_by(i, deadline),
            self.amount_at(deadline),
            now,
        );
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
        let i = self.debt_index(debt)?;
        if !real_witnesses.contains(&witness) {
            return Err(RuleError::NotRealWitness);
        }
        if self.debts.owner[i].is_some() {
            return Err(RuleError::AlreadyClaimed);
        }
        Ok(Change::Claimed {
            debt,
            witness,
            automatic: false,
            at: now,
        })
    }

    /// Row of a known debt.
    fn debt_index(&self, debt: DebtId) -> Result<usize, RuleError> {
        let i = idx(debt.0);
        if i < self.debts.occurrence.len() {
            Ok(i)
        } else {
            Err(RuleError::UnknownDebt)
        }
    }

    /// The worker marks a claimed debt as sent. The caller has checked
    /// that the actor is the worker (APP-21, APP-23).
    ///
    /// # Errors
    ///
    /// `UnknownDebt`, `NotClaimed`, `AlreadyMarked`.
    pub fn mark_sent(&self, debt: DebtId, now: Timestamp) -> Result<Change, RuleError> {
        let i = self.debt_index(debt)?;
        if self.debts.owner[i].is_none() {
            return Err(RuleError::NotClaimed);
        }
        if self.debts.sent_at[i].is_some() {
            return Err(RuleError::AlreadyMarked);
        }
        Ok(Change::MarkedSent { debt, at: now })
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
        let i = self.debt_index(debt)?;
        if self.debts.owner[i] != Some(witness) {
            return Err(RuleError::NotOwner);
        }
        if self.debts.sent_at[i].is_none() {
            return Err(RuleError::NotSent);
        }
        if self.debts.received_at[i].is_some() {
            return Err(RuleError::AlreadyMarked);
        }
        Ok(Change::MarkedReceived { debt, at: now })
    }

    /// The worker proposes a new amount (APP-16). The caller has checked
    /// that the actor is the worker.
    ///
    /// # Errors
    ///
    /// `InvalidAmount`.
    pub fn propose_amount(&self, amount: Centimes, now: Timestamp) -> Result<Change, RuleError> {
        if amount.0 == 0 || amount == self.amount() {
            return Err(RuleError::InvalidAmount);
        }
        Ok(Change::AmountProposed { amount, at: now })
    }

    /// A real witness approves the pending proposal. Returns
    /// `AmountApproved`, followed by `AmountInForce` when this was the
    /// last approval needed (APP-16). A witness who already approved
    /// gets `AmountInForce` alone when every current real witness has
    /// approved (the last missing one left the list).
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
        if !real_witnesses.contains(&witness) {
            return Err(RuleError::NotRealWitness);
        }
        let p = self.proposal.as_ref().ok_or(RuleError::NoProposal)?;
        if p.approvals.contains(&witness) {
            // DECISION: if the last missing witness left after everyone
            // else approved, a repeat approval from a remaining witness
            // brings the proposal into force instead of leaving it stuck
            // (APP-16: "every real witness has approved").
            if real_witnesses.iter().all(|w| p.approvals.contains(w)) {
                return Ok(vec![Change::AmountInForce {
                    amount: p.amount,
                    at: now,
                }]);
            }
            return Err(RuleError::AlreadyApproved);
        }
        // DECISION: with zero real witnesses nobody can approve, so a
        // proposal never comes into force; the worker keeps the amount in
        // force. Accepted: approving needs a real witness by definition.
        let mut out = vec![Change::AmountApproved { witness, at: now }];
        // DECISION: "every real witness" is the list passed now, so a
        // witness who joined after the proposal must approve too, and an
        // approval from someone no longer real does not count.
        let all = real_witnesses
            .iter()
            .all(|w| *w == witness || p.approvals.contains(w));
        if all {
            out.push(Change::AmountInForce {
                amount: p.amount,
                at: now,
            });
        }
        Ok(out)
    }

    /// Owed, sent and confirmed totals (APP-24).
    pub fn totals(&self) -> Totals {
        // DECISION: saturating sums; u32 centimes cap at ~42.9M CHF, far
        // past any real total, and a capped figure beats a panic in a view.
        let sum = |keep: &dyn Fn(usize) -> bool| {
            let total = self
                .debts
                .amount
                .iter()
                .enumerate()
                .filter(|&(i, _)| keep(i))
                .fold(0u32, |acc, (_, a)| acc.saturating_add(a.0));
            Centimes(total)
        };
        Totals {
            owed: sum(&|_| true),
            sent: sum(&|i| self.debts.sent_at[i].is_some()),
            confirmed: sum(&|i| self.debts.received_at[i].is_some()),
        }
    }

    /// The amount a debt created now would carry (APP-15, APP-16).
    pub fn amount(&self) -> Centimes {
        self.amounts
            .last()
            .map_or(Centimes::DEFAULT_AMOUNT, |&(_, a)| a)
    }

    /// The pending amount proposal, if any (APP-16).
    pub fn proposal(&self) -> Option<&Proposal> {
        self.proposal.as_ref()
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

    /// Whether the last poll before the deadline saw `occurrence`'s task
    /// done, so the views can show it done while it waits for its APP-9
    /// decision. `false` for an unknown occurrence.
    pub fn seen_done(&self, occurrence: OccurrenceId) -> bool {
        let i = occurrence.0 as usize;
        i < self.tracking.seen_done.len() && self.seen_done_last(i)
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
    #[test]
    fn app_4_records_future_deadline() {
        let mut l = Ledger::new();
        let changes = poll(&mut l, &snap(vec![obs(7, Some(DAY))]), 10, &[]);
        assert_eq!(
            changes,
            vec![Change::Recorded {
                occurrence: OccurrenceId(0),
                task: TaskId(7),
                deadline: t(DAY),
                at: t(10),
            }]
        );
        assert_eq!(l.occurrences().deadline, vec![t(DAY)]);
        // Seen again unchanged: nothing new.
        assert!(poll(&mut l, &snap(vec![obs(7, Some(DAY))]), 20, &[]).is_empty());
    }

    #[test]
    fn app_5_past_due_at_first_sight_no_record() {
        let mut l = Ledger::new();
        assert!(poll(&mut l, &snap(vec![obs(1, Some(100))]), 200, &[]).is_empty());
        assert!(poll(&mut l, &snap(vec![obs(1, Some(100))]), 300, &[]).is_empty());
        assert!(l.occurrences().task.is_empty());
        assert!(l.debts().occurrence.is_empty());
    }

    #[test]
    fn app_6_moved_later_keeps_recorded_and_logs_event() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        let changes = poll(&mut l, &snap(vec![obs(1, Some(2 * DAY))]), 10, &[]);
        assert_eq!(
            changes,
            vec![Change::Event {
                occurrence: OccurrenceId(0),
                kind: EventKind::MovedLater,
                new_due: Some(t(2 * DAY)),
                at: t(10),
            }]
        );
        assert_eq!(l.occurrences().deadline, vec![t(DAY)]);
        assert_eq!(l.events().kind, vec![EventKind::MovedLater]);
        // Logged once, not on every poll.
        assert!(poll(&mut l, &snap(vec![obs(1, Some(2 * DAY))]), 20, &[]).is_empty());
    }

    #[test]
    fn app_6_removed_due_keeps_recorded_and_logs_event() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        let changes = poll(&mut l, &snap(vec![obs(1, None)]), 10, &[]);
        assert_eq!(
            changes,
            vec![Change::Event {
                occurrence: OccurrenceId(0),
                kind: EventKind::Removed,
                new_due: None,
                at: t(10),
            }]
        );
        assert_eq!(l.occurrences().deadline, vec![t(DAY)]);
        assert!(poll(&mut l, &snap(vec![obs(1, None)]), 20, &[]).is_empty());
    }

    #[test]
    fn app_6_due_restored_to_recorded_is_logged_as_restored() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        poll(&mut l, &snap(vec![obs(1, None)]), 10, &[]);
        let c = poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 20, &[]);
        assert_eq!(
            c,
            vec![Change::Event {
                occurrence: OccurrenceId(0),
                kind: EventKind::Restored,
                new_due: Some(t(DAY)),
                at: t(20),
            }]
        );
        assert_eq!(l.occurrences().deadline, vec![t(DAY)]);
    }

    #[test]
    fn app_6_due_restored_to_recorded_then_removed_again_is_logged() {
        // Restoring the due date to exactly the recorded deadline must
        // leave the ledger able to log a later removal (APP-6).
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        poll(&mut l, &snap(vec![obs(1, None)]), 10, &[]);
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 20, &[]);
        assert_eq!(
            l.events().kind,
            vec![EventKind::Removed, EventKind::Restored]
        );
        assert!(poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 25, &[]).is_empty());
        let c = poll(&mut l, &snap(vec![obs(1, None)]), 30, &[]);
        assert_eq!(
            c,
            vec![Change::Event {
                occurrence: OccurrenceId(0),
                kind: EventKind::Removed,
                new_due: None,
                at: t(30),
            }]
        );
        assert_eq!(l.occurrences().deadline, vec![t(DAY)]);
    }

    #[test]
    fn app_6_deleted_task_keeps_recorded_and_logs_event() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        let changes = poll(&mut l, &snap(vec![]), 10, &[]);
        assert_eq!(
            changes,
            vec![Change::Event {
                occurrence: OccurrenceId(0),
                kind: EventKind::Deleted,
                new_due: None,
                at: t(10),
            }]
        );
        assert_eq!(l.occurrences().deadline, vec![t(DAY)]);
        assert_eq!(l.events().occurrence, vec![OccurrenceId(0)]);
        assert!(poll(&mut l, &snap(vec![]), 20, &[]).is_empty());
    }

    #[test]
    fn app_7_moved_earlier_updates_recorded_and_logs_event() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(2 * DAY))]), 0, &[]);
        let changes = poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 10, &[]);
        assert_eq!(
            changes,
            vec![Change::Event {
                occurrence: OccurrenceId(0),
                kind: EventKind::MovedEarlier,
                new_due: Some(t(DAY)),
                at: t(10),
            }]
        );
        assert_eq!(l.occurrences().deadline, vec![t(DAY)]);
        assert_eq!(l.events().new_due, vec![Some(t(DAY))]);
    }

    // ledger-03
    #[test]
    fn app_8_recurrence_advance_closes_and_opens_occurrence() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![daily(1, DAY, None)]), 0, &[]);
        // Marked done at 10; Vikunja moves the due date one day on.
        let changes = poll(&mut l, &snap(vec![daily(1, 2 * DAY, Some(10))]), 20, &[]);
        assert_eq!(
            changes,
            vec![
                Change::Decided {
                    occurrence: OccurrenceId(0),
                    outcome: Outcome::Met,
                    at: t(20)
                },
                Change::Recorded {
                    occurrence: OccurrenceId(1),
                    task: TaskId(1),
                    deadline: t(2 * DAY),
                    at: t(20),
                },
            ]
        );
        let o = l.occurrences();
        assert_eq!(o.outcome, vec![Outcome::Met, Outcome::Pending]);
        assert_eq!(o.deadline, vec![t(DAY), t(2 * DAY)]);
        // Same snapshot again: the new occurrence is stable.
        assert!(poll(&mut l, &snap(vec![daily(1, 2 * DAY, Some(10))]), 30, &[]).is_empty());
    }

    #[test]
    fn app_8_recurrence_advance_is_not_a_deadline_event() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![daily(1, DAY, None)]), 0, &[]);
        let changes = poll(&mut l, &snap(vec![daily(1, 2 * DAY, Some(10))]), 20, &[]);
        assert!(!changes.iter().any(|c| matches!(c, Change::Event { .. })));
        assert!(l.events().kind.is_empty());
        // Monthly mode counts as recurring too, with repeat_after 0.
        let mut m = Ledger::new();
        let mut o = obs(2, Some(DAY));
        o.repeat_mode = RepeatMode::Monthly;
        poll(&mut m, &snap(vec![o.clone()]), 0, &[]);
        o.due = Some(t(31 * DAY));
        o.done_at = Some(t(10));
        poll(&mut m, &snap(vec![o]), 20, &[]);
        assert!(m.events().kind.is_empty());
        assert_eq!(m.occurrences().task.len(), 2);
    }

    #[test]
    fn app_8_manual_move_of_recurring_task_is_a_deadline_event() {
        let mut l = Ledger::new();
        // done_at from an earlier occurrence, before this one was recorded.
        poll(&mut l, &snap(vec![daily(1, DAY, Some(50))]), 100, &[]);
        let changes = poll(&mut l, &snap(vec![daily(1, 2 * DAY, Some(50))]), 200, &[]);
        assert_eq!(
            changes,
            vec![Change::Event {
                occurrence: OccurrenceId(0),
                kind: EventKind::MovedLater,
                new_due: Some(t(2 * DAY)),
                at: t(200),
            }]
        );
        assert_eq!(l.occurrences().task.len(), 1);
        assert_eq!(l.occurrences().outcome, vec![Outcome::Pending]);
        // Not recurring at all: a done_at after recording is no advance.
        let mut n = Ledger::new();
        poll(&mut n, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        let mut o = obs(1, Some(2 * DAY));
        o.done_at = Some(t(10));
        poll(&mut n, &snap(vec![o]), 20, &[]);
        assert_eq!(n.events().kind, vec![EventKind::MovedLater]);
        assert_eq!(n.occurrences().task.len(), 1);
    }

    #[test]
    fn app_8_advance_while_down_detected_from_done_at() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![daily(1, DAY, None)]), 0, &[]);
        // Down from 1h to after the deadline. Meanwhile the task was
        // marked done before its deadline and advanced; done is false
        // again, only done_at shows it.
        let now = DAY + 5 * HOUR;
        let changes = poll(
            &mut l,
            &snap(vec![daily(1, 2 * DAY, Some(DAY - HOUR))]),
            now,
            &[],
        );
        assert_eq!(
            changes,
            vec![
                Change::Decided {
                    occurrence: OccurrenceId(0),
                    outcome: Outcome::Met,
                    at: t(now)
                },
                Change::Recorded {
                    occurrence: OccurrenceId(1),
                    task: TaskId(1),
                    deadline: t(2 * DAY),
                    at: t(now),
                },
            ]
        );
        assert!(l.events().kind.is_empty());
    }

    // ledger-04
    fn decisions(changes: &[Change]) -> Vec<(OccurrenceId, Outcome, Timestamp)> {
        changes
            .iter()
            .filter_map(|c| match *c {
                Change::Decided {
                    occurrence,
                    outcome,
                    at,
                } => Some((occurrence, outcome, at)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn app_9_done_before_deadline_is_met() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        let c = poll(
            &mut l,
            &snap(vec![done(obs(1, Some(DAY)), DAY - 10)]),
            DAY + 1,
            &[],
        );
        assert_eq!(
            decisions(&c),
            vec![(OccurrenceId(0), Outcome::Met, t(DAY + 1))]
        );
        assert_eq!(l.occurrences().outcome, vec![Outcome::Met]);
        assert!(l.debts().occurrence.is_empty());
    }

    #[test]
    fn app_9_done_after_deadline_is_late() {
        let mut l = Ledger::new();
        poll(
            &mut l,
            &snap(vec![obs(1, Some(DAY)), obs(2, Some(DAY))]),
            0,
            &[],
        );
        let s = snap(vec![done(obs(1, Some(DAY)), DAY + 10), obs(2, Some(DAY))]);
        let c = poll(&mut l, &s, DAY + 20, &[]);
        assert_eq!(
            decisions(&c),
            vec![
                (OccurrenceId(0), Outcome::Late, t(DAY + 20)),
                (OccurrenceId(1), Outcome::Late, t(DAY + 20)),
            ]
        );
    }

    #[test]
    fn seen_done_follows_the_last_poll_before_the_deadline() {
        let mut l = Ledger::new();
        assert!(!l.seen_done(OccurrenceId(0)), "unknown occurrence");
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        assert!(!l.seen_done(OccurrenceId(0)), "open");
        poll(&mut l, &snap(vec![done(obs(1, Some(DAY)), 5)]), 10, &[]);
        assert!(l.seen_done(OccurrenceId(0)), "marked done, still pending");
        assert_eq!(l.occurrences().outcome[0], Outcome::Pending);
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 20, &[]);
        assert!(!l.seen_done(OccurrenceId(0)), "reopened");
    }

    #[test]
    fn app_9_seen_done_then_reopened_after_deadline_is_met() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        let c = poll(&mut l, &snap(vec![done(obs(1, Some(DAY)), 5)]), 10, &[]);
        assert_eq!(
            c,
            vec![Change::SeenDone {
                occurrence: OccurrenceId(0),
                done: true,
                at: t(10)
            }]
        );
        let c = poll(&mut l, &snap(vec![obs(1, Some(DAY))]), DAY + 1, &[]);
        assert_eq!(
            decisions(&c),
            vec![(OccurrenceId(0), Outcome::Met, t(DAY + 1))]
        );

        // Seen done, then reopened before the deadline: the last
        // observation before the deadline is "open", so late.
        let mut m = Ledger::new();
        poll(&mut m, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        poll(&mut m, &snap(vec![done(obs(1, Some(DAY)), 5)]), 10, &[]);
        let c = poll(&mut m, &snap(vec![obs(1, Some(DAY))]), 20, &[]);
        assert_eq!(
            c,
            vec![Change::SeenDone {
                occurrence: OccurrenceId(0),
                done: false,
                at: t(20)
            }]
        );
        let c = poll(&mut m, &snap(vec![obs(1, Some(DAY))]), DAY + 1, &[]);
        assert_eq!(
            decisions(&c),
            vec![(OccurrenceId(0), Outcome::Late, t(DAY + 1))]
        );
    }

    #[test]
    fn app_6a_deleted_pending_task_is_never_decided() {
        // Deleted before its deadline: no decision, no debt, ever.
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        poll(&mut l, &snap(vec![]), 10, &[]);
        assert!(poll(&mut l, &snap(vec![]), DAY + 1, &[WitnessId(3)]).is_empty());
        assert!(poll(&mut l, &snap(vec![]), 9 * DAY, &[WitnessId(3)]).is_empty());
        assert_eq!(l.occurrences().outcome, vec![Outcome::Pending]);
        assert!(l.debts().occurrence.is_empty());
        assert_eq!(l.events().kind, vec![EventKind::Deleted]);

        // Gone in the same poll that passes the deadline: not decided.
        let mut g = Ledger::new();
        poll(&mut g, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        let c = poll(&mut g, &snap(vec![]), DAY + 1, &[]);
        assert!(decisions(&c).is_empty());
        assert_eq!(g.events().kind, vec![EventKind::Deleted]);
    }

    #[test]
    fn app_6a_task_back_after_deadline_is_decided_then() {
        // An archived project brought back: decided at the first poll
        // that sees it again, by APP-9.
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        poll(&mut l, &snap(vec![]), 10, &[]);
        assert!(decisions(&poll(&mut l, &snap(vec![]), DAY + 1, &[])).is_empty());
        let c = poll(&mut l, &snap(vec![obs(1, Some(DAY))]), DAY + 2, &[]);
        assert_eq!(
            decisions(&c),
            vec![(OccurrenceId(0), Outcome::Late, t(DAY + 2))]
        );

        // Seen done before it went: met when it comes back.
        let mut m = Ledger::new();
        poll(&mut m, &snap(vec![done(obs(1, Some(DAY)), 5)]), 10, &[]);
        poll(&mut m, &snap(vec![]), 20, &[]);
        let c = poll(&mut m, &snap(vec![done(obs(1, Some(DAY)), 5)]), DAY + 1, &[]);
        assert_eq!(
            decisions(&c),
            vec![(OccurrenceId(0), Outcome::Met, t(DAY + 1))]
        );
    }

    #[test]
    fn app_9_moved_earlier_past_done_observation() {
        // Seen done at DAY, then reopened and moved earlier to DAY/2: the
        // only observation before DAY/2 (at 0) showed it open, so late.
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(2 * DAY))]), 0, &[]);
        poll(
            &mut l,
            &snap(vec![done(obs(1, Some(2 * DAY)), DAY)]),
            DAY,
            &[],
        );
        let c = poll(&mut l, &snap(vec![obs(1, Some(DAY / 2))]), DAY + 10, &[]);
        assert_eq!(
            decisions(&c),
            vec![(OccurrenceId(0), Outcome::Late, t(DAY + 10))]
        );
        assert_eq!(l.occurrences().deadline, vec![t(DAY / 2)]);

        // Seen done at 10, reopened at DAY, then moved earlier to DAY/2:
        // the last observation before DAY/2 (at 10) showed it done, so met.
        let mut m = Ledger::new();
        poll(&mut m, &snap(vec![obs(1, Some(2 * DAY))]), 0, &[]);
        poll(&mut m, &snap(vec![done(obs(1, Some(2 * DAY)), 5)]), 10, &[]);
        poll(&mut m, &snap(vec![obs(1, Some(2 * DAY))]), DAY, &[]);
        let c = poll(&mut m, &snap(vec![obs(1, Some(DAY / 2))]), DAY + 10, &[]);
        assert_eq!(
            decisions(&c),
            vec![(OccurrenceId(0), Outcome::Met, t(DAY + 10))]
        );
    }

    #[test]
    fn app_9_decided_on_first_poll_after_deadline_even_if_late_poll() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        // At the deadline itself: not yet past.
        assert!(poll(&mut l, &snap(vec![obs(1, Some(DAY))]), DAY, &[]).is_empty());
        // Next successful poll is ten days later (server or Vikunja down).
        let c = poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 10 * DAY, &[]);
        assert_eq!(
            decisions(&c),
            vec![(OccurrenceId(0), Outcome::Late, t(10 * DAY))]
        );
    }

    #[test]
    fn app_10_decision_never_revised() {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), DAY + 1, &[]);
        assert_eq!(l.occurrences().outcome, vec![Outcome::Late]);
        // Vikunja later claims it was done before the deadline.
        let c = poll(
            &mut l,
            &snap(vec![done(obs(1, Some(DAY)), DAY - 1)]),
            DAY + 2,
            &[],
        );
        assert!(decisions(&c).is_empty());
        assert!(!c.iter().any(|c| matches!(c, Change::SeenDone { .. })));
        assert_eq!(l.occurrences().outcome, vec![Outcome::Late]);
        // And a met one stays met when reopened and moved.
        let mut m = Ledger::new();
        poll(&mut m, &snap(vec![obs(1, Some(DAY))]), 0, &[]);
        poll(
            &mut m,
            &snap(vec![done(obs(1, Some(DAY)), 5)]),
            DAY + 1,
            &[],
        );
        let c = poll(&mut m, &snap(vec![obs(1, Some(DAY - 5))]), DAY + 2, &[]);
        assert!(decisions(&c).is_empty());
        assert_eq!(m.occurrences().outcome, vec![Outcome::Met]);
        assert_eq!(m.occurrences().deadline, vec![t(DAY)]);
    }

    // ledger-05
    const A: WitnessId = WitnessId(100);
    const B: WitnessId = WitnessId(200);

    /// Put `amount` in force at `now` with the given real witnesses.
    fn set_amount(l: &mut Ledger, amount: u32, now: i64, witnesses: &[WitnessId]) {
        let p = l.propose_amount(Centimes(amount), t(now)).unwrap();
        l.apply(&p);
        for &w in witnesses {
            let c = l.approve_amount(w, witnesses, t(now)).unwrap();
            apply_all(l, &c);
        }
        assert_eq!(l.amount(), Centimes(amount));
    }

    /// One late debt for task 1, deadline `DAY`, on an empty ledger.
    fn one_late_debt(witnesses: &[WitnessId]) -> (Ledger, Vec<Change>) {
        let mut l = Ledger::new();
        poll(&mut l, &snap(vec![obs(1, Some(DAY))]), 0, witnesses);
        let c = poll(&mut l, &snap(vec![obs(1, Some(DAY))]), DAY + 1, witnesses);
        (l, c)
    }

    #[test]
    fn app_11_late_creates_one_debt_with_amount_in_force() {
        let (l, c) = one_late_debt(&[]);
        assert_eq!(
            c,
            vec![
                Change::Decided {
                    occurrence: OccurrenceId(0),
                    outcome: Outcome::Late,
                    at: t(DAY + 1)
                },
                Change::DebtCreated {
                    debt: DebtId(0),
                    occurrence: OccurrenceId(0),
                    amount: Centimes(1000),
                    at: t(DAY + 1),
                },
            ]
        );
        let d = l.debts();
        assert_eq!(d.occurrence, vec![OccurrenceId(0)]);
        assert_eq!(d.amount, vec![Centimes(1000)]);
        assert_eq!(d.created_at, vec![t(DAY + 1)]);
        assert_eq!(d.owner, vec![None]);
        assert_eq!(d.sent_at, vec![None]);
        assert_eq!(d.received_at, vec![None]);

        // The amount in force at the deadline, not at the decision.
        let mut m = Ledger::new();
        poll(&mut m, &snap(vec![obs(1, Some(DAY))]), 0, &[A]);
        set_amount(&mut m, 2000, 10, &[A]);
        poll(&mut m, &snap(vec![obs(1, Some(DAY))]), DAY + 1, &[A]);
        set_amount(&mut m, 3000, DAY + 2, &[A]);
        assert_eq!(m.debts().amount, vec![Centimes(2000)]);
    }

    #[test]
    fn app_12_moving_deadline_never_creates_second_debt() {
        let (mut l, _) = one_late_debt(&[]);
        poll(&mut l, &snap(vec![obs(1, Some(DAY / 2))]), DAY + 2, &[]);
        poll(&mut l, &snap(vec![obs(1, Some(5 * DAY))]), DAY + 3, &[]);
        poll(&mut l, &snap(vec![obs(1, None)]), 6 * DAY, &[]);
        poll(&mut l, &snap(vec![]), 7 * DAY, &[]);
        poll(&mut l, &snap(vec![obs(1, Some(8 * DAY))]), 9 * DAY, &[]);
        assert_eq!(l.debts().occurrence.len(), 1);
        assert_eq!(l.occurrences().task.len(), 1);
        assert_eq!(l.events().kind.len(), 5);
    }

    #[test]
    fn app_13_no_operation_removes_or_changes_debt() {
        let (mut l, _) = one_late_debt(&[A, B]);
        let before = l.debts().clone();
        set_amount(&mut l, 5000, DAY + 2, &[A, B]);
        poll(&mut l, &snap(vec![]), DAY + 3, &[A, B]);
        poll(&mut l, &snap(vec![obs(1, Some(3 * DAY))]), DAY + 4, &[A, B]);
        assert_eq!(l.debts(), &before);
        assert_eq!(l.occurrences().deadline, vec![t(DAY)]);
        assert_eq!(l.events().kind.len(), 2);
    }

    #[test]
    fn app_15_default_amount_is_10_chf() {
        assert_eq!(Ledger::new().amount(), Centimes(1000));
        assert_eq!(Ledger::new().amount(), Centimes::DEFAULT_AMOUNT);
        assert_eq!(Ledger::default().amount(), Centimes::DEFAULT_AMOUNT);
        assert!(Ledger::new().proposal().is_none());
    }

    #[test]
    fn app_16_amount_needs_every_real_witness() {
        let w = [A, B];
        let mut l = Ledger::new();
        assert_eq!(l.approve_amount(A, &w, t(1)), Err(RuleError::NoProposal));
        assert_eq!(
            l.propose_amount(Centimes(0), t(1)),
            Err(RuleError::InvalidAmount)
        );
        assert_eq!(
            l.propose_amount(Centimes(1000), t(1)),
            Err(RuleError::InvalidAmount)
        );

        let p = l.propose_amount(Centimes(1500), t(2)).unwrap();
        assert_eq!(
            p,
            Change::AmountProposed {
                amount: Centimes(1500),
                at: t(2)
            }
        );
        l.apply(&p);
        assert_eq!(
            l.approve_amount(WitnessId(999), &w, t(3)),
            Err(RuleError::NotRealWitness)
        );

        let c = l.approve_amount(A, &w, t(3)).unwrap();
        assert_eq!(
            c,
            vec![Change::AmountApproved {
                witness: A,
                at: t(3)
            }]
        );
        apply_all(&mut l, &c);
        assert_eq!(l.amount(), Centimes(1000));
        assert_eq!(
            l.approve_amount(A, &w, t(4)),
            Err(RuleError::AlreadyApproved)
        );
        assert_eq!(
            l.proposal(),
            Some(&Proposal {
                amount: Centimes(1500),
                proposed_at: t(2),
                approvals: vec![A]
            })
        );

        // A new proposal replaces the old one and clears its approvals.
        let p = l.propose_amount(Centimes(1200), t(5)).unwrap();
        l.apply(&p);
        assert!(l.proposal().unwrap().approvals.is_empty());
        let c = l.approve_amount(A, &w, t(6)).unwrap();
        apply_all(&mut l, &c);
        let c = l.approve_amount(B, &w, t(7)).unwrap();
        assert_eq!(
            c,
            vec![
                Change::AmountApproved {
                    witness: B,
                    at: t(7)
                },
                Change::AmountInForce {
                    amount: Centimes(1200),
                    at: t(7)
                },
            ]
        );
        apply_all(&mut l, &c);
        assert_eq!(l.amount(), Centimes(1200));
        assert!(l.proposal().is_none());
    }

    #[test]
    fn app_16_in_force_when_last_missing_witness_leaves() {
        let mut l = Ledger::new();
        l.apply(&l.propose_amount(Centimes(2000), t(1)).unwrap());
        let c = l.approve_amount(A, &[A, B], t(2)).unwrap();
        apply_all(&mut l, &c);
        // B leaves: every current real witness has approved.
        let c = l.approve_amount(A, &[A], t(3)).unwrap();
        assert_eq!(
            c,
            vec![Change::AmountInForce {
                amount: Centimes(2000),
                at: t(3)
            }]
        );
        apply_all(&mut l, &c);
        assert_eq!(l.amount(), Centimes(2000));
        assert!(l.proposal().is_none());
        // Without B leaving, a repeat approval is still refused.
        l.apply(&l.propose_amount(Centimes(1500), t(4)).unwrap());
        let c = l.approve_amount(A, &[A, B], t(5)).unwrap();
        apply_all(&mut l, &c);
        assert_eq!(
            l.approve_amount(A, &[A, B], t(6)),
            Err(RuleError::AlreadyApproved)
        );
    }

    #[test]
    fn app_16_new_amount_not_retroactive() {
        let w = [A];
        let mut l = Ledger::new();
        poll(
            &mut l,
            &snap(vec![obs(1, Some(DAY)), obs(2, Some(3 * DAY))]),
            0,
            &w,
        );
        set_amount(&mut l, 2500, 2 * DAY, &w);
        // Both decided late after the change; only task 2's deadline is
        // after the moment the new amount came into force.
        poll(
            &mut l,
            &snap(vec![obs(1, Some(DAY)), obs(2, Some(3 * DAY))]),
            4 * DAY,
            &w,
        );
        assert_eq!(l.debts().occurrence, vec![OccurrenceId(0), OccurrenceId(1)]);
        assert_eq!(l.debts().amount, vec![Centimes(1000), Centimes(2500)]);
    }

    // ledger-06
    const C: WitnessId = WitnessId(300);
    /// The admin witness's account id. Callers never pass it as real.
    const ADMIN: WitnessId = WitnessId(1);

    fn claimed(l: &mut Ledger, debt: u32, w: WitnessId, real: &[WitnessId], now: i64) {
        let c = l.claim(DebtId(debt), w, real, t(now)).unwrap();
        l.apply(&c);
    }

    #[test]
    fn app_17_single_witness_auto_claims() {
        let (l, c) = one_late_debt(&[A]);
        assert_eq!(
            c.last(),
            Some(&Change::Claimed {
                debt: DebtId(0),
                witness: A,
                automatic: true,
                at: t(DAY + 1)
            })
        );
        assert_eq!(l.debts().owner, vec![Some(A)]);
    }

    #[test]
    fn app_18_zero_witnesses_leaves_claimable() {
        for real in [&[][..], &[A, B][..]] {
            let (mut l, c) = one_late_debt(real);
            assert!(!c.iter().any(|c| matches!(c, Change::Claimed { .. })));
            assert_eq!(l.debts().owner, vec![None]);
            claimed(&mut l, 0, A, &[A, B], DAY + 2);
            assert_eq!(l.debts().owner, vec![Some(A)]);
        }
    }

    #[test]
    fn app_18_first_claim_wins_second_rejected() {
        let w = [A, B];
        let (mut l, _) = one_late_debt(&w);
        let c = l.claim(DebtId(0), A, &w, t(DAY + 2)).unwrap();
        assert_eq!(
            c,
            Change::Claimed {
                debt: DebtId(0),
                witness: A,
                automatic: false,
                at: t(DAY + 2)
            }
        );
        l.apply(&c);
        assert_eq!(
            l.claim(DebtId(0), B, &w, t(DAY + 3)),
            Err(RuleError::AlreadyClaimed)
        );
        assert_eq!(
            l.claim(DebtId(5), B, &w, t(DAY + 3)),
            Err(RuleError::UnknownDebt)
        );
        assert_eq!(l.debts().owner, vec![Some(A)]);
    }

    #[test]
    fn app_19_unclaimed_counts_in_owed() {
        let (l, _) = one_late_debt(&[A, B]);
        assert_eq!(
            l.totals(),
            Totals {
                owed: Centimes(1000),
                sent: Centimes(0),
                confirmed: Centimes(0)
            }
        );
    }

    #[test]
    fn app_19_late_joiner_can_claim_old_debt() {
        let (mut l, _) = one_late_debt(&[A, B]);
        // C joins a year later; the debt is still claimable.
        assert_eq!(
            l.claim(DebtId(0), C, &[A, B], t(400 * DAY)),
            Err(RuleError::NotRealWitness)
        );
        claimed(&mut l, 0, C, &[A, B, C], 400 * DAY);
        assert_eq!(l.debts().owner, vec![Some(C)]);
    }

    #[test]
    fn app_20_admin_not_counted_as_real_witness() {
        // The caller passes real witnesses only: A plus the admin is one.
        let (mut l, _) = one_late_debt(&[A]);
        assert_eq!(l.debts().owner, vec![Some(A)]);
        assert_eq!(
            l.claim(DebtId(0), ADMIN, &[A], t(DAY + 2)),
            Err(RuleError::NotRealWitness)
        );
        l.apply(&l.mark_sent(DebtId(0), t(DAY + 3)).unwrap());
        assert_eq!(
            l.mark_received(DebtId(0), ADMIN, t(DAY + 4)),
            Err(RuleError::NotOwner)
        );
        let p = l.propose_amount(Centimes(2000), t(DAY + 5)).unwrap();
        l.apply(&p);
        assert_eq!(
            l.approve_amount(ADMIN, &[A], t(DAY + 6)),
            Err(RuleError::NotRealWitness)
        );
    }

    #[test]
    fn app_21_only_claimed_debt_can_be_marked_sent() {
        let w = [A, B];
        let (mut l, _) = one_late_debt(&w);
        assert_eq!(
            l.mark_sent(DebtId(0), t(DAY + 2)),
            Err(RuleError::NotClaimed)
        );
        assert_eq!(
            l.mark_sent(DebtId(1), t(DAY + 2)),
            Err(RuleError::UnknownDebt)
        );
        claimed(&mut l, 0, B, &w, DAY + 3);
        let c = l.mark_sent(DebtId(0), t(DAY + 4)).unwrap();
        assert_eq!(
            c,
            Change::MarkedSent {
                debt: DebtId(0),
                at: t(DAY + 4)
            }
        );
        l.apply(&c);
        assert_eq!(l.debts().sent_at, vec![Some(t(DAY + 4))]);
    }

    #[test]
    fn app_22_received_only_by_owner_after_sent() {
        let w = [A, B];
        let (mut l, _) = one_late_debt(&w);
        assert_eq!(
            l.mark_received(DebtId(0), A, t(DAY + 2)),
            Err(RuleError::NotOwner)
        );
        assert_eq!(
            l.mark_received(DebtId(9), A, t(DAY + 2)),
            Err(RuleError::UnknownDebt)
        );
        claimed(&mut l, 0, A, &w, DAY + 3);
        assert_eq!(
            l.mark_received(DebtId(0), A, t(DAY + 4)),
            Err(RuleError::NotSent)
        );
        l.apply(&l.mark_sent(DebtId(0), t(DAY + 5)).unwrap());
        assert_eq!(
            l.mark_received(DebtId(0), B, t(DAY + 6)),
            Err(RuleError::NotOwner)
        );
        let c = l.mark_received(DebtId(0), A, t(DAY + 7)).unwrap();
        assert_eq!(
            c,
            Change::MarkedReceived {
                debt: DebtId(0),
                at: t(DAY + 7)
            }
        );
        l.apply(&c);
        assert_eq!(l.debts().received_at, vec![Some(t(DAY + 7))]);
    }

    #[test]
    fn app_23_marks_cannot_be_unset() {
        let (mut l, _) = one_late_debt(&[A]);
        l.apply(&l.mark_sent(DebtId(0), t(DAY + 2)).unwrap());
        assert_eq!(
            l.mark_sent(DebtId(0), t(DAY + 3)),
            Err(RuleError::AlreadyMarked)
        );
        l.apply(&l.mark_received(DebtId(0), A, t(DAY + 4)).unwrap());
        assert_eq!(
            l.mark_received(DebtId(0), A, t(DAY + 5)),
            Err(RuleError::AlreadyMarked)
        );
        // Later polls never touch the marks either.
        poll(&mut l, &snap(vec![]), DAY + 6, &[A]);
        assert_eq!(l.debts().sent_at, vec![Some(t(DAY + 2))]);
        assert_eq!(l.debts().received_at, vec![Some(t(DAY + 4))]);
    }

    #[test]
    fn app_24_totals_owed_sent_confirmed() {
        let w = [A, B];
        let mut l = Ledger::new();
        let tasks = || {
            snap(vec![
                obs(1, Some(DAY)),
                obs(2, Some(DAY)),
                obs(3, Some(3 * DAY)),
            ])
        };
        poll(&mut l, &tasks(), 0, &w);
        set_amount(&mut l, 2500, 2 * DAY, &w);
        poll(&mut l, &tasks(), 4 * DAY, &w);
        assert_eq!(
            l.debts().amount,
            vec![Centimes(1000), Centimes(1000), Centimes(2500)]
        );
        assert_eq!(
            l.totals(),
            Totals {
                owed: Centimes(4500),
                ..Totals::default()
            }
        );
        // Debt 0 stays unclaimed; debt 1 is sent; debt 2 is received.
        claimed(&mut l, 1, A, &w, 5 * DAY);
        claimed(&mut l, 2, B, &w, 5 * DAY);
        l.apply(&l.mark_sent(DebtId(1), t(6 * DAY)).unwrap());
        l.apply(&l.mark_sent(DebtId(2), t(6 * DAY)).unwrap());
        l.apply(&l.mark_received(DebtId(2), B, t(7 * DAY)).unwrap());
        assert_eq!(
            l.totals(),
            Totals {
                owed: Centimes(4500),
                sent: Centimes(3500),
                confirmed: Centimes(2500)
            }
        );
    }

    #[test]
    fn ledger_double_claim_same_witness_is_rejected_not_duplicated() {
        let w = [A, B];
        let (mut l, _) = one_late_debt(&w);
        claimed(&mut l, 0, A, &w, DAY + 2);
        assert_eq!(
            l.claim(DebtId(0), A, &w, t(DAY + 3)),
            Err(RuleError::AlreadyClaimed)
        );
        assert_eq!(l.debts().owner, vec![Some(A)]);
        // Auto-claimed debts reject a manual claim by the same witness too.
        let (l, _) = one_late_debt(&[A]);
        assert_eq!(
            l.claim(DebtId(0), A, &[A], t(DAY + 2)),
            Err(RuleError::AlreadyClaimed)
        );
    }

    // DB-3, Review Focus 4
    #[test]
    fn db_3_replaying_changes_rebuilds_same_state() {
        let wit = [A, B];
        let mut live = Ledger::new();
        let mut log: Vec<Change> = Vec::new();
        let mut run = |live: &mut Ledger, c: Vec<Change>| {
            apply_all(live, &c);
            log.extend(c);
        };
        let step = live.observe(
            &snap(vec![
                obs(1, Some(DAY)),
                daily(2, DAY, None),
                obs(3, Some(2 * DAY)),
            ]),
            t(0),
            &wit,
        );
        run(&mut live, step);
        let step = live.observe(
            &snap(vec![
                done(obs(1, Some(DAY)), 10),
                daily(2, DAY, None),
                obs(3, Some(DAY / 2)),
            ]),
            t(20),
            &wit,
        );
        run(&mut live, step);
        let step = vec![live.propose_amount(Centimes(1800), t(30)).unwrap()];
        run(&mut live, step);
        let step = live.approve_amount(A, &wit, t(40)).unwrap();
        run(&mut live, step);
        let step = live.observe(
            &snap(vec![daily(2, 2 * DAY, Some(DAY + HOUR))]),
            t(DAY + 2 * HOUR),
            &wit,
        );
        run(&mut live, step);
        let step = vec![live.claim(DebtId(0), B, &wit, t(DAY + 3 * HOUR)).unwrap()];
        run(&mut live, step);
        let step = vec![live.mark_sent(DebtId(0), t(DAY + 4 * HOUR)).unwrap()];
        run(&mut live, step);
        let step = vec![live.mark_received(DebtId(0), B, t(DAY + 5 * HOUR)).unwrap()];
        run(&mut live, step);
        assert!(log.iter().any(|c| matches!(c, Change::DebtCreated { .. })));
        assert!(log.iter().any(|c| matches!(c, Change::Event { .. })));
        assert!(log.iter().any(|c| matches!(c, Change::SeenDone { .. })));

        let mut rebuilt = Ledger::new();
        apply_all(&mut rebuilt, &log);
        assert_eq!(format!("{rebuilt:?}"), format!("{live:?}"));
        assert_eq!(rebuilt.occurrences(), live.occurrences());
        assert_eq!(rebuilt.events(), live.events());
        assert_eq!(rebuilt.debts(), live.debts());
        assert_eq!(rebuilt.totals(), live.totals());
        assert_eq!(rebuilt.amount(), live.amount());
        assert_eq!(rebuilt.proposal(), live.proposal());
        // After the restart both decide the next poll the same way, so
        // deadlines that passed while down are still decided (DB-3).
        let next = snap(vec![daily(2, 2 * DAY, Some(DAY + HOUR))]);
        assert_eq!(
            rebuilt.observe(&next, t(3 * DAY), &wit),
            live.observe(&next, t(3 * DAY), &wit)
        );
        assert!(!rebuilt.observe(&next, t(3 * DAY), &wit).is_empty());
    }
}
