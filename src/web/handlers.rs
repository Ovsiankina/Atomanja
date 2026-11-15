//! Sign in, sign out, and the dashboard each role lands on.

use std::collections::BTreeMap;

use axum::extract::{Form, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use maud::Markup;
use serde::Deserialize;

use super::i18n::{msgs, Msgs};
use super::pages::{self, AuthCard, Field, Nav};
use super::session::{self, internal, lock, see_other, Actor};
use super::views::{self, Ctx, Viewer};
use super::AppState;
use crate::auth::{self, Account, Database, Role};
use crate::config::Lang;
use crate::ledger::TaskId;
use crate::store::{Kind, Store, StoreError};

/// The signed-in actor, or the response to send instead (redirect to
/// the login page, or 500). Boxed: a `Response` is large.
pub(super) fn require(state: &AppState, headers: &HeaderMap) -> Result<Actor, Box<Response>> {
    match session::actor(state, headers) {
        Ok(Some(a)) => Ok(a),
        Ok(None) => Err(Box::new(see_other("/login"))),
        Err(e) => Err(Box::new(internal(&e))),
    }
}

/// A full page for a signed-in account.
pub(super) fn page(m: &Msgs, title: &str, current: &'static str, content: &Markup) -> Response {
    pages::layout(m, title, Some(&Nav { current }), content).into_response()
}

/// A full pre-login page: always French (APP-40).
pub(super) fn public_page(title: &str, content: &Markup) -> Markup {
    pages::layout(msgs(Lang::Fr), title, None, content)
}

/// Any other path: 404 with a page that leads back.
pub(super) async fn not_found() -> Response {
    let m = msgs(Lang::Fr);
    let card = pages::notice_card("404", m.err_not_found, m.err_not_found, ("/", m.nav_tasks));
    (StatusCode::NOT_FOUND, public_page(m.err_not_found, &card)).into_response()
}

/// `GET /`: the worker view or the witness view, from the one file the
/// account's role reads (DEV-3).
pub(super) async fn index(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let actor = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return *r,
    };
    let m = msgs(actor.account.lang);
    // Titles are copied before any store lock (never two locks at once),
    // and only for the worker: the witness view never reads them (APP-32).
    let titles = (actor.account.role == Role::Worker).then(|| lock(&state.titles).clone());
    let body = match actor.account.role.database() {
        Database::Ledger => dashboard_from(&*lock(&state.ledger), &actor.account, titles.as_ref()),
        Database::Dev => dashboard_from(&*lock(&state.dev), &actor.account, None),
    };
    match body {
        Ok(b) => page(m, m.nav_tasks, "tasks", &b),
        Err(e) => internal(&e),
    }
}

/// The dashboard of `account` over `store`.
fn dashboard_from<K: Kind>(
    store: &Store<K>,
    account: &Account,
    titles: Option<&BTreeMap<TaskId, String>>,
) -> Result<Markup, StoreError> {
    let ledger = store.load_ledger()?;
    let ctx = Ctx {
        m: msgs(account.lang),
        viewer: Viewer::of(account),
        titles,
        names: Ctx::names_of(&store.accounts()?),
    };
    Ok(views::dashboard(&ctx, &ledger, None))
}

/// The login form, with an optional error or notice.
fn login_page(error: Option<&str>, notice: Option<&str>, email: &str) -> Markup {
    let m = msgs(Lang::Fr);
    let mut email_field = Field::new(m.email, "email", "email", "username");
    email_field.value = email;
    let card = AuthCard {
        action: "/login",
        label: m.sign_in_label,
        title: "Atomanja",
        big_title: true,
        intro: None,
        fields: vec![
            email_field,
            Field::new(m.password, "password", "password", "current-password"),
        ],
        error,
        notice,
        submit: m.sign_in,
        footer: Some(("/reset", m.forgot)),
    };
    public_page(m.sign_in_label, &card.render())
}

/// `?done=reset` after a password reset.
#[derive(Deserialize)]
pub(super) struct LoginQuery {
    done: Option<String>,
}

/// `GET /login`.
pub(super) async fn login_form(
    axum::extract::Query(q): axum::extract::Query<LoginQuery>,
) -> Markup {
    let notice = (q.done.as_deref() == Some("reset")).then_some(msgs(Lang::Fr).reset_done);
    login_page(None, notice, "")
}

/// The login form fields.
#[derive(Deserialize)]
pub(super) struct LoginForm {
    email: String,
    password: String,
}

/// `POST /login`: on success a session in the account's own file and a
/// cookie; otherwise 401 and the form again.
pub(super) async fn login(State(state): State<AppState>, Form(f): Form<LoginForm>) -> Response {
    let email = f.email.trim().to_owned();
    // Ledger first, then the dev file for the admin only (DEV-3).
    let found = lock(&state.ledger)
        .account_by_email(&email)
        .map(|a| a.map(|a| (a, Database::Ledger)));
    let found = match found {
        Ok(None) => lock(&state.dev)
            .account_by_email(&email)
            .map(|a| a.filter(|a| a.role == Role::Admin).map(|a| (a, Database::Dev))),
        other => other,
    };
    let (account, home) = match found {
        Ok(Some(found)) => found,
        Ok(None) => return rejected(&email),
        Err(e) => return internal(&e),
    };
    let hash = account.password_hash.clone();
    let password = f.password;
    // argon2 is slow on purpose: keep it off the async workers.
    let ok = tokio::task::spawn_blocking(move || auth::verify_password(&hash, &password))
        .await
        .unwrap_or(false);
    if !ok {
        return rejected(&email);
    }
    let token = match home {
        Database::Ledger => session::open_session(&mut *lock(&state.ledger), &account),
        Database::Dev => session::open_session(&mut *lock(&state.dev), &account),
    };
    match token {
        Ok(t) => {
            let mut r = see_other("/");
            r.headers_mut()
                .insert(header::SET_COOKIE, session::set_cookie(&state, Some(&t)));
            r
        }
        Err(e) => internal(&e),
    }
}

/// 401 with the login form and one error for both unknown email and
/// wrong password (no account probing).
fn rejected(email: &str) -> Response {
    let m = msgs(Lang::Fr);
    (StatusCode::UNAUTHORIZED, login_page(Some(m.login_error), None, email)).into_response()
}

/// `POST /logout`: the session is marked used, the cookie cleared.
pub(super) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Ok(Some(actor)) = session::actor(&state, &headers) {
        let done = match actor.home {
            Database::Ledger => lock(&state.ledger).use_token(&actor.token),
            Database::Dev => lock(&state.dev).use_token(&actor.token),
        };
        if let Err(e) = done {
            return internal(&e);
        }
    }
    let mut r = see_other("/login");
    r.headers_mut()
        .insert(header::SET_COOKIE, session::set_cookie(&state, None));
    r
}
