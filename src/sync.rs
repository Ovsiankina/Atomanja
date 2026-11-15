//! The poll loop: fetch from Vikunja, apply to the ledger, persist,
//! notify. Runs once at startup, then every poll interval.
//!
//! Uses: `config`, `vikunja`, `ledger`, `store`, `notify`.
//! SRS: APP-2, APP-9, DB-3, SRS 5.1.
//!
//! # Contract shape: one testable step, a thin loop
//!
//! [`poll_once`] is one complete poll. Every side is a seam: the fetch
//! goes through a [`PageSource`] (the real [`vikunja::Client`]
//! or a fake serving fixture pages), persistence through an in-memory
//! or file [`Store<LedgerDb>`], and delivery through [`Push`] (the real
//! [`Notifier`] or a fake that records messages). [`run`] calls
//! [`poll_once`] at startup (DB-3) and then every interval (APP-2).
//!
//! A poll is all-or-nothing (Review Focus 2, SRS 5.1): when the fetch
//! fails, nothing is observed, persisted or sent, so no met/late
//! decision is made until the next successful poll (APP-9).
//!
//! The store is shared with `web` behind a `std::sync::Mutex`. A poll
//! takes the lock only for its synchronous part (load ledger, observe,
//! append, collect messages and subscriptions) and never holds it across
//! an `.await`. The ledger is rebuilt from the store under that lock, so
//! claims and amount changes made through `web` since the last poll are
//! always seen (no stale in-memory copy).
//!
//! Task titles are not stored (the ledger keeps none, APP-32), yet the
//! worker view shows them (APP-31). Each successful poll replaces the
//! contents of a shared `TaskId -> title` map that `web` reads. Plain
//! `std` types carry it, so `web` needs no arrow to `sync`.
//!
//! [`Store<LedgerDb>`]: crate::store::Store
//! [`vikunja::Client`]: crate::vikunja::Client

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use time::OffsetDateTime;

use crate::ledger::{Change, Observation, RepeatMode, Snapshot, TaskId, Timestamp};
use crate::notify::{self, Message, Notifier, SendReport, Subscription};
use crate::store::{LedgerDb, Store, StoreError};
use crate::vikunja::{self, PageSource, Task, VikunjaError};

/// Where notifications go: the real [`Notifier`], or a fake in tests.
pub trait Push {
    /// Deliver `message` to every device in `subscriptions` (APP-33).
    /// Never fails; failures are counted in the report (SRS 5.1).
    fn push(
        &self,
        message: &Message,
        subscriptions: &[Subscription],
    ) -> impl Future<Output = SendReport> + Send;
}

impl Push for Notifier {
    /// Delegates to [`Notifier::send`].
    fn push(
        &self,
        message: &Message,
        subscriptions: &[Subscription],
    ) -> impl Future<Output = SendReport> + Send {
        self.send(message, subscriptions)
    }
}

/// Map one Vikunja record to what the ledger observes (APP-3, APP-8).
/// `sync` owns this mapping (no `vikunja -> ledger` arrow).
fn observation(task: &Task) -> Observation {
    Observation {
        due: task.due.map(timestamp),
        done_at: task.done_at.map(timestamp),
        task: TaskId(task.id),
        // DECISION: a negative or oversized `repeat_after` is clamped
        // into u32 (0 = not recurring, or the maximum) rather than
        // failing the whole poll over one odd task.
        repeat_after_secs: u32::try_from(task.repeat_after_secs.max(0)).unwrap_or(u32::MAX),
        // DECISION: an unknown raw `repeat_mode` counts as the Vikunja
        // default (interval), which is what Vikunja itself falls back to.
        repeat_mode: match task.repeat_mode {
            1 => RepeatMode::Monthly,
            2 => RepeatMode::FromDone,
            _ => RepeatMode::Interval,
        },
        done: task.done,
        archived: task.project_archived,
    }
}

fn timestamp(at: OffsetDateTime) -> Timestamp {
    Timestamp(at.unix_timestamp())
}

/// A lock that a panicking holder poisoned is still consistent here:
/// every store write is one transaction, the title map one assignment.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Messages to send after the lock is released, each with its devices.
type Outbox = Vec<(Message, Vec<Subscription>)>;

/// The synchronous, locked part of a poll (step 2 of [`poll_once`]).
fn apply(
    store: &mut Store<LedgerDb>,
    tasks: &[Task],
    now: Timestamp,
) -> Result<(Vec<Change>, Outbox), StoreError> {
    let mut ledger = store.load_ledger()?;
    let witnesses = store.real_witnesses()?;
    // DECISION: accounts and devices are read before the append, so no
    // read can fail after the batch is written; a poll then either
    // reports its persisted changes or wrote nothing.
    let accounts = store.accounts()?;
    let mut devices = BTreeMap::new();
    for account in &accounts {
        let subs: Vec<Subscription> = store
            .subscriptions(account.id)?
            .into_iter()
            .map(|s| Subscription {
                endpoint: s.endpoint,
                p256dh: s.p256dh,
                auth: s.auth,
            })
            .collect();
        devices.insert(account.id, subs);
    }
    let snapshot = Snapshot {
        tasks: tasks.iter().map(observation).collect(),
    };
    let changes = ledger.observe(&snapshot, now, &witnesses);
    if changes.is_empty() {
        return Ok((changes, Vec::new()));
    }
    store.append(&changes)?;
    for change in &changes {
        ledger.apply(change);
    }
    let outbox = notify::messages(&changes, &ledger, &accounts)
        .into_iter()
        .map(|m| {
            let subs = devices.get(&m.to).cloned().unwrap_or_default();
            (m, subs)
        })
        .collect();
    Ok((changes, outbox))
}

// DECISION: titles travel in a std BTreeMap (ordered by task number,
// no hasher generic) rather than a sync type, so web needs no sync arrow.
/// One poll at time `now` (APP-9, DB-3, SRS 5.1):
///
/// 1. fetch every task through `source` ([`vikunja::fetch_all`]);
///    on failure return [`SyncError::Fetch`] having changed nothing;
/// 2. under the `store` lock: rebuild the ledger
///    ([`Store::load_ledger`]), map the tasks to a `ledger::Snapshot`,
///    run `Ledger::observe` with the store's real witnesses, append the
///    changes in one transaction ([`Store::append`]), and collect the
///    `notify::messages` for them with each recipient's subscriptions;
/// 3. replace the contents of `titles` with every fetched task's title,
///    and of `starts` with every Vikunja start date that is set;
/// 4. after releasing the lock, send each message through `push` and
///    remove the subscriptions the report lists as gone.
///
/// Returns the changes that were persisted (empty when nothing moved).
/// A poisoned lock is recovered: [`Store::append`] is one transaction,
/// so a panic elsewhere cannot leave a half-written batch.
///
/// # Errors
///
/// [`SyncError::Fetch`] when Vikunja is unreachable, rejects the token
/// or any page fails (nothing changed); [`SyncError::Store`] when
/// loading or appending fails (the transaction wrote nothing).
/// Push failures are never errors.
///
/// [`Store::load_ledger`]: crate::store::Store::load_ledger
/// [`Store::append`]: crate::store::Store::append
/// [`fetch_all`]: crate::vikunja::fetch_all
// DECISION: titles and starts stay two plain maps (web reads them
// separately; only titles are worker-only), hence six arguments.
#[allow(clippy::too_many_arguments)]
pub async fn poll_once<S, P>(
    source: &S,
    store: &Mutex<Store<LedgerDb>>,
    titles: &Mutex<BTreeMap<TaskId, String>>,
    starts: &Mutex<BTreeMap<TaskId, Timestamp>>,
    push: &P,
    now: Timestamp,
) -> Result<Vec<Change>, SyncError>
where
    S: PageSource + Sync,
    P: Push + Sync,
{
    let tasks = vikunja::fetch_all(source).await.map_err(SyncError::Fetch)?;
    let (changes, outbox) = {
        let mut store = lock(store);
        apply(&mut store, &tasks, now).map_err(SyncError::Store)?
    };
    *lock(starts) = tasks
        .iter()
        .filter_map(|t| t.start.map(|s| (TaskId(t.id), timestamp(s))))
        .collect();
    *lock(titles) = tasks.into_iter().map(|t| (TaskId(t.id), t.title)).collect();
    let mut gone = Vec::new();
    for (message, subscriptions) in &outbox {
        gone.extend(push.push(message, subscriptions).await.gone);
    }
    if !gone.is_empty() {
        let mut store = lock(store);
        for endpoint in &gone {
            // DECISION: a failed cleanup is logged, not returned: the batch
            // is already persisted and push failures are never errors.
            if let Err(e) = store.remove_subscription(endpoint) {
                eprintln!("sync: could not remove a gone push subscription: {e}");
            }
        }
    }
    Ok(changes)
}

/// The poll loop (APP-2, DB-3): one [`poll_once`] immediately, then one
/// every `interval` (`config.poll_interval`), each at the current UTC
/// time. A failed poll is logged to stderr and the loop waits for the
/// next tick (SRS 5.1). Never returns.
// DECISION: titles and starts stay two plain maps (web reads them
// separately; only titles are worker-only), hence six arguments.
#[allow(clippy::too_many_arguments)]
pub async fn run<S, P>(
    source: &S,
    store: &Mutex<Store<LedgerDb>>,
    titles: &Mutex<BTreeMap<TaskId, String>>,
    starts: &Mutex<BTreeMap<TaskId, Timestamp>>,
    push: &P,
    interval: Duration,
) where
    S: PageSource + Sync,
    P: Push + Sync,
{
    // DECISION: a poll slower than the interval delays the next tick
    // instead of firing a burst of catch-up polls.
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // The first tick completes at once: the startup poll (DB-3).
        ticks.tick().await;
        let now = timestamp(OffsetDateTime::now_utc());
        if let Err(e) = poll_once(source, store, titles, starts, push, now).await {
            eprintln!("sync: poll failed, retrying next interval: {e}");
        }
    }
}

/// Why one poll changed nothing.
#[derive(Debug)]
pub enum SyncError {
    /// The fetch failed; state untouched (SRS 5.1, Review Focus 2).
    Fetch(VikunjaError),
    /// Loading or appending failed; the batch was not written.
    Store(StoreError),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Fetch(e) => write!(f, "fetch failed, nothing changed: {e}"),
            SyncError::Store(e) => write!(f, "store failed, nothing written: {e}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SyncError::Fetch(e) => Some(e),
            SyncError::Store(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::{Push, SyncError, poll_once, run};
    use crate::auth::{AccountId, PasswordHash, Role};
    use crate::ledger::{Change, Outcome, TaskId, Timestamp};
    use crate::notify::{Message, MessageKind, SendReport, Subscription};
    use crate::store::{LedgerDb, Store, StoredSubscription};
    use crate::vikunja::{Page, PageSource, Resource, VikunjaError};

    /// 2020-01-01T00:00:00Z, the fixture task's due date. In the past,
    /// so [`run`]'s wall clock is always after it.
    const DUE: i64 = 1_577_836_800;

    /// Serves one task page and one project page, or fails a chosen one.
    struct FakeSource {
        tasks: String,
        fail: Option<Resource>,
        polls: AtomicUsize,
    }

    impl FakeSource {
        fn task(done: bool, done_at: &str) -> FakeSource {
            FakeSource {
                tasks: format!(
                    r#"[{{"id":42,"title":"Pay rent","project_id":7,"done":{done},
                        "due_date":"2020-01-01T00:00:00Z","done_at":"{done_at}",
                        "repeat_after":0,"repeat_mode":0}}]"#
                ),
                fail: None,
                polls: AtomicUsize::new(0),
            }
        }

        fn open() -> FakeSource {
            FakeSource::task(false, "0001-01-01T00:00:00Z")
        }
    }

    impl PageSource for FakeSource {
        async fn page(&self, resource: Resource, page: u32) -> Result<Page, VikunjaError> {
            if resource == Resource::Tasks && page == 1 {
                self.polls.fetch_add(1, Ordering::SeqCst);
            }
            if self.fail == Some(resource) {
                return Err(VikunjaError::Transport("connection refused".into()));
            }
            let body = match resource {
                Resource::Tasks => self.tasks.clone(),
                Resource::Projects => r#"[{"id":7,"is_archived":false}]"#.to_owned(),
            };
            Ok(Page {
                body,
                total_pages: 1,
            })
        }
    }

    /// Records every push; reports the listed endpoints as gone.
    #[derive(Default)]
    struct FakePush {
        sent: Mutex<Vec<(Message, Vec<Subscription>)>>,
        gone: Vec<String>,
    }

    impl Push for FakePush {
        async fn push(&self, message: &Message, subscriptions: &[Subscription]) -> SendReport {
            self.sent
                .lock()
                .unwrap()
                .push((*message, subscriptions.to_vec()));
            let gone: Vec<String> = subscriptions
                .iter()
                .map(|s| s.endpoint.clone())
                .filter(|e| self.gone.contains(e))
                .collect();
            SendReport {
                delivered: subscriptions.len() - gone.len(),
                failed: gone.len(),
                gone,
            }
        }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f)
    }

    /// A worker and one real witness, each with one device.
    fn people(store: &mut Store<LedgerDb>) -> (AccountId, AccountId) {
        let hash = PasswordHash("x".into());
        let worker = store.insert_account("w@x", &hash, Role::Worker).unwrap().id;
        let witness = store
            .insert_account("v@x", &hash, Role::Witness)
            .unwrap()
            .id;
        for (account, endpoint) in [
            (worker, "https://push/worker"),
            (witness, "https://push/witness"),
        ] {
            store
                .upsert_subscription(&StoredSubscription {
                    account,
                    endpoint: endpoint.into(),
                    p256dh: "k".into(),
                    auth: "a".into(),
                })
                .unwrap();
        }
        (worker, witness)
    }

    fn fresh() -> (Mutex<Store<LedgerDb>>, AccountId, AccountId) {
        let mut store = Store::open_in_memory().unwrap();
        let (worker, witness) = people(&mut store);
        (Mutex::new(store), worker, witness)
    }

    fn starts() -> Mutex<BTreeMap<TaskId, Timestamp>> {
        Mutex::new(BTreeMap::new())
    }

    fn titles() -> Mutex<BTreeMap<TaskId, String>> {
        Mutex::new(BTreeMap::new())
    }

    fn changes(store: &Mutex<Store<LedgerDb>>) -> Vec<Change> {
        store.lock().unwrap().load_changes().unwrap()
    }

    fn decisions(store: &Mutex<Store<LedgerDb>>) -> Vec<Outcome> {
        changes(store)
            .into_iter()
            .filter_map(|c| match c {
                Change::Decided { outcome, .. } => Some(outcome),
                _ => None,
            })
            .collect()
    }

    // sync-01
    #[test]
    fn sync_successful_poll_applies_persists_notifies() {
        let (store, worker, witness) = fresh();
        let titles = titles();
        let push = FakePush {
            gone: vec!["https://push/witness".into()],
            ..FakePush::default()
        };
        let source = FakeSource::open();

        // Before the deadline: recorded, persisted, nothing notified.
        let first = block_on(poll_once(
            &source,
            &store,
            &titles,
            &starts(),
            &push,
            Timestamp(DUE - 60),
        ))
        .unwrap();
        assert!(matches!(
            first.as_slice(),
            [Change::Recorded {
                task: TaskId(42),
                deadline: Timestamp(DUE),
                ..
            }]
        ));
        assert_eq!(changes(&store), first);
        assert_eq!(
            titles.lock().unwrap().get(&TaskId(42)).map(String::as_str),
            Some("Pay rent")
        );
        assert!(push.sent.lock().unwrap().is_empty());

        // After it, still open: late, debt, auto-claimed by the only witness.
        let second = block_on(poll_once(
            &source,
            &store,
            &titles,
            &starts(),
            &push,
            Timestamp(DUE + 60),
        ))
        .unwrap();
        assert!(matches!(
            second.as_slice(),
            [
                Change::Decided {
                    outcome: Outcome::Late,
                    ..
                },
                Change::DebtCreated { .. },
                Change::Claimed {
                    automatic: true,
                    ..
                },
            ]
        ));
        assert_eq!(changes(&store), [first, second].concat());

        // APP-34/35: "claimed for you" to the witness, "claimed" to the
        // worker, each with that person's devices.
        let sent = push.sent.lock().unwrap();
        let got: Vec<(MessageKind, AccountId, TaskId, Vec<&str>)> = sent
            .iter()
            .map(|(m, subs)| {
                let endpoints = subs.iter().map(|s| s.endpoint.as_str()).collect();
                (m.kind, m.to, m.task, endpoints)
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    MessageKind::ClaimedForYou,
                    witness,
                    TaskId(42),
                    vec!["https://push/witness"]
                ),
                (
                    MessageKind::Claimed,
                    worker,
                    TaskId(42),
                    vec!["https://push/worker"]
                ),
            ]
        );
        // The device reported gone is forgotten.
        assert!(
            store
                .lock()
                .unwrap()
                .subscriptions(witness)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn db_3_first_poll_runs_at_startup() {
        let path =
            std::env::temp_dir().join(format!("atomanja-sync-db3-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let push = FakePush::default();
        // Before the restart: the deadline is recorded, still in the future.
        {
            let mut store = Store::<LedgerDb>::open(&path).unwrap();
            people(&mut store);
            let store = Mutex::new(store);
            let source = FakeSource::open();
            block_on(poll_once(
                &source,
                &store,
                &titles(),
                &starts(),
                &push,
                Timestamp(DUE - 60),
            ))
            .unwrap();
        }
        // The deadline passed while down. Restart: the loop polls at once,
        // without waiting its (long) interval, and decides (DB-3, APP-9).
        let store = Mutex::new(Store::<LedgerDb>::open(&path).unwrap());
        let titles = titles();
        let source = FakeSource::open();
        let ran = block_on(async {
            tokio::time::timeout(
                Duration::from_millis(300),
                run(&source, &store, &titles, &starts(), &push, Duration::from_hours(1)),
            )
            .await
        });
        assert!(ran.is_err(), "run never returns");
        assert_eq!(source.polls.load(Ordering::SeqCst), 1);
        assert_eq!(decisions(&store), [Outcome::Late]);
        assert!(titles.lock().unwrap().contains_key(&TaskId(42)));
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn run_polls_again_every_interval() {
        let (store, _, _) = fresh();
        let source = FakeSource::open();
        let push = FakePush::default();
        let ran = block_on(async {
            tokio::time::timeout(
                Duration::from_millis(250),
                run(
                    &source,
                    &store,
                    &titles(),
                    &starts(),
                    &push,
                    Duration::from_millis(100),
                ),
            )
            .await
        });
        assert!(ran.is_err());
        // At 0, 100 and 200 ms.
        assert_eq!(source.polls.load(Ordering::SeqCst), 3);
    }

    // sync-02
    #[test]
    fn sync_failed_fetch_changes_nothing() {
        // Either list failing fails the whole poll, including a failure
        // after the task pages already arrived (Review Focus 2).
        for broken in [Resource::Tasks, Resource::Projects] {
            let (store, worker, witness) = fresh();
            let push = FakePush::default();
            let titles = titles();
            let ok = FakeSource::open();
            block_on(poll_once(&ok, &store, &titles, &starts(), &push, Timestamp(DUE - 60))).unwrap();
            let before = changes(&store);
            *titles.lock().unwrap() = BTreeMap::from([(TaskId(1), "old".to_owned())]);

            // The deadline has passed, but Vikunja is unreachable.
            let source = FakeSource {
                fail: Some(broken),
                ..FakeSource::open()
            };
            let err = block_on(poll_once(
                &source,
                &store,
                &titles,
                &starts(),
                &push,
                Timestamp(DUE + 60),
            ))
            .unwrap_err();

            assert!(
                matches!(&err, SyncError::Fetch(VikunjaError::Transport(_))),
                "{err:?}"
            );
            assert!(err.to_string().contains("connection refused"), "{err}");
            assert!(std::error::Error::source(&err).is_some());
            assert_eq!(changes(&store), before, "nothing persisted");
            assert_eq!(
                *titles.lock().unwrap(),
                BTreeMap::from([(TaskId(1), "old".to_owned())]),
                "titles untouched"
            );
            assert!(push.sent.lock().unwrap().is_empty(), "nothing sent");
            let store = store.lock().unwrap();
            assert_eq!(store.subscriptions(worker).unwrap().len(), 1);
            assert_eq!(store.subscriptions(witness).unwrap().len(), 1);
        }
    }

    #[test]
    fn app_9_decisions_deferred_until_successful_poll() {
        let (store, _, _) = fresh();
        let push = FakePush::default();
        let titles = titles();
        block_on(poll_once(
            &FakeSource::open(),
            &store,
            &titles,
            &starts(),
            &push,
            Timestamp(DUE - 600),
        ))
        .unwrap();

        // Polls after the deadline fail: no decision yet (SRS 5.1).
        let down = FakeSource {
            fail: Some(Resource::Tasks),
            ..FakeSource::open()
        };
        for late in [60, 3600] {
            let polled = block_on(poll_once(
                &down,
                &store,
                &titles,
                &starts(),
                &push,
                Timestamp(DUE + late),
            ));
            assert!(polled.is_err());
            assert!(decisions(&store).is_empty());
        }

        // First successful poll: decided once, from Vikunja's done_at
        // (done 5 minutes before the deadline: met, APP-9).
        let done = FakeSource::task(true, "2019-12-31T23:55:00Z");
        let decided = block_on(poll_once(
            &done,
            &store,
            &titles,
            &starts(),
            &push,
            Timestamp(DUE + 7200),
        ))
        .unwrap();
        assert!(decided.contains(&Change::Decided {
            occurrence: crate::ledger::OccurrenceId(0),
            outcome: Outcome::Met,
            at: Timestamp(DUE + 7200),
        }));
        assert_eq!(decisions(&store), [Outcome::Met]);

        // And never again (APP-10).
        let again = block_on(poll_once(
            &done,
            &store,
            &titles,
            &starts(),
            &push,
            Timestamp(DUE + 9000),
        ))
        .unwrap();
        assert!(again.is_empty());
        assert_eq!(decisions(&store), [Outcome::Met]);
    }
}
