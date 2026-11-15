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
//! A dev handle does not compile where a ledger handle is required:
//!
//! ```compile_fail
//! use atomanja::store::{DevDb, LedgerDb, Store};
//! fn ledger_only(_: &Store<LedgerDb>) {}
//! let dev = Store::<DevDb>::open_in_memory().unwrap();
//! ledger_only(&dev);
//! ```
//!
//! TODO(srs) DEV-3: which file holds the admin account and its sessions
//! is open (login reads accounts before the role is known). Account and
//! token methods are generic over `K`, so either answer fits this
//! contract; see `docs/deferred.md`.

use std::marker::PhantomData;
use std::path::Path;

use crate::auth::{
    Account, AccountId, Database, IssuedToken, PasswordHash, Role, Token, TokenKind,
};
use crate::config::Lang;
use crate::ledger::{
    Centimes, Change, DebtId, EventKind, Ledger, OccurrenceId, Outcome, TaskId, Timestamp,
    WitnessId,
};

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
        let conn = rusqlite::Connection::open(path)?;
        // DECISION: wait up to 5 s for a lock instead of failing at once,
        // so two requests writing the same file serialise (Review Focus 3).
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::with_schema(conn)
    }

    /// A fresh in-memory database with the schema, for tests.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] when the schema cannot be created.
    pub fn open_in_memory() -> Result<Store<K>, StoreError> {
        Self::with_schema(rusqlite::Connection::open_in_memory()?)
    }

    /// Which file this handle is open on (DEV-3).
    pub fn database(&self) -> Database {
        K::DATABASE
    }

    /// Create the schema if absent and wrap the connection.
    fn with_schema(conn: rusqlite::Connection) -> Result<Store<K>, StoreError> {
        conn.execute_batch(SCHEMA)?;
        Ok(Store {
            conn,
            kind: PhantomData,
        })
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
        // Dropping the transaction on an early `?` rolls the batch back.
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO changes (kind, at, occurrence, debt, task, witness, amount, due, sub)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for change in changes {
                let r = ChangeRow::encode(change);
                insert.execute(rusqlite::params![
                    r.kind,
                    r.at,
                    r.occurrence,
                    r.debt,
                    r.task,
                    r.witness,
                    r.amount,
                    r.due,
                    r.sub
                ])?;
                match *change {
                    Change::Recorded {
                        occurrence,
                        deadline,
                        ..
                    } => {
                        tx.execute(
                            "INSERT INTO deadlines (occurrence, deadline) VALUES (?1, ?2)",
                            rusqlite::params![occurrence.0, deadline.0],
                        )?;
                    }
                    Change::Event {
                        occurrence,
                        kind: EventKind::MovedEarlier,
                        new_due: Some(due),
                        ..
                    } => {
                        // DECISION: same rule as `Ledger::apply`: only a
                        // pending (undecided) occurrence follows an earlier
                        // due date, so the table always matches the replay.
                        tx.execute(
                            "UPDATE deadlines SET deadline = ?2
                             WHERE occurrence = ?1 AND deadline > ?2
                               AND NOT EXISTS (SELECT 1 FROM changes
                                               WHERE kind = ?3 AND occurrence = ?1)",
                            rusqlite::params![occurrence.0, due.0, tag::DECIDED],
                        )?;
                    }
                    _ => {}
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every stored change, in the order it was appended (DB-3).
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] when a row does not decode to a change,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn load_changes(&self) -> Result<Vec<Change>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, at, occurrence, debt, task, witness, amount, due, sub
             FROM changes ORDER BY seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ChangeRow {
                kind: r.get(0)?,
                at: r.get(1)?,
                occurrence: r.get(2)?,
                debt: r.get(3)?,
                task: r.get(4)?,
                witness: r.get(5)?,
                amount: r.get(6)?,
                due: r.get(7)?,
                sub: r.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.decode()?);
        }
        Ok(out)
    }

    /// A ledger rebuilt by applying [`Store::load_changes`] in order to
    /// `Ledger::new()` (DB-3).
    ///
    /// # Errors
    ///
    /// As [`Store::load_changes`].
    pub fn load_ledger(&self) -> Result<Ledger, StoreError> {
        let mut ledger = Ledger::new();
        for change in self.load_changes()? {
            ledger.apply(&change);
        }
        Ok(ledger)
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
        let lang = Lang::Fr;
        self.conn.execute(
            "INSERT INTO accounts (email, password_hash, lang, role) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![email, password_hash.0, lang.code(), role as u8],
        )?;
        Ok(Account {
            id: AccountId(self.conn.last_insert_rowid()),
            email: email.to_owned(),
            password_hash: password_hash.clone(),
            lang,
            role,
        })
    }

    /// Accounts matching `filter` (an SQL condition on `?1`), by id.
    fn query_accounts(
        &self,
        filter: &str,
        arg: &dyn rusqlite::ToSql,
    ) -> Result<Vec<Account>, StoreError> {
        let sql = format!(
            "SELECT id, email, password_hash, lang, role FROM accounts WHERE {filter} ORDER BY id"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([arg], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, email, hash, lang, role) = row?;
            out.push(Account {
                id: AccountId(id),
                email,
                password_hash: PasswordHash(hash),
                lang: Lang::from_code(&lang)
                    .ok_or_else(|| corrupt(format!("account {id}: lang {lang:?}")))?,
                role: decode_role(role)?,
            });
        }
        Ok(out)
    }

    /// Run an UPDATE on one account row; `NotFound` when no row matched.
    fn update_account(
        &mut self,
        sql: &str,
        id: AccountId,
        value: &dyn rusqlite::ToSql,
    ) -> Result<(), StoreError> {
        match self.conn.execute(sql, rusqlite::params![id.0, value])? {
            0 => Err(StoreError::NotFound),
            _ => Ok(()),
        }
    }

    /// The account with this id, if any.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] for an undecodable row,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn account(&self, id: AccountId) -> Result<Option<Account>, StoreError> {
        Ok(self.query_accounts("id = ?1", &id.0)?.pop())
    }

    /// The account with this email (exact match), if any. Used by login
    /// and reset requests.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] for an undecodable row,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn account_by_email(&self, email: &str) -> Result<Option<Account>, StoreError> {
        Ok(self.query_accounts("email = ?1", &email)?.pop())
    }

    /// Every account, by id.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] for an undecodable row,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn accounts(&self) -> Result<Vec<Account>, StoreError> {
        self.query_accounts("?1", &true)
    }

    /// Ids of the real witnesses (`Role::Witness`, admin excluded,
    /// APP-20), by id, for the ledger's claiming rules.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn real_witnesses(&self) -> Result<Vec<WitnessId>, StoreError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM accounts WHERE role = ?1 ORDER BY id")?;
        let ids = stmt
            .query_map([Role::Witness as u8], |r| r.get(0).map(WitnessId))?
            .collect::<Result<_, _>>()?;
        Ok(ids)
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
        self.update_account("UPDATE accounts SET email = ?2 WHERE id = ?1", id, &email)
    }

    /// Replace an account's password hash. The caller has checked
    /// `auth::permits(.., SetPassword)` or a reset token (APP-27, APP-28).
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] for an unknown id,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn set_password(&mut self, id: AccountId, hash: &PasswordHash) -> Result<(), StoreError> {
        self.update_account(
            "UPDATE accounts SET password_hash = ?2 WHERE id = ?1",
            id,
            &hash.0,
        )
    }

    /// Change an account's language (APP-39).
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`] for an unknown id,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn set_lang(&mut self, id: AccountId, lang: Lang) -> Result<(), StoreError> {
        self.update_account(
            "UPDATE accounts SET lang = ?2 WHERE id = ?1",
            id,
            &lang.code(),
        )
    }

    // --- tokens (APP-26, APP-28, sessions) ---

    /// Store a newly issued token.
    ///
    /// # Errors
    ///
    /// [`StoreError::Conflict`] when the same secret exists,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn insert_token(&mut self, token: &IssuedToken) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO tokens (secret, kind, account, issued_at, used)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                token.token.0,
                token.kind as u8,
                token.account.map(|a| a.0),
                encode_time(token.issued_at)?,
                token.used
            ],
        )?;
        Ok(())
    }

    /// The stored row for this secret, if any. The caller runs
    /// `IssuedToken::check` on it.
    ///
    /// # Errors
    ///
    /// [`StoreError::Corrupt`] for an undecodable row,
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn token(&self, token: &Token) -> Result<Option<IssuedToken>, StoreError> {
        use rusqlite::OptionalExtension;
        // DECISION: the secret is the primary key, so the lookup is an
        // index search, not a byte-by-byte compare in Rust (auth review).
        let row = self
            .conn
            .query_row(
                "SELECT kind, account, issued_at, used FROM tokens WHERE secret = ?1",
                [&token.0],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, bool>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((kind, account, issued_at, used)) = row else {
            return Ok(None);
        };
        Ok(Some(IssuedToken {
            token: token.clone(),
            kind: match kind {
                0 => TokenKind::Invite,
                1 => TokenKind::Reset,
                2 => TokenKind::Session,
                n => return Err(corrupt(format!("token kind {n}"))),
            },
            account: account.map(AccountId),
            issued_at: decode_time(issued_at)?,
            used,
        }))
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
        // One conditional UPDATE: SQLite serialises writers, so only one
        // caller can see the row flip from 0 to 1.
        let flipped = self.conn.execute(
            "UPDATE tokens SET used = 1 WHERE secret = ?1 AND used = 0",
            [&token.0],
        )?;
        if flipped == 1 {
            return Ok(true);
        }
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM tokens WHERE secret = ?1)",
            [&token.0],
            |r| r.get(0),
        )?;
        if exists {
            Ok(false)
        } else {
            Err(StoreError::NotFound)
        }
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
        self.conn.execute(
            "INSERT INTO subscriptions (endpoint, account, p256dh, auth) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (endpoint) DO UPDATE
             SET account = excluded.account, p256dh = excluded.p256dh, auth = excluded.auth",
            rusqlite::params![sub.endpoint, sub.account.0, sub.p256dh, sub.auth],
        )?;
        Ok(())
    }

    /// Every device subscription of an account.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn subscriptions(&self, account: AccountId) -> Result<Vec<StoredSubscription>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT endpoint, p256dh, auth FROM subscriptions WHERE account = ?1 ORDER BY endpoint",
        )?;
        let subs = stmt
            .query_map([account.0], |r| {
                Ok(StoredSubscription {
                    account,
                    endpoint: r.get(0)?,
                    p256dh: r.get(1)?,
                    auth: r.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(subs)
    }

    /// Delete the subscription with this endpoint, if any (gone device,
    /// see `notify::SendReport::gone`).
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] for a database failure.
    pub fn remove_subscription(&mut self, endpoint: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM subscriptions WHERE endpoint = ?1", [endpoint])?;
        Ok(())
    }
}

/// Stored `kind` values of the `changes` table, one per `Change` variant.
// DECISION: explicit, stable discriminants owned by store, never derived
// from the enum's declaration order, so reordering `ledger::Change` cannot
// silently change what an existing file means. Never renumber; append.
// The partial unique indexes in `SCHEMA` use these numbers literally.
mod tag {
    pub(super) const RECORDED: i64 = 0;
    pub(super) const SEEN_DONE: i64 = 1;
    pub(super) const EVENT: i64 = 2;
    pub(super) const DECIDED: i64 = 3;
    pub(super) const DEBT_CREATED: i64 = 4;
    pub(super) const CLAIMED: i64 = 5;
    pub(super) const MARKED_SENT: i64 = 6;
    pub(super) const MARKED_RECEIVED: i64 = 7;
    pub(super) const AMOUNT_PROPOSED: i64 = 8;
    pub(super) const AMOUNT_APPROVED: i64 = 9;
    pub(super) const AMOUNT_IN_FORCE: i64 = 10;
}

/// One `changes` row. `sub` holds the variant's small field: the event
/// kind, the outcome, or a bool (`done`, `automatic`).
struct ChangeRow {
    kind: i64,
    at: i64,
    occurrence: Option<i64>,
    debt: Option<i64>,
    task: Option<i64>,
    witness: Option<i64>,
    amount: Option<i64>,
    due: Option<i64>,
    sub: Option<i64>,
}

impl ChangeRow {
    fn new(kind: i64, at: Timestamp) -> ChangeRow {
        ChangeRow {
            kind,
            at: at.0,
            occurrence: None,
            debt: None,
            task: None,
            witness: None,
            amount: None,
            due: None,
            sub: None,
        }
    }

    fn encode(change: &Change) -> ChangeRow {
        let occ = |o: OccurrenceId| Some(i64::from(o.0));
        let debt = |d: DebtId| Some(i64::from(d.0));
        let cents = |c: Centimes| Some(i64::from(c.0));
        match *change {
            Change::Recorded {
                occurrence,
                task,
                deadline,
                at,
            } => ChangeRow {
                occurrence: occ(occurrence),
                task: Some(task.0),
                due: Some(deadline.0),
                ..ChangeRow::new(tag::RECORDED, at)
            },
            Change::SeenDone {
                occurrence,
                done,
                at,
            } => ChangeRow {
                occurrence: occ(occurrence),
                sub: Some(i64::from(done)),
                ..ChangeRow::new(tag::SEEN_DONE, at)
            },
            Change::Event {
                occurrence,
                kind,
                new_due,
                at,
            } => ChangeRow {
                occurrence: occ(occurrence),
                due: new_due.map(|t| t.0),
                sub: Some(i64::from(kind as u8)),
                ..ChangeRow::new(tag::EVENT, at)
            },
            Change::Decided {
                occurrence,
                outcome,
                at,
            } => ChangeRow {
                occurrence: occ(occurrence),
                sub: Some(i64::from(outcome as u8)),
                ..ChangeRow::new(tag::DECIDED, at)
            },
            Change::DebtCreated {
                debt: d,
                occurrence,
                amount,
                at,
            } => ChangeRow {
                debt: debt(d),
                occurrence: occ(occurrence),
                amount: cents(amount),
                ..ChangeRow::new(tag::DEBT_CREATED, at)
            },
            Change::Claimed {
                debt: d,
                witness,
                automatic,
                at,
            } => ChangeRow {
                debt: debt(d),
                witness: Some(witness.0),
                sub: Some(i64::from(automatic)),
                ..ChangeRow::new(tag::CLAIMED, at)
            },
            Change::MarkedSent { debt: d, at } => ChangeRow {
                debt: debt(d),
                ..ChangeRow::new(tag::MARKED_SENT, at)
            },
            Change::MarkedReceived { debt: d, at } => ChangeRow {
                debt: debt(d),
                ..ChangeRow::new(tag::MARKED_RECEIVED, at)
            },
            Change::AmountProposed { amount, at } => ChangeRow {
                amount: cents(amount),
                ..ChangeRow::new(tag::AMOUNT_PROPOSED, at)
            },
            Change::AmountApproved { witness, at } => ChangeRow {
                witness: Some(witness.0),
                ..ChangeRow::new(tag::AMOUNT_APPROVED, at)
            },
            Change::AmountInForce { amount, at } => ChangeRow {
                amount: cents(amount),
                ..ChangeRow::new(tag::AMOUNT_IN_FORCE, at)
            },
        }
    }

    fn decode(&self) -> Result<Change, StoreError> {
        let at = Timestamp(self.at);
        let change = match self.kind {
            tag::RECORDED => Change::Recorded {
                occurrence: self.occurrence()?,
                task: TaskId(need(self.task, "task")?),
                deadline: Timestamp(need(self.due, "deadline")?),
                at,
            },
            tag::SEEN_DONE => Change::SeenDone {
                occurrence: self.occurrence()?,
                done: self.flag()?,
                at,
            },
            tag::EVENT => Change::Event {
                occurrence: self.occurrence()?,
                kind: match need(self.sub, "event kind")? {
                    0 => EventKind::MovedLater,
                    1 => EventKind::MovedEarlier,
                    2 => EventKind::Removed,
                    3 => EventKind::Deleted,
                    4 => EventKind::Restored,
                    n => return Err(corrupt(format!("event kind {n}"))),
                },
                new_due: self.due.map(Timestamp),
                at,
            },
            tag::DECIDED => Change::Decided {
                occurrence: self.occurrence()?,
                outcome: match need(self.sub, "outcome")? {
                    1 => Outcome::Met,
                    2 => Outcome::Late,
                    n => return Err(corrupt(format!("outcome {n}"))),
                },
                at,
            },
            tag::DEBT_CREATED => Change::DebtCreated {
                debt: self.debt()?,
                occurrence: self.occurrence()?,
                amount: self.amount()?,
                at,
            },
            tag::CLAIMED => Change::Claimed {
                debt: self.debt()?,
                witness: WitnessId(need(self.witness, "witness")?),
                automatic: self.flag()?,
                at,
            },
            tag::MARKED_SENT => Change::MarkedSent {
                debt: self.debt()?,
                at,
            },
            tag::MARKED_RECEIVED => Change::MarkedReceived {
                debt: self.debt()?,
                at,
            },
            tag::AMOUNT_PROPOSED => Change::AmountProposed {
                amount: self.amount()?,
                at,
            },
            tag::AMOUNT_APPROVED => Change::AmountApproved {
                witness: WitnessId(need(self.witness, "witness")?),
                at,
            },
            tag::AMOUNT_IN_FORCE => Change::AmountInForce {
                amount: self.amount()?,
                at,
            },
            n => return Err(corrupt(format!("change kind {n}"))),
        };
        Ok(change)
    }

    fn occurrence(&self) -> Result<OccurrenceId, StoreError> {
        u32_col(self.occurrence, "occurrence").map(OccurrenceId)
    }

    fn debt(&self) -> Result<DebtId, StoreError> {
        u32_col(self.debt, "debt").map(DebtId)
    }

    fn amount(&self) -> Result<Centimes, StoreError> {
        u32_col(self.amount, "amount").map(Centimes)
    }

    fn flag(&self) -> Result<bool, StoreError> {
        match need(self.sub, "flag")? {
            0 => Ok(false),
            1 => Ok(true),
            n => Err(corrupt(format!("flag {n}"))),
        }
    }
}

fn corrupt(what: String) -> StoreError {
    StoreError::Corrupt(what)
}

fn decode_role(role: i64) -> Result<Role, StoreError> {
    match role {
        0 => Ok(Role::Worker),
        1 => Ok(Role::Witness),
        2 => Ok(Role::Admin),
        n => Err(corrupt(format!("role {n}"))),
    }
}

/// A token issue time as stored: nanoseconds since the Unix epoch, UTC.
// DECISION: integer nanoseconds, not RFC 3339 text, so the value round
// trips exactly and compares as a number. i64 nanoseconds reach year 2262.
fn encode_time(t: time::OffsetDateTime) -> Result<i64, StoreError> {
    i64::try_from(t.unix_timestamp_nanos())
        .map_err(|_| StoreError::Corrupt(format!("time {t} out of range")))
}

fn decode_time(nanos: i64) -> Result<time::OffsetDateTime, StoreError> {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(nanos))
        .map_err(|_| corrupt(format!("time {nanos} out of range")))
}

/// A column the row's kind requires.
fn need(value: Option<i64>, what: &str) -> Result<i64, StoreError> {
    value.ok_or_else(|| corrupt(format!("missing {what}")))
}

/// A required column that must fit a `u32` id or amount.
fn u32_col(value: Option<i64>, what: &str) -> Result<u32, StoreError> {
    let v = need(value, what)?;
    u32::try_from(v).map_err(|_| corrupt(format!("{what} {v} out of range")))
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
        f.debug_struct("StoredSubscription")
            .field("account", &self.account)
            .field("endpoint", &self.endpoint)
            .field("p256dh", &"<hidden>")
            .field("auth", &"<hidden>")
            .finish()
    }
}

impl<K: Kind> std::fmt::Debug for Store<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("database", &K::DATABASE)
            .field("path", &self.conn.path())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Conflict => f.write_str("row already exists"),
            StoreError::NotFound => f.write_str("no such row"),
            StoreError::Corrupt(what) => write!(f, "stored row does not decode: {what}"),
            StoreError::Sqlite(err) => write!(f, "sqlite: {err}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Sqlite(err) => Some(err),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        // DECISION: only UNIQUE and PRIMARY KEY violations are a Conflict
        // (a row already exists); a trigger refusing an update or delete,
        // or any other failure, stays a Sqlite error.
        if let rusqlite::Error::SqliteFailure(e, _) = &err
            && matches!(
                e.extended_code,
                rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    | rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
            )
        {
            return StoreError::Conflict;
        }
        StoreError::Sqlite(err)
    }
}

/// The schema of both files. Every statement is `IF NOT EXISTS`, so
/// opening an existing file is a no-op (idempotent creation).
// DECISION: ledger facts are one insert-only `changes` table, one row per
// `ledger::Change`, `seq` giving the append order (DB-3). Partial unique
// indexes encode "at most once" per occurrence or debt, so a second claim,
// a second debt for one occurrence or a replayed batch is refused by the
// database itself (APP-12, Review Focus 3).
// DECISION: `changes_no_replay` makes a row identical in every column a
// Conflict too, so a replayed batch of kinds the partial indexes do not
// cover (SeenDone, Event, Amount*) is refused as `append` documents. Two
// identical facts at the same instant are never two real facts. COALESCE
// because NULLs are distinct in a unique index; -1 never occurs as a
// stored value of these columns (ids and cents are unsigned, `sub` is a
// small tag), and `kind` fixes which columns are NULL anyway.
// Triggers forbid UPDATE and DELETE on it (DB-2). `deadlines` holds the current recorded deadline per
// occurrence: the one value DB-2 lets change, and only to an earlier one
// (APP-7).
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS changes (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    kind       INTEGER NOT NULL,
    at         INTEGER NOT NULL,
    occurrence INTEGER,
    debt       INTEGER,
    task       INTEGER,
    witness    INTEGER,
    amount     INTEGER,
    due        INTEGER,
    sub        INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS changes_once_per_occurrence
    ON changes (kind, occurrence) WHERE kind IN (0, 3, 4);
CREATE UNIQUE INDEX IF NOT EXISTS changes_once_per_debt
    ON changes (kind, debt) WHERE kind IN (4, 5, 6, 7);
CREATE UNIQUE INDEX IF NOT EXISTS changes_no_replay
    ON changes (kind, at, COALESCE(occurrence, -1), COALESCE(debt, -1),
                COALESCE(task, -1), COALESCE(witness, -1), COALESCE(amount, -1),
                COALESCE(due, -1), COALESCE(sub, -1));
CREATE TRIGGER IF NOT EXISTS changes_no_update BEFORE UPDATE ON changes
    BEGIN SELECT RAISE(ABORT, 'changes are insert-only (DB-2)'); END;
CREATE TRIGGER IF NOT EXISTS changes_no_delete BEFORE DELETE ON changes
    BEGIN SELECT RAISE(ABORT, 'changes are insert-only (DB-2)'); END;

CREATE TABLE IF NOT EXISTS deadlines (
    occurrence INTEGER PRIMARY KEY,
    deadline   INTEGER NOT NULL
);
CREATE TRIGGER IF NOT EXISTS deadlines_only_earlier BEFORE UPDATE ON deadlines
    WHEN NEW.occurrence IS NOT OLD.occurrence OR NEW.deadline >= OLD.deadline
    BEGIN SELECT RAISE(ABORT, 'a recorded deadline only moves earlier (APP-7)'); END;
CREATE TRIGGER IF NOT EXISTS deadlines_no_delete BEFORE DELETE ON deadlines
    BEGIN SELECT RAISE(ABORT, 'recorded deadlines are never deleted (DB-2)'); END;

CREATE TABLE IF NOT EXISTS accounts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    email         TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    lang          TEXT NOT NULL,
    role          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tokens (
    secret    TEXT PRIMARY KEY,
    kind      INTEGER NOT NULL,
    account   INTEGER,
    issued_at INTEGER NOT NULL,
    used      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS subscriptions (
    endpoint TEXT PRIMARY KEY,
    account  INTEGER NOT NULL,
    p256dh   TEXT NOT NULL,
    auth     TEXT NOT NULL
);
";

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh path in the system temp dir, removed on drop.
    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new(tag: &str) -> TempFile {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let name = format!("atomanja-store-{tag}-{}-{n}.sqlite", std::process::id());
            let path = std::env::temp_dir().join(name);
            let _ = std::fs::remove_file(&path);
            TempFile(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let mut p = self.0.clone().into_os_string();
                p.push(suffix);
                let _ = std::fs::remove_file(p);
            }
        }
    }

    fn tables<K: Kind>(store: &Store<K>) -> Vec<String> {
        let mut stmt = store
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    // store-01
    #[test]
    fn db_1_creates_schema_in_new_file() {
        let file = TempFile::new("db1");
        assert!(!file.0.exists());
        let store = Store::<LedgerDb>::open(&file.0).unwrap();
        assert!(file.0.exists());
        let names = tables(&store);
        for t in [
            "accounts",
            "changes",
            "deadlines",
            "subscriptions",
            "tokens",
        ] {
            assert!(names.iter().any(|n| n == t), "missing table {t}: {names:?}");
        }
        drop(store);
        // Opening again keeps the schema (idempotent creation).
        let again = Store::<LedgerDb>::open(&file.0).unwrap();
        assert_eq!(tables(&again), names);
    }

    #[test]
    fn dev_1_dev_handle_reports_dev_database() {
        let dev = Store::<DevDb>::open_in_memory().unwrap();
        assert_eq!(dev.database(), Database::Dev);
        let ledger = Store::<LedgerDb>::open_in_memory().unwrap();
        assert_eq!(ledger.database(), Database::Ledger);
        let debug = format!("{dev:?}");
        assert!(debug.contains("Dev"), "{debug}");
    }

    #[test]
    fn dev_3_dev_handle_not_accepted_where_ledger_required() {
        // The compile-time half is the `compile_fail` doc test in the
        // module docs. Here: the two handle types are distinct types.
        use std::any::TypeId;
        fn ledger_only(store: &Store<LedgerDb>) -> Database {
            store.database()
        }
        assert_ne!(
            TypeId::of::<Store<LedgerDb>>(),
            TypeId::of::<Store<DevDb>>()
        );
        let ledger = Store::<LedgerDb>::open_in_memory().unwrap();
        assert_eq!(ledger_only(&ledger), Database::Ledger);
    }

    // store-02
    use crate::ledger::{
        Centimes, DebtId, EventKind, Observation, OccurrenceId, Outcome, RepeatMode, Snapshot,
        TaskId, Timestamp,
    };

    fn recorded(occ: u32, task: i64, deadline: i64, at: i64) -> Change {
        Change::Recorded {
            occurrence: OccurrenceId(occ),
            task: TaskId(task),
            deadline: Timestamp(deadline),
            at: Timestamp(at),
        }
    }

    fn event(occ: u32, kind: EventKind, new_due: Option<i64>, at: i64) -> Change {
        Change::Event {
            occurrence: OccurrenceId(occ),
            kind,
            new_due: new_due.map(Timestamp),
            at: Timestamp(at),
        }
    }

    fn debt_created(debt: u32, occ: u32, at: i64) -> Change {
        Change::DebtCreated {
            debt: DebtId(debt),
            occurrence: OccurrenceId(occ),
            amount: Centimes::DEFAULT_AMOUNT,
            at: Timestamp(at),
        }
    }

    fn deadline_row<K: Kind>(store: &Store<K>, occ: u32) -> Option<i64> {
        use rusqlite::OptionalExtension;
        store
            .conn
            .query_row(
                "SELECT deadline FROM deadlines WHERE occurrence = ?1",
                [occ],
                |r| r.get(0),
            )
            .optional()
            .unwrap()
    }

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

    /// Persist then apply, the way `sync` and `web` will.
    fn commit(store: &mut Store<LedgerDb>, live: &mut Ledger, changes: &[Change]) {
        store.append(changes).unwrap();
        for c in changes {
            live.apply(c);
        }
    }

    #[test]
    fn db_2_ledger_rows_insert_only() {
        let mut store = Store::<LedgerDb>::open_in_memory().unwrap();
        let first = vec![recorded(0, 1, 1000, 10)];
        let second = vec![event(0, EventKind::MovedLater, Some(2000), 20)];
        store.append(&first).unwrap();
        store.append(&second).unwrap();
        let before = store.load_changes().unwrap();
        assert_eq!(before, [first, second].concat());

        assert!(store.conn.execute("UPDATE changes SET at = 0", []).is_err());
        assert!(store.conn.execute("DELETE FROM changes", []).is_err());
        assert_eq!(store.load_changes().unwrap(), before);
    }

    #[test]
    fn db_2_only_app_7_updates_recorded_deadline() {
        let mut store = Store::<LedgerDb>::open_in_memory().unwrap();
        let rec = recorded(0, 1, 1000, 10);
        store.append(std::slice::from_ref(&rec)).unwrap();
        assert_eq!(deadline_row(&store, 0), Some(1000));

        store
            .append(&[event(0, EventKind::MovedLater, Some(1500), 20)])
            .unwrap();
        assert_eq!(deadline_row(&store, 0), Some(1000));

        store
            .append(&[event(0, EventKind::MovedEarlier, Some(800), 30)])
            .unwrap();
        assert_eq!(deadline_row(&store, 0), Some(800));
        // The Recorded fact itself is untouched.
        assert_eq!(store.load_changes().unwrap()[0], rec);
        assert_eq!(
            store.load_ledger().unwrap().occurrences().deadline,
            vec![Timestamp(800)]
        );

        // Never later, never deleted, even by hand.
        assert!(
            store
                .conn
                .execute("UPDATE deadlines SET deadline = 900", [])
                .is_err()
        );
        assert!(store.conn.execute("DELETE FROM deadlines", []).is_err());

        // A decided occurrence keeps its deadline (APP-7, APP-10).
        store
            .append(&[Change::Decided {
                occurrence: OccurrenceId(0),
                outcome: Outcome::Met,
                at: Timestamp(40),
            }])
            .unwrap();
        store
            .append(&[event(0, EventKind::MovedEarlier, Some(500), 50)])
            .unwrap();
        assert_eq!(deadline_row(&store, 0), Some(800));
        assert_eq!(
            store.load_ledger().unwrap().occurrences().deadline,
            vec![Timestamp(800)]
        );
    }

    #[test]
    fn db_3_load_after_restart_equals_state_before() {
        let file = TempFile::new("db3");
        let w1 = WitnessId(1);
        let w2 = WitnessId(2);
        let witnesses = [w1, w2];
        let mut live = Ledger::new();
        let mut all = Vec::new();
        {
            let mut store = Store::<LedgerDb>::open(&file.0).unwrap();
            let poll = |store: &mut Store<LedgerDb>,
                        live: &mut Ledger,
                        all: &mut Vec<Change>,
                        tasks,
                        now| {
                let changes = live.observe(&Snapshot { tasks }, Timestamp(now), &witnesses);
                commit(store, live, &changes);
                all.extend(changes);
            };
            poll(
                &mut store,
                &mut live,
                &mut all,
                vec![obs(1, Some(100)), obs(2, Some(200)), obs(3, Some(300))],
                0,
            );
            let mut t2_done = obs(2, Some(200));
            t2_done.done = true;
            t2_done.done_at = Some(Timestamp(50));
            poll(
                &mut store,
                &mut live,
                &mut all,
                vec![obs(1, Some(80)), t2_done.clone(), obs(3, Some(300))],
                50,
            );
            poll(
                &mut store,
                &mut live,
                &mut all,
                vec![obs(1, Some(80)), t2_done.clone(), obs(3, Some(300))],
                150,
            );
            assert_eq!(live.debts().occurrence.len(), 1);

            let mut manual = Vec::new();
            manual.push(
                live.claim(DebtId(0), w1, &witnesses, Timestamp(160))
                    .unwrap(),
            );
            commit(&mut store, &mut live, &manual[manual.len() - 1..]);
            manual.push(live.mark_sent(DebtId(0), Timestamp(170)).unwrap());
            commit(&mut store, &mut live, &manual[manual.len() - 1..]);
            manual.push(live.propose_amount(Centimes(2000), Timestamp(175)).unwrap());
            commit(&mut store, &mut live, &manual[manual.len() - 1..]);
            for w in witnesses {
                let changes = live.approve_amount(w, &witnesses, Timestamp(180)).unwrap();
                commit(&mut store, &mut live, &changes);
                manual.extend(changes);
            }
            manual.push(live.mark_received(DebtId(0), w1, Timestamp(190)).unwrap());
            commit(&mut store, &mut live, &manual[manual.len() - 1..]);
            all.extend(manual);

            poll(
                &mut store,
                &mut live,
                &mut all,
                vec![obs(1, Some(80)), t2_done.clone(), obs(3, None)],
                250,
            );
            // Due date set back to the recorded deadline: EventKind::Restored.
            poll(
                &mut store,
                &mut live,
                &mut all,
                vec![obs(1, Some(80)), t2_done.clone(), obs(3, Some(300))],
                260,
            );
            assert!(all.iter().any(|c| matches!(
                c,
                Change::Event { kind: EventKind::Restored, .. }
            )));
            poll(
                &mut store,
                &mut live,
                &mut all,
                vec![obs(1, Some(80)), t2_done],
                400,
            );
            assert_eq!(live.amount(), Centimes(2000));
        }
        // Restart: a new handle on the same file.
        let store = Store::<LedgerDb>::open(&file.0).unwrap();
        assert_eq!(store.load_changes().unwrap(), all);
        let loaded = store.load_ledger().unwrap();
        assert_eq!(format!("{loaded:?}"), format!("{live:?}"));
        assert_eq!(loaded.totals(), live.totals());
    }

    #[test]
    fn store_poll_changes_appended_in_one_transaction() {
        let mut store = Store::<LedgerDb>::open_in_memory().unwrap();
        let ok = vec![
            recorded(0, 1, 1000, 10),
            Change::Decided {
                occurrence: OccurrenceId(0),
                outcome: Outcome::Late,
                at: Timestamp(1100),
            },
            debt_created(0, 0, 1100),
        ];
        store.append(&ok).unwrap();
        assert_eq!(store.load_changes().unwrap(), ok);

        // The last change of this batch conflicts: nothing of it is kept,
        // not even the deadline update of the MovedEarlier in the middle.
        let bad = vec![
            recorded(1, 2, 5000, 1200),
            event(1, EventKind::MovedEarlier, Some(4000), 1300),
            debt_created(0, 1, 1300),
        ];
        assert!(matches!(store.append(&bad), Err(StoreError::Conflict)));
        assert_eq!(store.load_changes().unwrap(), ok);
        assert_eq!(deadline_row(&store, 1), None);

        // The handle is still usable after the rollback.
        store.append(&bad[..2]).unwrap();
        assert_eq!(deadline_row(&store, 1), Some(4000));
        assert_eq!(store.load_changes().unwrap().len(), 5);
    }

    #[test]
    fn store_reapplying_same_poll_no_duplicate_debt() {
        let mut store = Store::<LedgerDb>::open_in_memory().unwrap();
        let mut live = Ledger::new();
        let witnesses = [WitnessId(1), WitnessId(2)];
        let first = live.observe(
            &Snapshot {
                tasks: vec![obs(1, Some(100))],
            },
            Timestamp(0),
            &witnesses,
        );
        commit(&mut store, &mut live, &first);
        let late = live.observe(
            &Snapshot {
                tasks: vec![obs(1, Some(100))],
            },
            Timestamp(200),
            &witnesses,
        );
        assert!(late.iter().any(|c| matches!(c, Change::DebtCreated { .. })));
        store.append(&late).unwrap();

        // Crash before apply, restart, same poll persisted again.
        assert!(matches!(store.append(&late), Err(StoreError::Conflict)));
        // A second debt for the same occurrence under a new id (APP-12).
        assert!(matches!(
            store.append(&[debt_created(1, 0, 300)]),
            Err(StoreError::Conflict)
        ));
        let loaded = store.load_ledger().unwrap();
        assert_eq!(loaded.debts().occurrence.len(), 1);
        assert_eq!(store.load_changes().unwrap(), [first, late].concat());
    }

    /// Review regression: a replayed batch with no per-occurrence or
    /// per-debt fact (a poll that only saw done flags or deadline events,
    /// or amount changes) must also be refused, as `append` documents.
    #[test]
    fn store_replayed_batch_without_unique_kinds_conflicts() {
        let mut store = Store::<LedgerDb>::open_in_memory().unwrap();
        store.append(&[recorded(0, 1, 1000, 10)]).unwrap();
        let poll = vec![
            Change::SeenDone {
                occurrence: OccurrenceId(0),
                done: true,
                at: Timestamp(20),
            },
            event(0, EventKind::MovedLater, Some(2000), 20),
            event(0, EventKind::Removed, None, 30),
        ];
        store.append(&poll).unwrap();
        assert!(matches!(store.append(&poll), Err(StoreError::Conflict)));
        // Each single row of the batch is refused on its own too.
        for row in &poll {
            assert!(matches!(
                store.append(std::slice::from_ref(row)),
                Err(StoreError::Conflict)
            ));
        }

        let amounts = vec![
            Change::AmountProposed {
                amount: Centimes(700),
                at: Timestamp(40),
            },
            Change::AmountApproved {
                witness: WitnessId(1),
                at: Timestamp(41),
            },
            Change::AmountInForce {
                amount: Centimes(700),
                at: Timestamp(42),
            },
        ];
        store.append(&amounts).unwrap();
        assert!(matches!(store.append(&amounts), Err(StoreError::Conflict)));

        // Same kind and occurrence at a later poll is a new fact, accepted.
        store
            .append(&[event(0, EventKind::MovedLater, Some(2000), 50)])
            .unwrap();
        assert_eq!(store.load_changes().unwrap().len(), 1 + 3 + 3 + 1);
    }

    // store-03

    fn hash(s: &str) -> PasswordHash {
        PasswordHash(format!("$argon2id$v=19$fake${s}"))
    }

    #[test]
    fn store_concurrent_claims_one_row() {
        let file = TempFile::new("claims");
        let witnesses = [WitnessId(1), WitnessId(2)];
        {
            let mut store = Store::<LedgerDb>::open(&file.0).unwrap();
            store
                .append(&[
                    recorded(0, 1, 100, 0),
                    Change::Decided {
                        occurrence: OccurrenceId(0),
                        outcome: Outcome::Late,
                        at: Timestamp(200),
                    },
                    debt_created(0, 0, 200),
                ])
                .unwrap();
        }
        // Two requests, two connections, same unclaimed debt, same moment.
        let barrier = std::sync::Barrier::new(2);
        let results: Vec<Result<(), StoreError>> = std::thread::scope(|s| {
            let handles: Vec<_> = witnesses
                .iter()
                .map(|&w| {
                    let (path, barrier) = (&file.0, &barrier);
                    s.spawn(move || {
                        let mut store = Store::<LedgerDb>::open(path).unwrap();
                        let ledger = store.load_ledger().unwrap();
                        let claim = ledger
                            .claim(DebtId(0), w, &witnesses, Timestamp(300))
                            .unwrap();
                        barrier.wait();
                        store.append(&[claim])
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "{results:?}"
        );
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(StoreError::Conflict)))
                .count(),
            1,
            "{results:?}"
        );
        let store = Store::<LedgerDb>::open(&file.0).unwrap();
        let claims = store
            .load_changes()
            .unwrap()
            .into_iter()
            .filter(|c| matches!(c, Change::Claimed { .. }))
            .count();
        assert_eq!(claims, 1);
        assert!(store.load_ledger().unwrap().debts().owner[0].is_some());
    }

    fn account_crud<K: Kind>() {
        let mut store = Store::<K>::open_in_memory().unwrap();
        let worker = store
            .insert_account("w@example.org", &hash("w"), Role::Worker)
            .unwrap();
        let witness = store
            .insert_account("a@example.org", &hash("a"), Role::Witness)
            .unwrap();
        let admin = store
            .insert_account("admin@example.org", &hash("x"), Role::Admin)
            .unwrap();
        assert_ne!(worker.id, witness.id);
        assert_eq!(worker.email, "w@example.org");
        assert_eq!(worker.password_hash, hash("w"));
        assert_eq!(worker.role, Role::Worker);

        assert_eq!(store.account(witness.id).unwrap(), Some(witness.clone()));
        assert_eq!(
            store.account_by_email("admin@example.org").unwrap(),
            Some(admin.clone())
        );
        assert_eq!(store.account_by_email("ADMIN@example.org").unwrap(), None);
        assert_eq!(store.account(AccountId(999)).unwrap(), None);
        assert_eq!(
            store.accounts().unwrap(),
            vec![worker.clone(), witness.clone(), admin.clone()]
        );
        assert_eq!(
            store.real_witnesses().unwrap(),
            vec![WitnessId(witness.id.0)]
        );

        assert!(matches!(
            store.insert_account("a@example.org", &hash("b"), Role::Witness),
            Err(StoreError::Conflict)
        ));
        assert!(matches!(
            store.set_email(worker.id, "a@example.org"),
            Err(StoreError::Conflict)
        ));
        store.set_email(worker.id, "new@example.org").unwrap();
        store.set_password(worker.id, &hash("new")).unwrap();
        let w = store.account(worker.id).unwrap().unwrap();
        assert_eq!(w.email, "new@example.org");
        assert_eq!(w.password_hash, hash("new"));
        assert_eq!(store.account_by_email("w@example.org").unwrap(), None);

        let ghost = AccountId(999);
        assert!(matches!(
            store.set_email(ghost, "g@example.org"),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.set_password(ghost, &hash("g")),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.set_lang(ghost, Lang::En),
            Err(StoreError::NotFound)
        ));
        assert_eq!(store.accounts().unwrap().len(), 3);
    }

    #[test]
    fn store_account_crud() {
        account_crud::<LedgerDb>();
        account_crud::<DevDb>();
    }

    fn single_use_token<K: Kind>() {
        let mut store = Store::<K>::open_in_memory().unwrap();
        let issued_at = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
            + time::Duration::nanoseconds(123_456_789);
        let reset = IssuedToken {
            token: Token("reset-secret".into()),
            kind: TokenKind::Reset,
            account: Some(AccountId(4)),
            issued_at,
            used: false,
        };
        let invite = IssuedToken {
            token: Token("invite-secret".into()),
            kind: TokenKind::Invite,
            account: None,
            issued_at,
            used: false,
        };
        store.insert_token(&reset).unwrap();
        store.insert_token(&invite).unwrap();
        assert!(matches!(
            store.insert_token(&reset),
            Err(StoreError::Conflict)
        ));
        assert_eq!(store.token(&reset.token).unwrap(), Some(reset.clone()));
        assert_eq!(store.token(&invite.token).unwrap(), Some(invite.clone()));
        assert_eq!(store.token(&Token("nope".into())).unwrap(), None);

        assert!(store.use_token(&reset.token).unwrap());
        assert!(!store.use_token(&reset.token).unwrap());
        let used = store.token(&reset.token).unwrap().unwrap();
        assert!(used.used);
        assert!(used.check(issued_at).is_err());
        // The other token is untouched.
        assert!(!store.token(&invite.token).unwrap().unwrap().used);
        assert!(matches!(
            store.use_token(&Token("nope".into())),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn store_single_use_token_used_once() {
        single_use_token::<LedgerDb>();
        single_use_token::<DevDb>();
    }

    fn sub(account: i64, endpoint: &str, key: &str) -> StoredSubscription {
        StoredSubscription {
            account: AccountId(account),
            endpoint: endpoint.into(),
            p256dh: format!("p256-{key}"),
            auth: format!("auth-{key}"),
        }
    }

    fn push_subscriptions<K: Kind>() {
        let mut store = Store::<K>::open_in_memory().unwrap();
        let phone = sub(1, "https://push.example/phone", "1");
        let laptop = sub(1, "https://push.example/laptop", "2");
        let other = sub(2, "https://push.example/other", "3");
        for s in [&phone, &laptop, &other] {
            store.upsert_subscription(s).unwrap();
        }
        let mut mine = store.subscriptions(AccountId(1)).unwrap();
        mine.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
        assert_eq!(mine, vec![laptop.clone(), phone.clone()]);
        assert_eq!(
            store.subscriptions(AccountId(2)).unwrap(),
            vec![other.clone()]
        );

        // Same device re-subscribes under another account with new keys.
        let moved = sub(2, "https://push.example/phone", "9");
        store.upsert_subscription(&moved).unwrap();
        assert_eq!(
            store.subscriptions(AccountId(1)).unwrap(),
            vec![laptop.clone()]
        );
        let mut theirs = store.subscriptions(AccountId(2)).unwrap();
        theirs.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
        assert_eq!(theirs, vec![other.clone(), moved.clone()]);

        store.remove_subscription(&laptop.endpoint).unwrap();
        store
            .remove_subscription("https://push.example/unknown")
            .unwrap();
        assert!(store.subscriptions(AccountId(1)).unwrap().is_empty());
        assert_eq!(store.subscriptions(AccountId(3)).unwrap(), vec![]);

        let debug = format!("{moved:?}");
        assert!(
            !debug.contains("p256-9") && !debug.contains("auth-9"),
            "{debug}"
        );
    }

    #[test]
    fn store_push_subscriptions_per_device() {
        push_subscriptions::<LedgerDb>();
        push_subscriptions::<DevDb>();
    }

    fn language_persists<K: Kind>() {
        let file = TempFile::new("lang");
        let id = {
            let mut store = Store::<K>::open(&file.0).unwrap();
            let a = store
                .insert_account("a@example.org", &hash("a"), Role::Witness)
                .unwrap();
            assert_eq!(a.lang, Lang::Fr);
            assert_eq!(store.account(a.id).unwrap().unwrap().lang, Lang::Fr);
            store.set_lang(a.id, Lang::En).unwrap();
            a.id
        };
        let store = Store::<K>::open(&file.0).unwrap();
        assert_eq!(store.account(id).unwrap().unwrap().lang, Lang::En);
    }

    #[test]
    fn app_39_account_language_persists_default_french() {
        language_persists::<LedgerDb>();
        language_persists::<DevDb>();
    }
}
