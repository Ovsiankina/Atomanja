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
use std::sync::Mutex;
use std::time::Duration;

use crate::ledger::{Change, TaskId, Timestamp};
use crate::notify::{Message, Notifier, SendReport, Subscription};
use crate::store::{LedgerDb, Store, StoreError};
use crate::vikunja::{PageSource, VikunjaError};

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
        let _ = (message, subscriptions);
        async move { todo!() }
    }
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
/// 3. replace the contents of `titles` with every fetched task's title;
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
pub async fn poll_once<S, P>(
    source: &S,
    store: &Mutex<Store<LedgerDb>>,
    titles: &Mutex<BTreeMap<TaskId, String>>,
    push: &P,
    now: Timestamp,
) -> Result<Vec<Change>, SyncError>
where
    S: PageSource + Sync,
    P: Push + Sync,
{
    let _ = (source, store, titles, push, now);
    todo!()
}

/// The poll loop (APP-2, DB-3): one [`poll_once`] immediately, then one
/// every `interval` (`config.poll_interval`), each at the current UTC
/// time. A failed poll is logged to stderr and the loop waits for the
/// next tick (SRS 5.1). Never returns.
pub async fn run<S, P>(
    source: &S,
    store: &Mutex<Store<LedgerDb>>,
    titles: &Mutex<BTreeMap<TaskId, String>>,
    push: &P,
    interval: Duration,
) where
    S: PageSource + Sync,
    P: Push + Sync,
{
    let _ = (source, store, titles, push, interval);
    todo!()
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
        let _ = f;
        todo!()
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    // sync-01
    #[test] fn sync_successful_poll_applies_persists_notifies() { todo!() }
    #[test] fn db_3_first_poll_runs_at_startup() { todo!() }

    // sync-02
    #[test] fn sync_failed_fetch_changes_nothing() { todo!() }
    #[test] fn app_9_decisions_deferred_until_successful_poll() { todo!() }
}
