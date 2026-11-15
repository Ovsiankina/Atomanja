//! Session cookies, the signed-in actor, and the shared response helpers.
//!
//! DEV-3 (contract `TODO(srs)` proposal): the admin account and its
//! sessions live in the dev file; login and session lookup check the
//! ledger file first, then the dev file. From the dev file only an
//! account with `Role::Admin` is ever accepted, so a fake witness that
//! `devdata` writes there can never sign in and reach the ledger.

use std::sync::{Mutex, MutexGuard};

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use time::OffsetDateTime;

use super::AppState;
use crate::auth::{Account, Database, IssuedToken, Role, Token, TokenKind};
use crate::ledger::Timestamp;
use crate::store::{Kind, Store, StoreError};

/// Name of the session cookie.
pub(super) const COOKIE: &str = "atomanja_session";

/// The signed-in account and where its row and session live.
pub(super) struct Actor {
    pub account: Account,
    /// The file holding the account and its session (not always the file
    /// its requests read: that is `account.role.database()`).
    pub home: Database,
    pub token: Token,
}

// DECISION: a poisoned lock is recovered, as `sync` does: every store
// write is one transaction, so a panic elsewhere leaves no half batch.
/// Lock a store handle.
pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Current UTC time.
pub(super) fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// Current UTC time as a ledger timestamp.
pub(super) fn stamp() -> Timestamp {
    Timestamp(now().unix_timestamp())
}

/// The session secret from the `Cookie` header, if any.
pub(super) fn cookie_token(headers: &HeaderMap) -> Option<Token> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE && !value.is_empty()).then(|| Token(value.to_owned()))
        })
}

/// The account behind a valid session token in `store`, if any.
fn session_in<K: Kind>(
    store: &Store<K>,
    token: &Token,
    at: OffsetDateTime,
) -> Result<Option<Account>, StoreError> {
    let Some(row) = store.token(token)? else {
        return Ok(None);
    };
    if row.kind != TokenKind::Session || row.check(at).is_err() {
        return Ok(None);
    }
    match row.account {
        Some(id) => store.account(id),
        None => Ok(None),
    }
}

/// The signed-in actor for this request, if any (ledger first, then the
/// dev file for the admin only, DEV-3).
pub(super) fn actor(state: &AppState, headers: &HeaderMap) -> Result<Option<Actor>, StoreError> {
    let Some(token) = cookie_token(headers) else {
        return Ok(None);
    };
    let at = now();
    if let Some(account) = session_in(&*lock(&state.ledger), &token, at)? {
        return Ok(Some(Actor {
            account,
            home: Database::Ledger,
            token,
        }));
    }
    let dev = session_in(&*lock(&state.dev), &token, at)?;
    Ok(dev.filter(|a| a.role == Role::Admin).map(|account| Actor {
        account,
        home: Database::Dev,
        token,
    }))
}

/// Store a new session for `account` in `store`; returns the secret.
pub(super) fn open_session<K: Kind>(
    store: &mut Store<K>,
    account: &Account,
) -> Result<Token, StoreError> {
    let token = Token::generate();
    store.insert_token(&IssuedToken {
        token: token.clone(),
        kind: TokenKind::Session,
        account: Some(account.id),
        issued_at: now(),
        used: false,
    })?;
    Ok(token)
}

/// `Set-Cookie` value for a session (or its removal with `None`).
pub(super) fn set_cookie(state: &AppState, token: Option<&Token>) -> HeaderValue {
    // DECISION: HttpOnly + SameSite=Lax. Lax keeps the cookie off
    // cross-site POSTs, which is the CSRF guard for every action form;
    // Secure only when the public URL is https, so local http works.
    let secure = if state.config.base_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let value = match token {
        Some(t) => format!(
            "{COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
            t.0,
            TokenKind::Session.lifetime().whole_seconds()
        ),
        None => format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}"),
    };
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("atomanja_session="))
}

/// `303 See Other` to `to`.
pub(super) fn see_other(to: &str) -> Response {
    let mut r = StatusCode::SEE_OTHER.into_response();
    if let Ok(v) = HeaderValue::from_str(to) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

/// Log a store failure and answer 500 (SRS 5.1: logged, state kept).
pub(super) fn internal(e: &dyn std::fmt::Display) -> Response {
    eprintln!("web: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
}

/// `true` when htmx sent the request (answer with a fragment).
pub(super) fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("HX-Request").is_some_and(|v| v == "true")
}
