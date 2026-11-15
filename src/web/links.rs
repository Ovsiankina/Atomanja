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
use crate::auth::{self, AuthError, IssuedToken, Role, Token, TokenKind};
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
        Err(r) => return r,
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
        settings::account_panel(m, &accounts, &actor.account, Some(&outcome)).into_response()
    } else {
        settings::render(&state, &actor.account, Some(&outcome))
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
