//! Pure account logic: roles, password hashing, invite / reset /
//! session tokens and permission checks. No I/O.
//!
//! Uses: config (Lang only).
//! SRS: APP-20, APP-25..APP-30.
//!
//! `store` persists [`Account`] and token rows; `web` asks this module
//! whether an account may act, and which database its requests go to.

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use rand::TryRng;
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
        match self {
            TokenKind::Invite => Duration::days(7),
            TokenKind::Reset => Duration::hours(1),
            TokenKind::Session => Duration::days(30),
        }
    }

    /// `true` for invite and reset tokens (APP-26, APP-28).
    pub fn single_use(self) -> bool {
        matches!(self, TokenKind::Invite | TokenKind::Reset)
    }
}

/// A random secret: 32 bytes from the OS RNG, URL-safe text form.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Token(pub String);

impl Token {
    /// A fresh unguessable token (Review Focus 5).
    ///
    /// # Panics
    ///
    /// If the OS RNG fails. A weak token is never returned instead.
    pub fn generate() -> Token {
        let mut bytes = [0u8; TOKEN_BYTES];
        // DECISION: read straight from the OS RNG (SysRng) as the contract
        // says, and panic on failure rather than fall back to anything weaker.
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .expect("OS random number generator failed");
        Token(base64url(&bytes))
    }
}

/// Random bytes per token (Review Focus 5: at least 32).
const TOKEN_BYTES: usize = 32;

/// Unpadded base64url (RFC 4648 section 5) of `bytes`.
// DECISION: hand-rolled, about 15 lines, instead of adding a base64 crate.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        // 1 input byte gives 2 chars, 2 give 3, 3 give 4.
        for i in 0..=chunk.len() {
            let idx = (n >> (18 - 6 * i)) & 0x3f;
            out.push(char::from(ALPHABET[idx as usize]));
        }
    }
    out
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
        if self.used {
            return Err(AuthError::AlreadyUsed);
        }
        // DECISION: valid on [issued_at, issued_at + lifetime); the exact
        // end instant is already expired.
        if now >= self.issued_at + self.kind.lifetime() {
            return Err(AuthError::Expired);
        }
        Ok(())
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
        f.write_str("Token(<redacted>)")
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
    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn issued(kind: TokenKind, used: bool) -> IssuedToken {
        IssuedToken {
            token: Token::generate(),
            kind,
            account: match kind {
                TokenKind::Invite => None,
                _ => Some(AccountId(1)),
            },
            issued_at: t0(),
            used,
        }
    }

    #[test]
    fn app_26_invite_single_use() {
        assert!(TokenKind::Invite.single_use());
        assert_eq!(issued(TokenKind::Invite, false).check(t0()), Ok(()));
        assert_eq!(
            issued(TokenKind::Invite, true).check(t0()),
            Err(AuthError::AlreadyUsed)
        );
    }

    #[test]
    fn app_26_invite_expires_after_7_days() {
        assert_eq!(TokenKind::Invite.lifetime(), Duration::days(7));
        let tok = issued(TokenKind::Invite, false);
        let edge = t0() + Duration::days(7);
        assert_eq!(tok.check(edge - Duration::seconds(1)), Ok(()));
        assert_eq!(tok.check(edge), Err(AuthError::Expired));
        assert_eq!(tok.check(edge + Duration::days(1)), Err(AuthError::Expired));
    }

    #[test]
    fn app_28_reset_single_use() {
        assert!(TokenKind::Reset.single_use());
        assert!(!TokenKind::Session.single_use());
        assert_eq!(issued(TokenKind::Reset, false).check(t0()), Ok(()));
        assert_eq!(
            issued(TokenKind::Reset, true).check(t0()),
            Err(AuthError::AlreadyUsed)
        );
        // A logged-out session is refused too.
        assert_eq!(
            issued(TokenKind::Session, true).check(t0()),
            Err(AuthError::AlreadyUsed)
        );
    }

    #[test]
    fn app_28_reset_expires_after_1_hour() {
        assert_eq!(TokenKind::Reset.lifetime(), Duration::hours(1));
        assert_eq!(TokenKind::Session.lifetime(), Duration::days(30));
        let tok = issued(TokenKind::Reset, false);
        let edge = t0() + Duration::hours(1);
        assert_eq!(tok.check(edge - Duration::seconds(1)), Ok(()));
        assert_eq!(tok.check(edge), Err(AuthError::Expired));
    }

    #[test]
    fn auth_tokens_are_unique_and_unguessable_length() {
        let tokens: Vec<Token> = (0..200).map(|_| Token::generate()).collect();
        let distinct: std::collections::HashSet<&Token> = tokens.iter().collect();
        assert_eq!(distinct.len(), tokens.len());
        for t in &tokens {
            // 32 bytes, unpadded base64url: ceil(32 * 8 / 6) = 43 chars.
            assert_eq!(t.0.len(), 43);
            assert!(
                t.0.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            );
            assert!(!format!("{t:?}").contains(&t.0));
        }
        // Encoder against RFC 4648 vectors, URL-safe alphabet.
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    // auth-04
    #[test] fn app_27_worker_cannot_set_witness_password() { todo!() }
    #[test] fn app_29_only_owner_changes_email() { todo!() }
    #[test] fn app_30_admin_targets_dev_db() { todo!() }
    #[test] fn dev_3_worker_and_witness_target_ledger_db() { todo!() }
}
