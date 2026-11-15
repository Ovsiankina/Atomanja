//! Fills the dev database with fake tasks and debts covering every
//! debt state, built through the real ledger rules.
//!
//! Uses: `ledger`, `auth`, `store`.
//! SRS: DEV-2.
//!
//! # Contract shape: one seed function on a dev handle
//!
//! [`seed`] takes a [`Store<DevDb>`], so it cannot write the ledger file
//! (DEV-3 by type). It creates fake real-witness accounts (role
//! `Witness`, hashed through `auth`, in the dev file only), then drives
//! `Ledger::observe` with fake snapshots around `now` and the ledger's
//! own `claim` / `mark_sent` / `mark_received` operations, appending each
//! batch through [`Store::append`] before applying it. The result holds
//! at least one debt in each state: claimable, claimed, sent, received
//! (plus met occurrences and deadline events, so the witness view has
//! every row kind). The admin account itself is not created here: that
//! is `create-admin` in the CLI (APP-30).
//!
//! [`Store<DevDb>`]: crate::store::Store
//! [`Store::append`]: crate::store::Store::append

use crate::auth::{AuthError, PasswordHash, Role, hash_password};
use crate::ledger::{
    Change, DebtId, Ledger, Observation, RepeatMode, RuleError, Snapshot, TaskId, Timestamp,
    WitnessId,
};
use crate::store::{DevDb, Store, StoreError};

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

/// Fake real witnesses created in the dev file. Two of them, so late
/// debts stay claimable (APP-18) instead of being auto-claimed (APP-17).
const FAKE_WITNESSES: [&str; 2] = ["witness1@dev.invalid", "witness2@dev.invalid"];

/// The password of every fake account. Dev data only.
const FAKE_PASSWORD: &str = "dev-witness-password";

/// Fill `store` with fake data covering every debt state (DEV-2), all
/// timestamps derived from `now` so tests are deterministic. Only a
/// fresh dev file is seeded: a file that already holds ledger changes
/// is refused, so running `seed-dev` twice never doubles the data.
/// Accounts already present (the admin) do not count as data.
///
/// # Errors
///
/// [`DevdataError::AlreadySeeded`] when the file holds ledger changes;
/// [`DevdataError::Store`] when a write fails (each batch is one
/// transaction); [`DevdataError::Auth`] when a fake password cannot be
/// hashed.
pub fn seed(store: &mut Store<DevDb>, now: Timestamp) -> Result<(), DevdataError> {
    if !store
        .load_changes()
        .map_err(DevdataError::Store)?
        .is_empty()
    {
        return Err(DevdataError::AlreadySeeded);
    }
    let witnesses = fake_witnesses(store)?;
    let (a, b) = (witnesses[0], witnesses[1]);
    let real = store.real_witnesses().map_err(DevdataError::Store)?;
    let mut ledger = store.load_ledger().map_err(DevdataError::Store)?;
    let at = |offset: i64| Timestamp(now.0 + offset);

    // DECISION: one fixed script of four polls; each task id plays one
    // row kind, so the witness view shows every state:
    //  1 late, claimable          6 moved later, late, claimable
    //  2 late, claimed            7 moved earlier, late, claimable
    //  3 late, claimed, sent      8 upcoming
    //  4 late, claimed, sent,     9 due date removed (pending)
    //    received                10 deleted
    //  5 done in time (met)
    let first = vec![
        task(1, Some(at(-9 * DAY))),
        task(2, Some(at(-8 * DAY))),
        task(3, Some(at(-7 * DAY))),
        task(4, Some(at(-6 * DAY))),
        task(5, Some(at(-5 * DAY))),
        task(6, Some(at(-4 * DAY))),
        task(7, Some(at(-3 * DAY))),
        task(8, Some(at(5 * DAY))),
        task(9, Some(at(2 * DAY))),
        task(10, Some(at(DAY))),
    ];
    poll(store, &mut ledger, first.clone(), at(-10 * DAY), &real)?;

    // Deadline events: 6 later, 7 earlier, 9 removed, 10 gone.
    let mut moved: Vec<Observation> = first.into_iter().filter(|o| o.task.0 != 10).collect();
    for o in &mut moved {
        match o.task.0 {
            6 => o.due = Some(at(3 * DAY)),
            7 => o.due = Some(at(-5 * DAY)),
            9 => o.due = None,
            _ => {}
        }
    }
    poll(
        store,
        &mut ledger,
        moved.clone(),
        at(-9 * DAY - 12 * HOUR),
        &real,
    )?;

    // Task 5 marked done before its deadline, seen by a poll.
    let mut done = moved;
    for o in done.iter_mut().filter(|o| o.task.0 == 5) {
        o.done = true;
        o.done_at = Some(at(-5 * DAY - 3 * HOUR));
    }
    poll(
        store,
        &mut ledger,
        done.clone(),
        at(-5 * DAY - 2 * HOUR),
        &real,
    )?;

    // First poll after every past deadline: met and late decisions, debts.
    poll(store, &mut ledger, done, at(-2 * HOUR), &real)?;

    let steps: [(i64, i64, Step); 6] = [
        (2, -90, Step::Claim(a)),
        (3, -80, Step::Claim(b)),
        (4, -70, Step::Claim(a)),
        (3, -60, Step::Sent),
        (4, -50, Step::Sent),
        (4, -40, Step::Received(a)),
    ];
    for (task_id, minutes, step) in steps {
        let debt = debt_of(&ledger, TaskId(task_id))?;
        let when = at(minutes * 60);
        let change = match step {
            Step::Claim(w) => ledger.claim(debt, w, &real, when),
            Step::Sent => ledger.mark_sent(debt, when),
            Step::Received(w) => ledger.mark_received(debt, w, when),
        }
        .map_err(refused)?;
        commit(store, &mut ledger, &[change])?;
    }
    Ok(())
}

/// A debt operation of the seed script.
#[derive(Clone, Copy)]
enum Step {
    Claim(WitnessId),
    Sent,
    Received(WitnessId),
}

/// The fake real-witness accounts, created if absent, as ledger ids.
fn fake_witnesses(store: &mut Store<DevDb>) -> Result<Vec<WitnessId>, DevdataError> {
    let mut hash: Option<PasswordHash> = None;
    let mut out = Vec::with_capacity(FAKE_WITNESSES.len());
    for email in FAKE_WITNESSES {
        // DECISION: an account left by an earlier seed that failed after
        // creating it is reused, so a retry is not refused for its email.
        if let Some(account) = store.account_by_email(email).map_err(DevdataError::Store)? {
            out.push(WitnessId(account.id.0));
            continue;
        }
        let hash = match &hash {
            Some(h) => h,
            None => hash.insert(hash_password(FAKE_PASSWORD).map_err(DevdataError::Auth)?),
        };
        let account = store
            .insert_account(email, hash, Role::Witness)
            .map_err(DevdataError::Store)?;
        out.push(WitnessId(account.id.0));
    }
    Ok(out)
}

/// A visible, non-recurring, not-done fake task.
fn task(id: i64, due: Option<Timestamp>) -> Observation {
    Observation {
        due,
        done_at: None,
        task: TaskId(id),
        repeat_after_secs: 0,
        repeat_mode: RepeatMode::Interval,
        done: false,
        archived: false,
    }
}

/// One fake poll through `Ledger::observe`, persisted then applied.
fn poll(
    store: &mut Store<DevDb>,
    ledger: &mut Ledger,
    tasks: Vec<Observation>,
    now: Timestamp,
    real: &[WitnessId],
) -> Result<(), DevdataError> {
    let changes = ledger.observe(&Snapshot { tasks }, now, real);
    commit(store, ledger, &changes)
}

/// Persist one batch (one transaction), then apply it: memory never runs
/// ahead of the file.
fn commit(
    store: &mut Store<DevDb>,
    ledger: &mut Ledger,
    changes: &[Change],
) -> Result<(), DevdataError> {
    store.append(changes).map_err(DevdataError::Store)?;
    for change in changes {
        ledger.apply(change);
    }
    Ok(())
}

/// The debt of the late occurrence of `task`.
fn debt_of(ledger: &Ledger, task: TaskId) -> Result<DebtId, DevdataError> {
    let tasks = &ledger.occurrences().task;
    ledger
        .debts()
        .occurrence
        .iter()
        .position(|occ| tasks.get(occ.0 as usize) == Some(&task))
        .and_then(|i| u32::try_from(i).ok())
        .map(DebtId)
        .ok_or_else(|| mismatch(&format!("no debt for task #{}", task.0)))
}

// DECISION: the contract has no rule-error variant. A refusal can only
// mean the fixed script disagrees with the ledger it just built from the
// file, so it is reported as a store mismatch (`Corrupt`), not a panic.
fn refused(err: RuleError) -> DevdataError {
    mismatch(&format!("ledger refused a seed step: {err:?}"))
}

/// The dev file does not hold what the seed script wrote.
fn mismatch(what: &str) -> DevdataError {
    DevdataError::Store(StoreError::Corrupt(what.to_owned()))
}

/// Why the dev database was not seeded.
#[derive(Debug)]
pub enum DevdataError {
    /// The dev file already holds ledger changes.
    AlreadySeeded,
    /// A store read or write failed.
    Store(StoreError),
    /// A fake account's password could not be hashed.
    Auth(AuthError),
}

impl std::fmt::Display for DevdataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DevdataError::AlreadySeeded => f.write_str("the dev database is already seeded"),
            DevdataError::Store(e) => write!(f, "dev database: {e}"),
            DevdataError::Auth(e) => write!(f, "fake account: {e}"),
        }
    }
}

impl std::error::Error for DevdataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DevdataError::AlreadySeeded => None,
            DevdataError::Store(e) => Some(e),
            DevdataError::Auth(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Database, Role};
    use crate::ledger::{EventKind, Outcome};
    use crate::store::LedgerDb;

    const NOW: Timestamp = Timestamp(1_800_000_000);

    /// A unique path in the system temp dir, removed on drop.
    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new(tag: &str) -> TempFile {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            TempFile(std::env::temp_dir().join(format!(
                "atomanja-devdata-{tag}-{}-{nanos}.sqlite",
                std::process::id()
            )))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    // devdata-01
    #[test]
    fn dev_2_seed_covers_claimable_claimed_sent_received() {
        let mut store = Store::<DevDb>::open_in_memory().unwrap();
        seed(&mut store, NOW).unwrap();
        let ledger = store.load_ledger().unwrap();

        let d = ledger.debts();
        let rows = 0..d.occurrence.len();
        let claimable = rows.clone().any(|i| d.owner[i].is_none());
        let claimed = rows
            .clone()
            .any(|i| d.owner[i].is_some() && d.sent_at[i].is_none());
        let sent = rows
            .clone()
            .any(|i| d.sent_at[i].is_some() && d.received_at[i].is_none());
        let received = rows.clone().any(|i| d.received_at[i].is_some());
        assert!(claimable, "no claimable debt");
        assert!(claimed, "no claimed debt");
        assert!(sent, "no sent debt");
        assert!(received, "no received debt");

        let outcomes = &ledger.occurrences().outcome;
        assert!(outcomes.contains(&Outcome::Met), "no met occurrence");
        assert!(outcomes.contains(&Outcome::Pending), "no upcoming deadline");
        assert!(
            ledger
                .occurrences()
                .deadline
                .iter()
                .zip(outcomes)
                .any(|(&dl, &o)| o == Outcome::Pending && dl > NOW)
        );

        let kinds = &ledger.events().kind;
        for k in [
            EventKind::MovedLater,
            EventKind::MovedEarlier,
            EventKind::Removed,
            EventKind::Deleted,
        ] {
            assert!(kinds.contains(&k), "no {k:?} event");
        }

        // Every owner is a fake real witness, never the admin (APP-20).
        let witnesses = store.real_witnesses().unwrap();
        assert!(witnesses.len() >= 2);
        assert!(d.owner.iter().flatten().all(|w| witnesses.contains(w)));
        // Every timestamp is derived from `now`, none in the future
        // except recorded upcoming deadlines.
        assert!(ledger.events().at.iter().all(|&t| t <= NOW));
    }

    #[test]
    fn dev_2_seed_writes_only_dev_db() {
        let dev_path = TempFile::new("dev");
        let ledger_path = TempFile::new("ledger");
        let ledger_store = Store::<LedgerDb>::open(&ledger_path.0).unwrap();
        let mut dev = Store::<DevDb>::open(&dev_path.0).unwrap();
        assert_eq!(dev.database(), Database::Dev);

        seed(&mut dev, NOW).unwrap();

        assert!(!dev.load_changes().unwrap().is_empty());
        assert!(
            dev.accounts()
                .unwrap()
                .iter()
                .all(|a| a.role == Role::Witness)
        );
        assert!(ledger_store.load_changes().unwrap().is_empty());
        assert!(ledger_store.accounts().unwrap().is_empty());
        // Reopened from disk, the ledger file is still empty.
        drop(ledger_store);
        let reopened = Store::<LedgerDb>::open(&ledger_path.0).unwrap();
        assert!(reopened.load_changes().unwrap().is_empty());
    }

    #[test]
    fn dev_2_seed_twice_is_refused_and_admin_does_not_count() {
        let mut store = Store::<DevDb>::open_in_memory().unwrap();
        let hash = crate::auth::PasswordHash("$argon2id$fake".into());
        store
            .insert_account("admin@example.invalid", &hash, Role::Admin)
            .unwrap();
        seed(&mut store, NOW).unwrap();
        let before = store.load_changes().unwrap().len();
        assert!(matches!(
            seed(&mut store, NOW),
            Err(DevdataError::AlreadySeeded)
        ));
        assert_eq!(store.load_changes().unwrap().len(), before);
    }
}
