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

use crate::auth::AuthError;
use crate::ledger::Timestamp;
use crate::store::{DevDb, Store, StoreError};

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
    let _ = (store, now);
    todo!()
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
        let _ = f;
        todo!()
    }
}

impl std::error::Error for DevdataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    // devdata-01
    #[test] fn dev_2_seed_covers_claimable_claimed_sent_received() { todo!() }
    #[test] fn dev_2_seed_writes_only_dev_db() { todo!() }
}
