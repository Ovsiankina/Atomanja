//! Single-use links: witness invites (APP-26, APP-27) and password
//! resets (APP-28). An expired, used or unknown link gets a page that
//! says so and never signs anyone in (Review Focus 5). Pages reached
//! before login are French (APP-40).

use axum::extract::{Form, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use maud::Markup;
use serde::Deserialize;

use super::handlers::{public_page, require};
use super::i18n::msgs;
use super::pages::{self, AuthCard, Field};
use super::session::{self, internal, lock, now, see_other};
use super::settings::{self, InviteOutcome};
use super::{outbox, AppState};
use crate::auth::{self, Account, AuthError, Database, IssuedToken, Role, Token, TokenKind};
use crate::config::Lang;
use crate::mail;
use crate::store::{Kind, Store, StoreError};

/// Why a link cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkError {
    Unknown,
    Expired,
    Used,
}

/// The stored row of `token` if it is a usable link of `kind`.
pub(super) fn check_link<K: Kind>(
    store: &Store<K>,
    token: &Token,
    kind: TokenKind,
) -> Result<Result<IssuedToken, LinkError>, StoreError> {
    let Some(row) = store.token(token)? else {
        return Ok(Err(LinkError::Unknown));
    };
    if row.kind != kind {
        return Ok(Err(LinkError::Unknown));
    }
    Ok(match row.check(now()) {
        Ok(()) => Ok(row),
        Err(AuthError::AlreadyUsed) => Err(LinkError::Used),
        Err(_) => Err(LinkError::Expired),
    })
}

/// The page for an unusable link: 404 unknown, 410 expired or used.
pub(super) fn link_error(e: LinkError, back: (&str, &str)) -> Response {
    let m = msgs(Lang::Fr);
    let (status, text) = match e {
        LinkError::Unknown => (StatusCode::NOT_FOUND, m.link_unknown),
        LinkError::Expired => (StatusCode::GONE, m.link_expired),
        LinkError::Used => (StatusCode::GONE, m.link_used),
    };
    let card = pages::notice_card(m.link_label, m.link_label, text, back);
    (status, public_page(m.link_label, &card)).into_response()
}

/// The public link for `path` + `token`.
fn link(state: &AppState, path: &str, token: &Token) -> String {
    format!("{}/{path}/{}", state.config.base_url.trim_end_matches('/'), token.0)
}

/// The invite form field.
#[derive(Deserialize)]
pub(super) struct InviteForm {
    #[serde(default)]
    email: String,
}

/// `POST /invite`: the worker creates a 7-day single-use link, shown to
/// copy and emailed when an address is given (APP-26).
pub(super) async fn create_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<InviteForm>,
) -> Response {
    let actor = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return *r,
    };
    let m = msgs(actor.account.lang);
    if !auth::permits(&actor.account, auth::Action::CreateInvite) {
        return (StatusCode::FORBIDDEN, m.err_forbidden).into_response();
    }
    let token = Token::generate();
    let row = IssuedToken {
        token: token.clone(),
        kind: TokenKind::Invite,
        account: None,
        issued_at: now(),
        used: false,
    };
    // Invite is worker-only, so this is always the ledger file (DEV-3).
    if let Err(e) = lock(&state.ledger).insert_token(&row) {
        return internal(&e);
    }
    let url = link(&state, "invite", &token);
    let to = f.email.trim();
    let mailed = if to.is_empty() {
        None
    } else {
        // The invitee has no account yet: French (APP-38, APP-40).
        let sent = outbox::deliver(&state, mail::invite(to, &url, Lang::Fr)).await;
        Some((to.to_owned(), sent))
    };
    let outcome = InviteOutcome { link: url, mailed };
    if session::is_htmx(&headers) {
        let accounts = match lock(&state.ledger).accounts() {
            Ok(a) => a,
            Err(e) => return internal(&e),
        };
        settings::account_panel(m, &accounts, &actor.account, Some(&outcome), None).into_response()
    } else {
        settings::render(&state, &actor.account, Some(&outcome), None)
    }
}

/// The accept form, with an optional error.
fn accept_page(state: &AppState, secret: &str, email: &str, error: Option<&str>) -> Markup {
    let m = msgs(Lang::Fr);
    let inviter = lock(&state.ledger)
        .accounts()
        .ok()
        .and_then(|a| a.into_iter().find(|a| a.role == Role::Worker))
        .map_or_else(|| m.a_worker.to_owned(), |a| a.email);
    let title = m.invite_title(&inviter);
    let action = format!("/invite/{secret}");
    let card = AuthCard {
        action: &action,
        label: m.invitation,
        title: &title,
        big_title: false,
        intro: Some(m.invite_intro),
        fields: vec![
            Field { value: email, ..Field::new(m.email, "email", "email", "username") },
            Field::new(m.new_password, "password", "password", "new-password"),
            Field::new(m.repeat_password, "repeat", "password", "new-password"),
        ],
        error,
        notice: None,
        submit: m.accept_invite,
        footer: None,
    };
    public_page(m.invitation, &card.render())
}

/// `GET /invite/{token}`.
pub(super) async fn accept_form(State(state): State<AppState>, Path(secret): Path<String>) -> Response {
    let checked = check_link(&*lock(&state.ledger), &Token(secret.clone()), TokenKind::Invite);
    match checked {
        Ok(Ok(_)) => accept_page(&state, &secret, "", None).into_response(),
        Ok(Err(e)) => link_error(e, ("/login", msgs(Lang::Fr).back_to_login)),
        Err(e) => internal(&e),
    }
}

/// The accept / reset form fields.
#[derive(Deserialize)]
pub(super) struct PasswordForm {
    #[serde(default)]
    email: String,
    password: String,
    repeat: String,
}

/// Check the two password fields and hash them (off the async workers).
async fn new_password(f: &PasswordForm) -> Result<auth::PasswordHash, &'static str> {
    let m = msgs(Lang::Fr);
    if f.password != f.repeat {
        return Err(m.err_mismatch);
    }
    let plain = f.password.clone();
    match tokio::task::spawn_blocking(move || auth::hash_password(&plain)).await {
        Ok(Ok(h)) => Ok(h),
        Ok(Err(AuthError::PasswordTooShort)) => Err(m.err_too_short),
        _ => Err(m.err_server),
    }
}

/// `POST /invite/{token}`: the witness sets their own password; the
/// account is created, the link consumed, the witness signed in (APP-27).
pub(super) async fn accept(
    State(state): State<AppState>,
    Path(secret): Path<String>,
    Form(f): Form<PasswordForm>,
) -> Response {
    let token = Token(secret.clone());
    let back = ("/login", msgs(Lang::Fr).back_to_login);
    match check_link(&*lock(&state.ledger), &token, TokenKind::Invite) {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return link_error(e, back),
        Err(e) => return internal(&e),
    }
    let m = msgs(Lang::Fr);
    let email = f.email.trim().to_owned();
    let bad = |error: &str| {
        (StatusCode::BAD_REQUEST, accept_page(&state, &secret, &email, Some(error))).into_response()
    };
    if email.is_empty() {
        return bad(m.err_email_empty);
    }
    let hash = match new_password(&f).await {
        Ok(h) => h,
        Err(msg) => return bad(msg),
    };
    // One lock for check, consume and create: two accepts of one link
    // cannot both win, and a taken email does not burn the link.
    let created = {
        let mut store = lock(&state.ledger);
        match store.account_by_email(&email) {
            Ok(Some(_)) => Err(Some(m.err_email_taken)),
            Ok(None) => match store.use_token(&token) {
                Ok(true) => store
                    .insert_account(&email, &hash, Role::Witness)
                    .and_then(|a| session::open_session(&mut store, &a))
                    .map_err(|e| {
                        eprintln!("web: invite accept: {e}");
                        None
                    }),
                Ok(false) => return link_error(LinkError::Used, back),
                Err(e) => return internal(&e),
            },
            Err(e) => return internal(&e),
        }
    };
    match created {
        Ok(session) => {
            let mut r = see_other("/");
            r.headers_mut()
                .insert(header::SET_COOKIE, session::set_cookie(&state, Some(&session)));
            r
        }
        Err(Some(msg)) => bad(msg),
        Err(None) => internal(&m.err_server),
    }
}

/// The reset request form field.
#[derive(Deserialize)]
pub(super) struct ResetRequest {
    #[serde(default)]
    email: String,
}

/// The reset request form, with an optional error.
fn reset_request_page(error: Option<&str>) -> Markup {
    let m = msgs(Lang::Fr);
    let card = AuthCard {
        action: "/reset",
        label: m.reset_label,
        title: m.reset_title,
        big_title: false,
        intro: Some(m.reset_intro),
        fields: vec![Field::new(m.email, "email", "email", "username")],
        error,
        notice: None,
        submit: m.send_link,
        footer: Some(("/login", m.back_to_login)),
    };
    public_page(m.reset_label, &card.render())
}

/// `GET /reset`.
pub(super) async fn reset_request_form() -> Markup {
    reset_request_page(None)
}

/// A reset token for the account with `email` in `store`, if any.
/// `admin_only` is set for the dev file (DEV-3).
fn issue_reset<K: Kind>(
    store: &mut Store<K>,
    email: &str,
    admin_only: bool,
) -> Result<Option<(Account, Token)>, StoreError> {
    let Some(account) = store.account_by_email(email)? else {
        return Ok(None);
    };
    if admin_only && account.role != Role::Admin {
        return Ok(None);
    }
    let token = Token::generate();
    store.insert_token(&IssuedToken {
        token: token.clone(),
        kind: TokenKind::Reset,
        account: Some(account.id),
        issued_at: now(),
        used: false,
    })?;
    Ok(Some((account, token)))
}

/// `POST /reset`: email a 1-hour single-use link to the account, if the
/// address has one (APP-28). The answer is the same either way.
pub(super) async fn reset_request(
    State(state): State<AppState>,
    Form(f): Form<ResetRequest>,
) -> Response {
    let m = msgs(Lang::Fr);
    let email = f.email.trim().to_owned();
    if email.is_empty() {
        return (StatusCode::BAD_REQUEST, reset_request_page(Some(m.err_email_empty))).into_response();
    }
    let issued = match issue_reset(&mut *lock(&state.ledger), &email, false) {
        Ok(None) => issue_reset(&mut *lock(&state.dev), &email, true),
        other => other,
    };
    match issued {
        Ok(Some((account, token))) => {
            let url = link(&state, "reset", &token);
            let expires = now() + TokenKind::Reset.lifetime();
            let mail = mail::reset(&account.email, &url, expires, account.lang);
            // DECISION: the mail leaves in the background and the answer
            // never says whether it did, so the page and its timing are the
            // same for known and unknown addresses; a failure is logged by
            // the outbox (SRS 5.1).
            outbox::deliver_later(&state, mail);
        }
        Ok(None) => {}
        Err(e) => return internal(&e),
    }
    let card = pages::notice_card(m.reset_label, m.reset_title, m.reset_sent, ("/login", m.back_to_login));
    public_page(m.reset_label, &card).into_response()
}

/// Where a reset link lives: ledger first, then the dev file for the
/// admin only (DEV-3).
fn find_reset(
    state: &AppState,
    token: &Token,
) -> Result<Result<(IssuedToken, Database), LinkError>, StoreError> {
    match check_link(&*lock(&state.ledger), token, TokenKind::Reset)? {
        Err(LinkError::Unknown) => {}
        found => return Ok(found.map(|t| (t, Database::Ledger))),
    }
    let dev = lock(&state.dev);
    match check_link(&*dev, token, TokenKind::Reset)? {
        Ok(row) => {
            let admin = match row.account {
                Some(id) => dev.account(id)?.is_some_and(|a| a.role == Role::Admin),
                None => false,
            };
            Ok(if admin { Ok((row, Database::Dev)) } else { Err(LinkError::Unknown) })
        }
        Err(e) => Ok(Err(e)),
    }
}

/// The new-password form, with an optional error.
fn reset_page(secret: &str, error: Option<&str>) -> Markup {
    let m = msgs(Lang::Fr);
    let action = format!("/reset/{secret}");
    let card = AuthCard {
        action: &action,
        label: m.reset_label,
        title: m.set_password,
        big_title: false,
        intro: None,
        fields: vec![
            Field::new(m.new_password, "password", "password", "new-password"),
            Field::new(m.repeat_password, "repeat", "password", "new-password"),
        ],
        error,
        notice: None,
        submit: m.set_password,
        footer: None,
    };
    public_page(m.reset_label, &card.render())
}

/// `GET /reset/{token}`.
pub(super) async fn reset_form(State(state): State<AppState>, Path(secret): Path<String>) -> Response {
    match find_reset(&state, &Token(secret.clone())) {
        Ok(Ok(_)) => reset_page(&secret, None).into_response(),
        Ok(Err(e)) => link_error(e, ("/reset", msgs(Lang::Fr).reset_title)),
        Err(e) => internal(&e),
    }
}

/// Consume `token` and set the password of its account in `store`.
fn apply_reset<K: Kind>(
    store: &mut Store<K>,
    token: &Token,
    row: &IssuedToken,
    hash: &auth::PasswordHash,
) -> Result<bool, StoreError> {
    let Some(account) = row.account else {
        return Ok(false);
    };
    if !store.use_token(token)? {
        return Ok(false);
    }
    store.set_password(account, hash)?;
    Ok(true)
}

/// `POST /reset/{token}`: set the new password, consume the link, send
/// the user to the login page (no session is opened).
pub(super) async fn reset(
    State(state): State<AppState>,
    Path(secret): Path<String>,
    Form(f): Form<PasswordForm>,
) -> Response {
    let token = Token(secret.clone());
    let back = ("/reset", msgs(Lang::Fr).reset_title);
    let (row, home) = match find_reset(&state, &token) {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => return link_error(e, back),
        Err(e) => return internal(&e),
    };
    let hash = match new_password(&f).await {
        Ok(h) => h,
        Err(msg) => return (StatusCode::BAD_REQUEST, reset_page(&secret, Some(msg))).into_response(),
    };
    let done = match home {
        Database::Ledger => apply_reset(&mut *lock(&state.ledger), &token, &row, &hash),
        Database::Dev => apply_reset(&mut *lock(&state.dev), &token, &row, &hash),
    };
    match done {
        Ok(true) => see_other("/login?done=reset"),
        Ok(false) => link_error(LinkError::Used, back),
        Err(e) => internal(&e),
    }
}
