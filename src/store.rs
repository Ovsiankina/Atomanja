//! `SQLite` persistence for the ledger database and the dev database:
//! schema, loading, insert-only ledger appends, accounts and tokens.
//!
//! Uses: `ledger`, `auth`, `config` (Lang only).
//! SRS: DB-1..DB-3, DEV-1, DEV-3, APP-39.
//!
//! # Contract shape: one typed handle per file
//!
//! A [`Store<K>`] owns one `rusqlite::Connection`. The marker `K`
//! ([`LedgerDb`] or [`DevDb`]) records which file it is, so a function
//! that takes `&Store<LedgerDb>` cannot be handed the dev file (DEV-3).
//! Both files share one schema and every method works on both; the
//! dev file holds fake data built through the same ledger rules.
//!
//! Ledger facts are stored as the `ledger::Change` values that produced
//! them. [`Store::append`] writes one batch in one transaction (Review
//! Focus 4); [`Store::load_ledger`] replays every stored change in order
//! (DB-3). Rows are insert-only except the one recorded-deadline update
//! of APP-7 (DB-2). Unique constraints make a second claim of a debt, or
//! a replayed batch, fail with [`StoreError::Conflict`] and write
//! nothing (Review Focus 3, APP-12).
//!
//! TODO(srs) DEV-3: which file holds the admin account and its sessions
//! is open (login reads accounts before the role is known). Account and
//! token methods are generic over `K`, so either answer fits this
//! contract; see `docs/deferred.md`.

use std::marker::PhantomData;
use std::path::Path;

use crate::auth::{Account, AccountId, Database, IssuedToken, PasswordHash, Role, Token};
use crate::config::Lang;
use crate::ledger::{Change, Ledger, WitnessId};

/// Which file a [`Store`] handle is open on (DB-1, DEV-1, DEV-3).
/// Implemented only by [`LedgerDb`] and [`DevDb`].
pub trait Kind {
    /// The `auth` database this marker stands for.
    const DATABASE: Database;
}

/// Marker for the ledger database file (DB-1). Never constructed.
#[derive(Debug)]
pub enum LedgerDb {}

/// Marker for the dev database file (DEV-1). Never constructed.
#[derive(Debug)]
pub enum DevDb {}

impl Kind for LedgerDb {
    const DATABASE: Database = Database::Ledger;
}

impl Kind for DevDb {
    const DATABASE: Database = Database::Dev;
}

/// A stored push subscription: one device of one account (APP-33).
/// `Debug` hides the keys. The caller converts it to
/// `notify::Subscription` (no `store -> notify` arrow in C4).
#[derive(Clone, PartialEq, Eq)]
pub struct StoredSubscription {
    /// Owner account.
    pub account: AccountId,
    /// Push service endpoint URL; unique per file.
    pub endpoint: String,
    /// Browser public key `p256dh`, base64url.
    pub p256dh: String,
    /// Browser auth secret, base64url.
    pub auth: String,
}

/// An open database file of kind `K`.
pub struct Store<K: Kind> {
    conn: rusqlite::Connection,
    kind: PhantomData<K>,
}

impl<K: Kind> Store<K> {
    /// Open (creating if absent) the file at `path` and create or upgrade
    /// the schema (DB-1, DEV-1). The path comes from `config`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when the file cannot be opened or the
    /// schema cannot be created.
    pub fn open(path: &Path) -> Result<Store<K>, StoreError> {
        let _ = path;
        todo!()
    }

    /// A fresh in-memory database with the schema, for tests.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when the schema cannot be created.
    pub fn open_in_memory() -> Result<Store<K>, StoreError> {
        todo!()
    }

    /// Which file this handle is open on (DEV-3).
    pub fn database(&self) -> Database {
        todo!()
    }

    // --- ledger facts (DB-2, DB-3) ---

    /// Append one batch of changes in one transaction, in order (DB-2,
    /// Review Focus 4). An `Event` of kind `MovedEarlier` also updates
    /// that occurrence's recorded deadline: the only update DB-2 allows.
    /// All or nothing: on any error no row of the batch is written.
    ///
    /// # Errors
    ///
    /// [`StoreError::Conflict`] when a row already exists (second claim
    /// of a debt, same occurrence or debt id twice, replayed batch);
    /// [`StoreError::Sqlite`] for any other database failure.
    pub fn append(&mut self, changes: &[Change]) -> Result<(), StoreError> {
        let _ = changes;
        todo!()
    }

    /// Every stored change, in the order it was appended (DB-3).
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] when a row does not decode to a change,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn load_changes(&self) -> Result<Vec<Change>, StoreError> {
        todo!()
    }

    /// A ledger rebuilt by applying [`Store::load_changes`] in order to
    /// `Ledger::new()` (DB-3).
    ///
    /// # Errors
    ///
    /// As [`Store::load_changes`].
    pub fn load_ledger(&self) -> Result<Ledger, StoreError> {
        todo!()
    }

    // --- accounts (APP-25..APP-30, APP-39) ---

    /// Insert an account with language French (APP-39) and return it
    /// with its new id. The caller has checked `auth::may_create_worker`
    /// for a worker (APP-25).
    ///
    /// # Errors
    ///
    /// [`StoreError::Conflict`] when the email is taken,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn insert_account(
        &mut self,
        email: &str,
        password_hash: &PasswordHash,
        role: Role,
    ) -> Result<Account, StoreError> {
        let _ = (email, password_hash, role);
        todo!()
    }

    /// The account with this id, if any.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] for an undecodable row,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn account(&self, id: AccountId) -> Result<Option<Account>, StoreError> {
        let _ = id;
        todo!()
    }

    /// The account with this email (exact match), if any. Used by login
    /// and reset requests.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] for an undecodable row,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn account_by_email(&self, email: &str) -> Result<Option<Account>, StoreError> {
        let _ = email;
        todo!()
    }

    /// Every account, by id.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] for an undecodable row,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn accounts(&self) -> Result<Vec<Account>, StoreError> {
        todo!()
    }

    /// Ids of the real witnesses (`Role::Witness`, admin excluded,
    /// APP-20), by id, for the ledger's claiming rules.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn real_witnesses(&self) -> Result<Vec<WitnessId>, StoreError> {
        todo!()
    }

    /// Change an account's email. The caller has checked
    /// `auth::permits(.., ChangeEmail)` (APP-29).
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] for an unknown id,
    /// [`StoreError::Conflict`] when the email is taken,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn set_email(&mut self, id: AccountId, email: &str) -> Result<(), StoreError> {
        let _ = (id, email);
        todo!()
    }

    /// Replace an account's password hash. The caller has checked
    /// `auth::permits(.., SetPassword)` or a reset token (APP-27, APP-28).
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] for an unknown id,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn set_password(&mut self, id: AccountId, hash: &PasswordHash) -> Result<(), StoreError> {
        let _ = (id, hash);
        todo!()
    }

    /// Change an account's language (APP-39).
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] for an unknown id,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn set_lang(&mut self, id: AccountId, lang: Lang) -> Result<(), StoreError> {
        let _ = (id, lang);
        todo!()
    }

    // --- tokens (APP-26, APP-28, sessions) ---

    /// Store a newly issued token.
    ///
    /// # Errors
    ///
    /// [`StoreError::Conflict`] when the same secret exists,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn insert_token(&mut self, token: &IssuedToken) -> Result<(), StoreError> {
        let _ = token;
        todo!()
    }

    /// The stored row for this secret, if any. The caller runs
    /// `IssuedToken::check` on it.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] for an undecodable row,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn token(&self, token: &Token) -> Result<Option<IssuedToken>, StoreError> {
        let _ = token;
        todo!()
    }

    /// Mark a token used, atomically: `Ok(true)` if this call flipped it
    /// from unused to used, `Ok(false)` if it was already used, so two
    /// concurrent uses of a single-use link cannot both win (APP-26,
    /// APP-28, Review Focus 5). Also used for logout.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] for an unknown secret,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn use_token(&mut self, token: &Token) -> Result<bool, StoreError> {
        let _ = token;
        todo!()
    }

    // --- push subscriptions (APP-33) ---

    /// Store a device subscription. Re-subscribing the same endpoint
    /// replaces its keys and owner; an account may hold any number of
    /// endpoints (APP-33).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn upsert_subscription(&mut self, sub: &StoredSubscription) -> Result<(), StoreError> {
        let _ = sub;
        todo!()
    }

    /// Every device subscription of an account.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn subscriptions(&self, account: AccountId) -> Result<Vec<StoredSubscription>, StoreError> {
        let _ = account;
        todo!()
    }

    /// Delete the subscription with this endpoint, if any (gone device,
    /// see `notify::SendReport::gone`).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn remove_subscription(&mut self, endpoint: &str) -> Result<(), StoreError> {
        let _ = endpoint;
        todo!()
    }
}

/// Why a store operation failed. On any error, nothing was written.
#[derive(Debug)]
pub enum StoreError {
    /// A unique constraint refused the write: second claim, duplicate
    /// id, replayed batch, taken email (Review Focus 3, APP-12).
    Conflict,
    /// No row with this id or secret.
    NotFound,
    /// A stored row does not decode (unknown discriminant, bad value).
    Corrupt(String),
    /// Any other `SQLite` failure.
    Sqlite(rusqlite::Error),
}

impl std::fmt::Debug for StoredSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl<K: Kind> std::fmt::Debug for Store<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = (f, &self.conn, self.kind);
        todo!()
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        todo!()
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        let _ = err;
        todo!()
    }
}

#[cfg(test)]
mod tests {
    // store-01
    #[test] fn db_1_creates_schema_in_new_file() { todo!() }
    #[test] fn dev_1_dev_handle_reports_dev_database() { todo!() }
    #[test] fn dev_3_dev_handle_not_accepted_where_ledger_required() { todo!() }

    // store-02
    #[test] fn db_2_ledger_rows_insert_only() { todo!() }
    #[test] fn db_2_only_app_7_updates_recorded_deadline() { todo!() }
    #[test] fn db_3_load_after_restart_equals_state_before() { todo!() }
    #[test] fn store_poll_changes_appended_in_one_transaction() { todo!() }
    #[test] fn store_reapplying_same_poll_no_duplicate_debt() { todo!() }

    // store-03
    #[test] fn store_concurrent_claims_one_row() { todo!() }
    #[test] fn store_account_crud() { todo!() }
    #[test] fn store_single_use_token_used_once() { todo!() }
    #[test] fn store_push_subscriptions_per_device() { todo!() }
    #[test] fn app_39_account_language_persists_default_french() { todo!() }
}
