//! HTTP layer: pages for both views, auth flows, claim/mark/amount
//! actions, push subscription, PWA manifest and service worker.
//!
//! Uses: `config`, `ledger`, `auth`, `store`, `notify`, `mail`.
//! SRS: APP-21..APP-33, DEV-3.
//!
//! # Contract shape: state in, router out
//!
//! The pub surface is three items: [`AppState`] (everything a handler
//! needs, built by the CLI), [`router`] (the axum `Router` over that
//! state; tests bind it to `127.0.0.1:0` and drive it with `reqwest`)
//! and [`serve`] (bind and run, called by `serve` in the CLI next to
//! `sync::run`). Handlers, pages (maud), the message catalogue per
//! language (APP-38..APP-40), session cookies and static assets are all
//! private to this module and its children in `src/web/`.
//!
//! Both store handles sit behind a `std::sync::Mutex`, shared with
//! `sync`. A handler takes the lock for its synchronous part only
//! (session lookup, rule check, append) and never across an `.await`;
//! mail and push are sent after the lock is released, and their
//! failures never undo the action (SRS 5.1). The ledger is rebuilt from
//! the chosen store under its lock, as `sync` does, so web and sync
//! never hold diverging in-memory copies.
//!
//! DEV-3: a request is served from exactly one file, picked from the
//! session account's role (`auth::Role::database`). An admin request
//! reads only [`AppState::dev`]; every other request touches only
//! [`AppState::ledger`].
//!
//! TODO(srs) DEV-3: where the admin account and its sessions live is
//! open (see the store row in `docs/deferred.md`). This contract takes
//! the documented proposal: the admin account and its sessions live in
//! the dev file; login and session lookup check the ledger file first,
//! then the dev file. An anonymous login attempt therefore may read the
//! dev file's account table, never its ledger data.
//!
//! TODO(srs) APP-31, APP-32: page layout waits on the Claude Design
//! handoff (`design-system/`); the first build ships plain pages with
//! the required content.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::ledger::TaskId;
use crate::mail::Mailer;
use crate::notify::Notifier;
use crate::store::{DevDb, LedgerDb, Store};

mod actions;
mod assets;
mod handlers;
mod i18n;
mod links;
mod outbox;
mod pages;
mod session;
mod settings;
mod views;

/// Everything the handlers share. Cheap to clone (all `Arc` or
/// already-shared handles). The CLI builds it once and hands the same
/// `ledger` and `titles` to `sync::run`.
#[derive(Clone)]
pub struct AppState {
    /// Settings; `base_url` builds invite and reset links (APP-26, APP-28).
    pub config: Arc<Config>,
    /// The ledger database (DB-1). Never touched by an admin request (DEV-3).
    pub ledger: Arc<Mutex<Store<LedgerDb>>>,
    /// The dev database (DEV-1). Touched only by admin requests (DEV-3),
    /// and by login / session lookup per the DEV-3 `TODO(srs)` above.
    pub dev: Arc<Mutex<Store<DevDb>>>,
    /// Task titles from the last successful poll, filled by `sync`. Read
    /// only by the worker view (APP-31); never by the witness view (APP-32).
    pub titles: Arc<Mutex<BTreeMap<TaskId, String>>>,
    /// Push sender for notifications caused by web actions (APP-35, APP-36).
    pub notifier: Arc<Notifier>,
    /// SMTP sender for invite and reset emails (APP-26, APP-28).
    pub mailer: Mailer,
}

/// The full axum router over `state`: login / logout, invite create and
/// accept, reset request and confirm, worker view, witness view, claim /
/// mark sent / mark received / propose / approve actions (htmx
/// fragments), language switch, push subscribe, manifest, service worker
/// and static assets. SRS: APP-16, APP-18, APP-21, APP-22, APP-26..APP-33,
/// APP-38..APP-40, DEV-3.
pub fn router(state: AppState) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/", get(handlers::index))
        .route("/login", get(handlers::login_form).post(handlers::login))
        .route("/logout", post(handlers::logout))
        .route("/settings", get(settings::settings))
        .route("/invite", post(links::create_invite))
        .route("/invite/{token}", get(links::accept_form).post(links::accept))
        .route("/reset", get(links::reset_request_form).post(links::reset_request))
        .route("/reset/{token}", get(links::reset_form).post(links::reset))
        .route("/debts/{id}/claim", post(actions::claim))
        .route("/debts/{id}/sent", post(actions::sent))
        .route("/debts/{id}/received", post(actions::received))
        .route("/amount/propose", post(actions::propose))
        .route("/amount/approve", post(actions::approve))
        .route("/assets/{name}", get(assets::asset))
        .with_state(state)
}

// DECISION: binds with tokio::net::TcpListener + axum::serve; tokio's `net`
// feature is already on through axum's default `tokio` feature, so no
// Cargo change. No graceful shutdown (would need tokio `signal`).
/// Bind `addr` and serve [`router`] until the process is stopped.
///
/// # Errors
///
/// [`WebError::Bind`] when `addr` cannot be bound, [`WebError::Serve`]
/// when the server stops with an I/O error.
pub async fn serve(state: AppState, addr: SocketAddr) -> Result<(), WebError> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(WebError::Bind)?;
    axum::serve(listener, router(state))
        .await
        .map_err(WebError::Serve)
}

/// Why the server could not start or stopped.
#[derive(Debug)]
pub enum WebError {
    /// The listen address could not be bound.
    Bind(std::io::Error),
    /// The server loop failed.
    Serve(std::io::Error),
}

impl std::fmt::Display for WebError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebError::Bind(e) => write!(f, "cannot bind the listen address: {e}"),
            WebError::Serve(e) => write!(f, "web server stopped: {e}"),
        }
    }
}

impl std::error::Error for WebError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WebError::Bind(e) | WebError::Serve(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod testkit;

#[cfg(test)]
mod tests {
    use super::testkit::{self, Harness};
    use crate::auth::{Role, Token, TokenKind};

    // web-01
    #[tokio::test]
    async fn web_login_sets_session() {
        let h = Harness::start().await;
        let worker = h.ledger(|s| {
            testkit::account_with_password(s, "worker@atomanja.test", "correct horse", Role::Worker)
        });
        let r = h
            .post(
                "/login",
                None,
                &[("email", "worker@atomanja.test"), ("password", "correct horse")],
            )
            .await;
        assert_eq!(r.status, 303, "{}", r.body);
        assert_eq!(r.location(), Some("/"));
        let cookie = r.session_cookie().expect("session cookie set");
        let set = r.headers.get("set-cookie").unwrap().to_str().unwrap().to_owned();
        assert!(set.contains("HttpOnly"), "{set}");
        assert!(set.contains("SameSite=Lax"), "{set}");
        let stored = h.ledger(|s| s.token(&Token(cookie.clone())).unwrap()).expect("stored");
        assert_eq!(stored.kind, TokenKind::Session);
        assert_eq!(stored.account, Some(worker.id));

        let page = h.get("/", Some(&cookie)).await;
        assert_eq!(page.status, 200);
        assert!(page.body.contains(r#"data-view="worker""#), "{}", page.body);

        let out = h.post("/logout", Some(&cookie), &[]).await;
        assert_eq!(out.status, 303);
        let after = h.get("/", Some(&cookie)).await;
        assert_eq!(after.status, 303, "logged-out session no longer works");
        assert_eq!(after.location(), Some("/login"));
    }

    #[tokio::test]
    async fn web_wrong_password_rejected() {
        let h = Harness::start().await;
        h.ledger(|s| {
            testkit::account_with_password(s, "worker@atomanja.test", "correct horse", Role::Worker)
        });
        for (email, password) in [
            ("worker@atomanja.test", "wrong horse"),
            ("nobody@atomanja.test", "correct horse"),
        ] {
            let r = h.post("/login", None, &[("email", email), ("password", password)]).await;
            assert_eq!(r.status, 401, "{}", r.body);
            assert!(r.session_cookie().is_none());
            assert!(r.body.contains("Adresse ou mot de passe incorrect"), "{}", r.body);
        }
        let anon = h.get("/", None).await;
        assert_eq!(anon.status, 303);
        assert_eq!(anon.location(), Some("/login"));
        let forged = h.get("/", Some("forged-token")).await;
        assert_eq!(forged.status, 303);
    }

    #[tokio::test]
    async fn dev_3_admin_requests_use_dev_store() {
        let h = Harness::start().await;
        let worker = h.ledger(|s| testkit::account(s, "worker@atomanja.test", Role::Worker));
        h.ledger(|s| testkit::late_debt(s, 47, None));
        h.title(47, "Real secret title");
        h.dev(|s| {
            testkit::account_with_password(s, "admin@atomanja.test", "admin password", Role::Admin)
        });
        h.dev(|s| testkit::late_debt(s, 901, None));

        let r = h
            .post(
                "/login",
                None,
                &[("email", "admin@atomanja.test"), ("password", "admin password")],
            )
            .await;
        assert_eq!(r.status, 303, "{}", r.body);
        let cookie = r.session_cookie().expect("admin session");
        assert!(h.dev(|s| s.token(&Token(cookie.clone())).unwrap()).is_some());
        assert!(h.ledger(|s| s.token(&Token(cookie.clone())).unwrap()).is_none());

        let page = h.get("/", Some(&cookie)).await;
        assert_eq!(page.status, 200);
        assert!(page.body.contains(r#"data-view="witness""#), "{}", page.body);
        assert!(page.body.contains("Tâche 901"), "{}", page.body);
        assert!(!page.body.contains("Tâche 47"), "{}", page.body);
        assert!(!page.body.contains("Real secret title"));
        assert!(page.body.contains("at-banner--fake"), "admin view carries the fake banner");

        // The admin cannot act, and nothing reaches the ledger file.
        let before = h.ledger(|s| s.load_changes().unwrap().len());
        let claim = h.post("/debts/0/claim", Some(&cookie), &[]).await;
        assert_eq!(claim.status, 403);
        assert_eq!(h.ledger(|s| s.load_changes().unwrap().len()), before);

        // A dev-file account that is not the admin never logs in (DEV-3).
        h.dev(|s| testkit::account_with_password(s, "fake@atomanja.test", "fake password", Role::Witness));
        let fake = h
            .post("/login", None, &[("email", "fake@atomanja.test"), ("password", "fake password")])
            .await;
        assert_eq!(fake.status, 401);

        // The worker sees the ledger, never the dev data.
        let wc = h.ledger(|s| testkit::session(s, &worker));
        let wp = h.get("/", Some(&wc)).await;
        assert!(wp.body.contains("Real secret title"), "{}", wp.body);
        assert!(!wp.body.contains("901"));
    }

    // web-02
    /// The `/invite/<token>` link in `text`, if any.
    fn invite_link(base: &str, text: &str) -> Option<String> {
        let prefix = format!("{base}/invite/");
        let start = text.find(&prefix)?;
        let rest = &text[start + prefix.len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(rest.len());
        Some(format!("{prefix}{}", &rest[..end]))
    }

    #[tokio::test]
    async fn app_26_worker_creates_invite_link_shown_and_mailed() {
        let h = Harness::start().await;
        let worker = h.ledger(|s| testkit::account(s, "worker@atomanja.test", Role::Worker));
        let cookie = h.ledger(|s| testkit::session(s, &worker));

        let r = h.post("/invite", Some(&cookie), &[("email", "app26-new@witness.test")]).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let link = invite_link(&h.base, &r.body).expect("link shown to the worker");
        let secret = link.rsplit('/').next().unwrap().to_owned();
        let stored = h.ledger(|s| s.token(&Token(secret)).unwrap()).expect("token stored");
        assert_eq!(stored.kind, TokenKind::Invite);
        assert_eq!(stored.account, None);
        assert!(!stored.used);

        let mails = super::outbox::sent_to("app26-new@witness.test");
        assert_eq!(mails.len(), 1, "mailed once to the address the worker entered");
        assert!(mails[0].body.contains(&link), "{}", mails[0].body);
        // The test SMTP host refuses connections: the page says so and
        // still shows the link (SRS 5.1).
        assert!(r.body.contains("E-mail non envoyé. Copiez le lien."), "{}", r.body);

        // Without an address: a fresh link, shown only.
        let hx = h.hx_post("/invite", Some(&cookie), &[("email", "")]).await;
        assert_eq!(hx.status, 200);
        assert!(!hx.body.contains("<html"), "htmx gets a fragment");
        let second = invite_link(&h.base, &hx.body).expect("second link");
        assert_ne!(second, link);

        // A witness cannot invite (APP-26: the worker generates links).
        let witness = h.ledger(|s| testkit::account(s, "w@witness.test", Role::Witness));
        let wc = h.ledger(|s| testkit::session(s, &witness));
        let denied = h.post("/invite", Some(&wc), &[("email", "x@witness.test")]).await;
        assert_eq!(denied.status, 403);
        assert!(super::outbox::sent_to("x@witness.test").is_empty());
    }

    #[tokio::test]
    async fn app_27_invite_accept_sets_witness_password() {
        let h = Harness::start().await;
        h.ledger(|s| testkit::account(s, "worker@atomanja.test", Role::Worker));
        let secret = h.ledger(|s| {
            testkit::token(s, TokenKind::Invite, None, time::Duration::hours(1), false)
        });
        let path = format!("/invite/{secret}");

        let form = h.get(&path, None).await;
        assert_eq!(form.status, 200);
        assert!(form.body.contains("worker@atomanja.test"), "names who asks: {}", form.body);
        assert!(form.body.contains(r#"name="repeat""#));

        let bad = h
            .post(&path, None, &[("email", "new@witness.test"), ("password", "witness pass"), ("repeat", "other pass")])
            .await;
        assert_eq!(bad.status, 400);
        assert!(bad.body.contains("Les deux mots de passe diffèrent."), "{}", bad.body);
        assert!(h.ledger(|s| s.account_by_email("new@witness.test").unwrap()).is_none());

        let ok = h
            .post(&path, None, &[("email", "new@witness.test"), ("password", "witness pass"), ("repeat", "witness pass")])
            .await;
        assert_eq!(ok.status, 303, "{}", ok.body);
        assert_eq!(ok.location(), Some("/"));
        let cookie = ok.session_cookie().expect("signed in after accepting");
        let account = h
            .ledger(|s| s.account_by_email("new@witness.test").unwrap())
            .expect("witness account");
        assert_eq!(account.role, Role::Witness);
        assert!(crate::auth::verify_password(&account.password_hash, "witness pass"));
        assert!(h.ledger(|s| s.token(&Token(secret.clone())).unwrap()).unwrap().used);
        let view = h.get("/", Some(&cookie)).await;
        assert!(view.body.contains(r#"data-view="witness""#));

        // Single use: the same link never creates a second account.
        let again = h
            .post(&path, None, &[("email", "other@witness.test"), ("password", "witness pass"), ("repeat", "witness pass")])
            .await;
        assert_eq!(again.status, 410);
        assert!(again.session_cookie().is_none());
        assert!(h.ledger(|s| s.account_by_email("other@witness.test").unwrap()).is_none());
    }

    // web-03
    #[tokio::test]
    async fn app_28_reset_request_sends_mail() {
        let h = Harness::start().await;
        let email = "app28-worker@atomanja.test";
        let worker = h.ledger(|s| testkit::account_with_password(s, email, "old password", Role::Worker));
        h.ledger(|s| s.set_lang(worker.id, crate::config::Lang::En).unwrap());

        let form = h.get("/reset", None).await;
        assert_eq!(form.status, 200);
        assert!(form.body.contains(r#"name="email""#));

        let r = h.post("/reset", None, &[("email", email)]).await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.body.contains("un lien est en route"), "{}", r.body);
        let mails = super::outbox::sent_to(email);
        assert_eq!(mails.len(), 1);
        assert!(mails[0].body.contains("password"), "account language (APP-39): {}", mails[0].body);
        let prefix = format!("{}/reset/", h.base);
        let start = mails[0].body.find(&prefix).expect("link in mail") + prefix.len();
        let secret: String = mails[0].body[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let stored = h.ledger(|s| s.token(&Token(secret.clone())).unwrap()).expect("stored");
        assert_eq!(stored.kind, TokenKind::Reset);
        assert_eq!(stored.account, Some(worker.id));

        // Unknown address: the same answer, no mail (no account probing).
        let unknown = h.post("/reset", None, &[("email", "app28-nobody@atomanja.test")]).await;
        assert_eq!(unknown.status, 200);
        assert!(unknown.body.contains("un lien est en route"));
        assert!(super::outbox::sent_to("app28-nobody@atomanja.test").is_empty());

        // Following the link sets the new password; it signs nobody in.
        let path = format!("/reset/{secret}");
        assert_eq!(h.get(&path, None).await.status, 200);
        let done = h
            .post(&path, None, &[("password", "new password"), ("repeat", "new password")])
            .await;
        assert_eq!(done.status, 303, "{}", done.body);
        assert_eq!(done.location(), Some("/login?done=reset"));
        assert!(done.session_cookie().is_none());
        let account = h.ledger(|s| s.account(worker.id).unwrap()).unwrap();
        assert!(crate::auth::verify_password(&account.password_hash, "new password"));
        let login = h.post("/login", None, &[("email", email), ("password", "new password")]).await;
        assert_eq!(login.status, 303);
    }

    #[tokio::test]
    async fn web_expired_or_used_link_shows_error_page() {
        use time::Duration;
        let h = Harness::start().await;
        let worker = h.ledger(|s| testkit::account_with_password(s, "w@atomanja.test", "old password", Role::Worker));
        let mut cases = h.ledger(|s| {
            vec![
                ("reset", testkit::token(s, TokenKind::Reset, Some(&worker), Duration::hours(2), false), 410, "Ce lien a expiré."),
                ("reset", testkit::token(s, TokenKind::Reset, Some(&worker), Duration::minutes(5), true), 410, "Ce lien a déjà servi."),
                ("invite", testkit::token(s, TokenKind::Invite, None, Duration::days(8), false), 410, "Ce lien a expiré."),
                ("invite", testkit::token(s, TokenKind::Invite, None, Duration::days(1), true), 410, "Ce lien a déjà servi."),
                // A session secret is not a reset link.
                ("reset", testkit::session(s, &worker), 404, "Ce lien n'est pas valide."),
            ]
        });
        cases.push(("reset", "no-such-token".to_owned(), 404, "Ce lien n'est pas valide."));
        cases.push(("invite", "no-such-token".to_owned(), 404, "Ce lien n'est pas valide."));

        for (kind, secret, status, text) in cases {
            let path = format!("/{kind}/{secret}");
            let page = h.get(&path, None).await;
            assert_eq!(page.status, status, "GET {path}");
            assert!(page.body.contains(text), "GET {path}: {}", page.body);
            assert!(page.session_cookie().is_none());

            let post = h
                .post(&path, None, &[("email", "late@witness.test"), ("password", "hacked password"), ("repeat", "hacked password")])
                .await;
            assert_eq!(post.status, status, "POST {path}");
            assert!(post.body.contains(text), "POST {path}: {}", post.body);
            assert!(post.session_cookie().is_none(), "never signs anyone in");
        }
        let account = h.ledger(|s| s.account(worker.id).unwrap()).unwrap();
        assert!(crate::auth::verify_password(&account.password_hash, "old password"));
        assert!(h.ledger(|s| s.account_by_email("late@witness.test").unwrap()).is_none());
    }

    // web-04
    /// Accounts of [`rich_ledger`].
    struct People {
        worker: crate::auth::Account,
        a: crate::auth::Account,
        b: crate::auth::Account,
    }

    /// Worker, witnesses a and b, and one row of every kind, with titles:
    /// #47 received, #48 sent, #49 unclaimed, #50 met, #51 upcoming with
    /// a moved deadline, #52 claimed by a and not sent.
    fn rich_ledger(h: &Harness) -> People {
        use crate::ledger::{Change, EventKind, OccurrenceId, Outcome, TaskId, Timestamp};
        let t = testkit::DEADLINE;
        h.ledger(|s| {
            let worker = testkit::account(s, "worker@atomanja.test", Role::Worker);
            let a = testkit::account(s, "a@witness.test", Role::Witness);
            let b = testkit::account(s, "b@witness.test", Role::Witness);
            let received = testkit::late_debt(s, 47, Some(a.id.0));
            let sent = testkit::late_debt(s, 48, Some(a.id.0));
            testkit::late_debt(s, 49, None);
            s.append(&[
                Change::MarkedSent { debt: received, at: Timestamp(t + 100) },
                Change::MarkedReceived { debt: received, at: Timestamp(t + 200) },
                Change::MarkedSent { debt: sent, at: Timestamp(t + 100) },
                Change::Recorded { occurrence: OccurrenceId(3), task: TaskId(50), deadline: Timestamp(t), at: Timestamp(t - 9000) },
                Change::Decided { occurrence: OccurrenceId(3), outcome: Outcome::Met, at: Timestamp(t + 60) },
                Change::Recorded { occurrence: OccurrenceId(4), task: TaskId(51), deadline: Timestamp(t), at: Timestamp(t - 9000) },
                Change::Event { occurrence: OccurrenceId(4), kind: EventKind::MovedLater, new_due: Some(Timestamp(t + 6 * 86_400)), at: Timestamp(t - 3600) },
            ])
            .unwrap();
            testkit::late_debt(s, 52, Some(a.id.0));
            People { worker, a, b }
        })
        .tap(|_| {
            for (n, title) in [(47, "Pay rent"), (48, "Write report"), (49, "Call dentist"), (50, "Water plants"), (51, "File taxes"), (52, "Fix bike")] {
                h.title(n, title);
            }
        })
    }

    /// `tap` for the fixture above.
    trait Tap: Sized {
        fn tap(self, f: impl FnOnce(&Self)) -> Self {
            f(&self);
            self
        }
    }
    impl<T> Tap for T {}

    #[tokio::test]
    async fn app_31_worker_view_shows_titles_debts_totals() {
        let h = Harness::start().await;
        let p = rich_ledger(&h);
        let cookie = h.ledger(|s| testkit::session(s, &p.worker));
        let page = h.get("/", Some(&cookie)).await;
        assert_eq!(page.status, 200);
        let body = &page.body;
        assert!(body.contains(r#"data-view="worker""#));
        for title in ["Pay rent", "Write report", "Call dentist", "Water plants", "File taxes", "Fix bike"] {
            assert!(body.contains(title), "title {title} shown: {body}");
        }
        for n in 47..=52 {
            assert!(body.contains(&format!("#{n}")), "task number #{n}");
        }
        assert!(body.contains("14 nov."), "recorded deadline, day then month");
        for word in ["En retard", "Tenu", "À venir", "Envoyé", "Reçu"] {
            assert!(body.contains(word), "status {word}");
        }
        assert!(body.contains("Réclamée par a@witness.test"), "owner shown");
        assert!(body.contains("Non réclamée"));
        assert!(body.contains("10 CHF"));
        assert!(body.contains("at-check is-on"), "marks shown");
        assert!(
            body.contains("Échéance déplacée au 20 nov. dans Vikunja. Le 14 nov. compte toujours."),
            "deadline event under its task: {body}"
        );
        // Totals: owed every debt (4), sent 2, confirmed 1 (APP-19, APP-24).
        assert!(body.contains(r#"at-total__n">40<"#), "owed: {body}");
        assert!(body.contains(r#"at-total__n">20<"#), "sent");
        assert!(body.contains(r#"at-total__n">10<"#), "confirmed");
        // The worker's one action: mark sent on the claimed, unsent debt (#52 = debt 3).
        assert!(body.contains(r#"action="/debts/3/sent""#), "{body}");
        assert!(!body.contains("/claim\""), "the worker never claims");
        assert!(!body.contains("/received\""));
        // The page is styled by the vendored design system.
        assert!(body.contains("/assets/bundle.css"));
        let css = h.get("/assets/bundle.css", None).await;
        assert_eq!(css.status, 200);
        assert!(css.body.contains(".at-row"));
        let _ = (p.a, p.b);
    }

    // web-05
    const TITLES: [&str; 6] = ["Pay rent", "Write report", "Call dentist", "Water plants", "File taxes", "Fix bike"];

    #[tokio::test]
    async fn app_32_witness_view_never_contains_task_title() {
        let h = Harness::start().await;
        let p = rich_ledger(&h);
        let ca = h.ledger(|s| testkit::session(s, &p.a));
        let cb = h.ledger(|s| testkit::session(s, &p.b));
        for (cookie, path) in [(&ca, "/"), (&ca, "/settings"), (&cb, "/"), (&cb, "/settings")] {
            let page = h.get(path, Some(cookie)).await;
            assert_eq!(page.status, 200, "{path}");
            for title in TITLES {
                assert!(!page.body.contains(title), "{path} leaks {title:?}: {}", page.body);
            }
        }
        let page = h.get("/", Some(&ca)).await.body;
        assert!(page.contains(r#"data-view="witness""#));
        for n in 47..=52 {
            assert!(page.contains(&format!("Tâche {n}")), "task number only: Tâche {n}");
        }
        assert!(page.contains("14 nov."), "recorded deadline");
        for word in ["En retard", "Tenu", "À venir", "Envoyé", "Reçu"] {
            assert!(page.contains(word), "status {word}");
        }
        assert!(page.contains("Échéance déplacée au 20 nov."), "deadline events");
        assert!(page.contains("Réclamée par vous"));
        assert!(page.contains(r#"action="/debts/2/claim""#), "unclaimed #49 claimable");
        assert!(page.contains(r#"action="/debts/1/received""#), "owner marks #48 received");
        assert!(!page.contains("/sent\""), "witnesses never mark sent");
        let other = h.get("/", Some(&cb)).await.body;
        assert!(other.contains(r#"action="/debts/2/claim""#));
        assert!(!other.contains("/received\""), "only the owner marks received");
    }

    // web-06
    /// Every stored change matching `keep`.
    fn count(h: &Harness, keep: impl Fn(&crate::ledger::Change) -> bool) -> usize {
        h.ledger(|s| s.load_changes().unwrap().iter().filter(|c| keep(c)).count())
    }

    #[tokio::test]
    async fn app_18_claim_action_second_claim_rejected() {
        use crate::ledger::{Change, DebtId, WitnessId};
        let h = Harness::start().await;
        let p = rich_ledger(&h);
        let (ca, cb) = h.ledger(|s| (testkit::session(s, &p.a), testkit::session(s, &p.b)));
        let cw = h.ledger(|s| testkit::session(s, &p.worker));
        let claims_of_2 = |h: &Harness| count(h, |c| matches!(c, Change::Claimed { debt: DebtId(2), .. }));

        let first = h.hx_post("/debts/2/claim", Some(&ca), &[]).await;
        assert_eq!(first.status, 200, "{}", first.body);
        assert!(first.body.starts_with("<li"), "htmx gets the row: {}", first.body);
        assert!(first.body.contains("Réclamée par vous"));
        assert!(first.body.contains(r#"hx-swap-oob="true""#), "totals refreshed");
        for title in TITLES {
            assert!(!first.body.contains(title), "witness fragment leaks {title}");
        }
        let owner = h.ledger(|s| s.load_ledger().unwrap().debts().owner[2]);
        assert_eq!(owner, Some(WitnessId(p.a.id.0)));

        let second = h.hx_post("/debts/2/claim", Some(&cb), &[]).await;
        assert_eq!(second.status, 409);
        assert!(second.body.contains("Déjà réclamée par un autre témoin."), "{}", second.body);
        assert!(second.body.contains("Réclamée par a@witness.test"), "row shows the real owner");
        assert_eq!(claims_of_2(&h), 1, "no duplicate claim row");
        let again = h.post("/debts/2/claim", Some(&ca), &[]).await;
        assert_eq!(again.status, 409, "double click");
        assert_eq!(claims_of_2(&h), 1);

        assert_eq!(h.post("/debts/2/claim", Some(&cw), &[]).await.status, 403, "the worker never claims");
        assert_eq!(h.post("/debts/99/claim", Some(&cb), &[]).await.status, 404);

        // Plain form post: redirect back to the view.
        let debt = h.ledger(|s| testkit::late_debt(s, 53, None));
        let plain = h.post(&format!("/debts/{}/claim", debt.0), Some(&cb), &[]).await;
        assert_eq!(plain.status, 303);
        assert_eq!(plain.location(), Some("/"));
        let owner = h.ledger(|s| s.load_ledger().unwrap().debts().owner[debt.0 as usize]);
        assert_eq!(owner, Some(WitnessId(p.b.id.0)));
        assert_eq!(h.post("/debts/2/claim", None, &[]).await.status, 303, "anonymous goes to login");
        assert_eq!(claims_of_2(&h), 1);
    }

    #[tokio::test]
    async fn app_21_mark_sent_action_worker_only() {
        use crate::ledger::{Change, DebtId};
        let h = Harness::start().await;
        let p = rich_ledger(&h);
        let (cw, ca) = h.ledger(|s| (testkit::session(s, &p.worker), testkit::session(s, &p.a)));
        let sent_3 = |h: &Harness| count(h, |c| matches!(c, Change::MarkedSent { debt: DebtId(3), .. }));

        assert_eq!(h.post("/debts/3/sent", Some(&ca), &[]).await.status, 403, "witness cannot mark sent");
        assert_eq!(sent_3(&h), 0);

        let unclaimed = h.hx_post("/debts/2/sent", Some(&cw), &[]).await;
        assert_eq!(unclaimed.status, 409, "only on a claimed debt");
        assert!(unclaimed.body.contains("Personne n'a encore réclamé cette dette."), "{}", unclaimed.body);

        let ok = h.hx_post("/debts/3/sent", Some(&cw), &[]).await;
        assert_eq!(ok.status, 200, "{}", ok.body);
        assert!(ok.body.contains("Fix bike"), "worker fragment keeps the title");
        assert!(ok.body.contains("at-row--sent"));
        assert_eq!(sent_3(&h), 1);

        let twice = h.hx_post("/debts/3/sent", Some(&cw), &[]).await;
        assert_eq!(twice.status, 409, "APP-23");
        assert!(twice.body.contains("Déjà marquée."));
        assert_eq!(sent_3(&h), 1);

        let plain = h.post("/debts/2/sent", Some(&cw), &[]).await;
        assert_eq!(plain.status, 409, "plain post shows the page with the reason");
        assert!(plain.body.contains("<html"));
        assert!(plain.body.contains("Personne n'a encore réclamé cette dette."));
    }

    #[tokio::test]
    async fn app_22_mark_received_action_owner_only() {
        use crate::ledger::{Change, DebtId};
        let h = Harness::start().await;
        let p = rich_ledger(&h);
        let (cw, ca, cb) = h.ledger(|s| {
            (testkit::session(s, &p.worker), testkit::session(s, &p.a), testkit::session(s, &p.b))
        });
        let received_1 = |h: &Harness| count(h, |c| matches!(c, Change::MarkedReceived { debt: DebtId(1), .. }));

        assert_eq!(h.post("/debts/1/received", Some(&cw), &[]).await.status, 403);
        let other = h.hx_post("/debts/1/received", Some(&cb), &[]).await;
        assert_eq!(other.status, 409);
        assert!(other.body.contains("Seul le témoin qui l'a réclamée"), "{}", other.body);
        let early = h.hx_post("/debts/3/received", Some(&ca), &[]).await;
        assert_eq!(early.status, 409, "received needs sent first");
        assert!(early.body.contains("pas encore marquée envoyée"), "{}", early.body);
        assert_eq!(received_1(&h), 0);

        let plain = h.post("/debts/1/received", Some(&ca), &[]).await;
        assert_eq!(plain.status, 303);
        assert_eq!(received_1(&h), 1);
        let twice = h.hx_post("/debts/1/received", Some(&ca), &[]).await;
        assert_eq!(twice.status, 409);
        assert_eq!(received_1(&h), 1);

        // APP-24: both the worker and the owner witness see both marks.
        for cookie in [&cw, &ca] {
            let page = h.get("/", Some(cookie)).await.body;
            let row = &page[page.find(r#"id="occ-1""#).unwrap()..];
            let row = &row[..row.find("</li>").unwrap()];
            assert_eq!(row.matches("at-check is-on").count(), 2, "{row}");
        }
    }

    #[tokio::test]
    async fn app_16_amount_propose_and_approve_actions() {
        use crate::ledger::Centimes;
        let h = Harness::start().await;
        let p = rich_ledger(&h);
        let (cw, ca, cb) = h.ledger(|s| {
            (testkit::session(s, &p.worker), testkit::session(s, &p.a), testkit::session(s, &p.b))
        });
        let state = |h: &Harness| {
            h.ledger(|s| {
                let l = s.load_ledger().unwrap();
                (l.amount(), l.proposal().map(|p| (p.amount, p.approvals.len())))
            })
        };

        let settings = h.get("/settings", Some(&cw)).await.body;
        assert!(settings.contains("Montant par tâche en retard"), "{settings}");
        assert!(settings.contains(r#"action="/amount/propose""#));

        assert_eq!(h.post("/amount/propose", Some(&ca), &[("amount", "15")]).await.status, 403);
        let bad = h.hx_post("/amount/propose", Some(&cw), &[("amount", "10")]).await;
        assert_eq!(bad.status, 409, "same as the amount in force");
        assert!(bad.body.contains("Indiquez un nombre entier de CHF"), "{}", bad.body);
        let junk = h.hx_post("/amount/propose", Some(&cw), &[("amount", "ten")]).await;
        assert_eq!(junk.status, 409);
        assert_eq!(state(&h), (Centimes(1000), None));

        let proposed = h.hx_post("/amount/propose", Some(&cw), &[("amount", "15")]).await;
        assert_eq!(proposed.status, 200, "{}", proposed.body);
        assert!(proposed.body.contains(r#"id="amount""#), "htmx gets the proposal block");
        assert!(proposed.body.contains("0 témoin(s) sur 2"), "{}", proposed.body);
        assert_eq!(state(&h), (Centimes(1000), Some((Centimes(1500), 0))), "not in force yet");

        assert_eq!(h.post("/amount/approve", Some(&cw), &[]).await.status, 403, "worker cannot approve");
        let one = h.hx_post("/amount/approve", Some(&ca), &[]).await;
        assert_eq!(one.status, 200, "{}", one.body);
        assert!(one.body.contains("1 témoin(s) sur 2"));
        assert_eq!(state(&h), (Centimes(1000), Some((Centimes(1500), 1))), "needs every witness");
        let twice = h.hx_post("/amount/approve", Some(&ca), &[]).await;
        assert_eq!(twice.status, 409);
        assert!(twice.body.contains("Vous êtes déjà d'accord."));

        let last = h.post("/amount/approve", Some(&cb), &[]).await;
        assert_eq!(last.status, 303, "plain form post");
        assert_eq!(last.location(), Some("/settings"));
        assert_eq!(state(&h), (Centimes(1500), None), "in force once all agreed");
        let after = h.get("/settings", Some(&cb)).await.body;
        assert!(after.contains(r#"at-proposal__n">15<"#), "{after}");
    }

    // web-07
    #[test] fn app_33_manifest_served() { todo!() }
    #[test] fn app_33_service_worker_served() { todo!() }
    #[test] fn app_33_push_subscribe_stores_device() { todo!() }

    // web-08
    #[test] fn app_38_pages_default_to_french() { todo!() }
    #[test] fn app_39_account_language_switch_to_english_and_back() { todo!() }
    #[test] fn app_40_pre_login_pages_in_french() { todo!() }
}
