//! Pure account logic: roles, password hashing, invite / reset /
//! session tokens and permission checks. No I/O.
//!
//! Uses: config (Lang only).
//! SRS: APP-20, APP-25..APP-30.
//!
//! `store` persists [`Account`] and token rows; `web` asks this module
//! whether an account may act, and which database its requests go to.

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use time::{Duration, OffsetDateTime};

/// Account id, as stored. A witness's id is also its
/// `ledger::WitnessId` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(pub i64);

/// What an account is (APP-20, APP-25, APP-30).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The single worker (APP-25).
    Worker = 0,
    /// A real witness, created from an invite (APP-26).
    Witness = 1,
    /// The admin witness: dev data only, never a real witness (APP-20, APP-30).
    Admin = 2,
}

impl Role {
    /// `true` only for [`Role::Witness`]. The admin is never a real
    /// witness (APP-20).
    pub fn is_real_witness(self) -> bool {
        self == Role::Witness
    }

    /// Which database this role's requests use (APP-30, DEV-3).
    pub fn database(self) -> Database {
        todo!()
    }
}

/// The two `SQLite` files (DB-1, DEV-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Database {
    /// Real data. Worker and witnesses.
    Ledger,
    /// Fake data. Admin witness only.
    Dev,
}

/// An argon2 hash in PHC string form. Never the plain password (APP-27).
#[derive(Clone, PartialEq, Eq)]
pub struct PasswordHash(pub String);

/// An account as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// Id.
    pub id: AccountId,
    /// Login and mail address. Changed only by its owner (APP-29).
    pub email: String,
    /// Password hash (APP-27).
    pub password_hash: PasswordHash,
    /// Pages, mails and notifications language (APP-39).
    pub lang: crate::config::Lang,
    /// Role.
    pub role: Role,
}

/// Minimum password length, in characters.
pub const MIN_PASSWORD_LEN: usize = 8;

/// Hash a new password with argon2 and a random salt (APP-27).
///
/// # Errors
///
/// `PasswordTooShort`, `Hashing`.
pub fn hash_password(plain: &str) -> Result<PasswordHash, AuthError> {
    if plain.chars().count() < MIN_PASSWORD_LEN {
        return Err(AuthError::PasswordTooShort);
    }
    // DECISION: argon2 default params (Argon2id v19); salt from the
    // crate's OS-RNG path, so no extra rand plumbing here.
    let phc: argon2::password_hash::phc::PasswordHash = argon2::Argon2::default()
        .hash_password(plain.as_bytes())
        .map_err(|_| AuthError::Hashing)?;
    Ok(PasswordHash(phc.to_string()))
}

/// `true` if `plain` matches `hash` (APP-27).
pub fn verify_password(hash: &PasswordHash, plain: &str) -> bool {
    // DECISION: a malformed stored hash verifies as `false`, never panics.
    argon2::Argon2::default()
        .verify_password(plain.as_bytes(), hash.0.as_str())
        .is_ok()
}

/// What a token is for. Each kind has its own lifetime.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Witness invite, 7 days, single use (APP-26).
    Invite = 0,
    /// Password reset, 1 hour, single use (APP-28).
    Reset = 1,
    /// Login session cookie, 30 days, reusable until logout.
    Session = 2,
}

impl TokenKind {
    /// How long a token of this kind stays valid after it is issued.
    pub fn lifetime(self) -> Duration {
        todo!()
    }

    /// `true` for invite and reset tokens (APP-26, APP-28).
    pub fn single_use(self) -> bool {
        todo!()
    }
}

/// A random secret: 32 bytes from the OS RNG, URL-safe text form.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Token(pub String);

impl Token {
    /// A fresh unguessable token (Review Focus 5).
    pub fn generate() -> Token {
        todo!()
    }
}

/// A stored token row, as `store` loads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuedToken {
    /// The secret.
    pub token: Token,
    /// What it is for.
    pub kind: TokenKind,
    /// The account it belongs to; `None` for an invite (no account yet).
    pub account: Option<AccountId>,
    /// Issue time.
    pub issued_at: OffsetDateTime,
    /// Already consumed (single-use kinds) or logged out (session).
    pub used: bool,
}

impl IssuedToken {
    /// `Ok` if the token may be used at `now`: not used, not expired
    /// (APP-26, APP-28, Review Focus 5).
    ///
    /// # Errors
    ///
    /// `AlreadyUsed`, `Expired`.
    pub fn check(&self, now: OffsetDateTime) -> Result<(), AuthError> {
        let _ = now;
        todo!()
    }
}

/// Things an account may try to do. Debt actions go to the database of
/// the actor's role (see [`Role::database`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// See the worker view (titles included).
    ViewWorker,
    /// See the witness view (no titles, APP-32).
    ViewWitness,
    /// Claim a debt (APP-18).
    Claim,
    /// Set a sent mark (APP-21).
    MarkSent,
    /// Set a received mark (APP-22; ownership checked by `ledger`).
    MarkReceived,
    /// Propose a new amount (APP-16).
    ProposeAmount,
    /// Approve a pending amount (APP-16).
    ApproveAmount,
    /// Create a witness invite (APP-26).
    CreateInvite,
    /// Change the email of this account (APP-29).
    ChangeEmail(AccountId),
    /// Set the password of this account (APP-27).
    SetPassword(AccountId),
}

/// `true` if `actor` may do `action` (APP-20, APP-21, APP-27, APP-29).
pub fn permits(actor: &Account, action: Action) -> bool {
    let _ = (actor, action);
    todo!()
}

/// `Ok` if a worker account may be created: none exists yet (APP-25).
///
/// # Errors
///
/// `WorkerExists`.
pub fn may_create_worker(existing: &[Role]) -> Result<(), AuthError> {
    if existing.contains(&Role::Worker) {
        Err(AuthError::WorkerExists)
    } else {
        Ok(())
    }
}

/// Why an auth operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// Password shorter than [`MIN_PASSWORD_LEN`].
    PasswordTooShort,
    /// The hashing library failed.
    Hashing,
    /// Token past its lifetime.
    Expired,
    /// Single-use token already consumed, or session logged out.
    AlreadyUsed,
    /// A worker account already exists (APP-25).
    WorkerExists,
}

impl std::fmt::Debug for PasswordHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PasswordHash(<redacted>)")
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::error::Error for AuthError {}

#[cfg(test)]
mod tests {
    use super::*;

    // auth-01
    #[test]
    fn app_20_admin_is_not_real_witness() {
        assert!(!Role::Admin.is_real_witness());
        assert!(!Role::Worker.is_real_witness());
        assert!(Role::Witness.is_real_witness());
    }

    #[test]
    fn app_25_exactly_one_worker_allowed() {
        assert_eq!(may_create_worker(&[]), Ok(()));
        assert_eq!(may_create_worker(&[Role::Witness, Role::Admin]), Ok(()));
        assert_eq!(
            may_create_worker(&[Role::Witness, Role::Worker]),
            Err(AuthError::WorkerExists)
        );
    }

    // auth-02
    #[test]
    fn app_27_password_hash_verifies_only_same_password() {
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password(&hash, "correct horse"));
        assert!(!verify_password(&hash, "correct hors"));
        assert!(!verify_password(&hash, "Correct horse"));
        assert!(!verify_password(&PasswordHash("not a phc".into()), "correct horse"));
    }

    #[test]
    fn app_27_hash_never_equals_plaintext() {
        let plain = "correct horse";
        let a = hash_password(plain).unwrap();
        let b = hash_password(plain).unwrap();
        assert_ne!(a.0, plain);
        assert!(!a.0.contains(plain));
        assert!(a.0.starts_with("$argon2"));
        // Random salt: same password, different hashes.
        assert_ne!(a, b);
        // Debug never prints the hash.
        assert!(!format!("{a:?}").contains(&a.0));
    }

    #[test]
    fn auth_short_password_rejected() {
        assert_eq!(hash_password("1234567"), Err(AuthError::PasswordTooShort));
        // Length counts characters, not bytes.
        assert_eq!(hash_password("ééééééé"), Err(AuthError::PasswordTooShort));
        assert!(hash_password("12345678").is_ok());
    }

    // auth-03
    #[test] fn app_26_invite_single_use() { todo!() }
    #[test] fn app_26_invite_expires_after_7_days() { todo!() }
    #[test] fn app_28_reset_single_use() { todo!() }
    #[test] fn app_28_reset_expires_after_1_hour() { todo!() }
    #[test] fn auth_tokens_are_unique_and_unguessable_length() { todo!() }

    // auth-04
    #[test] fn app_27_worker_cannot_set_witness_password() { todo!() }
    #[test] fn app_29_only_owner_changes_email() { todo!() }
    #[test] fn app_30_admin_targets_dev_db() { todo!() }
    #[test] fn dev_3_worker_and_witness_target_ledger_db() { todo!() }
}
