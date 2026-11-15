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
    let _state = state;
    todo!()
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
    let _ = (state, addr);
    todo!()
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
        let _ = f;
        todo!()
    }
}

impl std::error::Error for WebError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    // web-01
    #[test] fn web_login_sets_session() { todo!() }
    #[test] fn web_wrong_password_rejected() { todo!() }
    #[test] fn dev_3_admin_requests_use_dev_store() { todo!() }

    // web-02
    #[test] fn app_26_worker_creates_invite_link_shown_and_mailed() { todo!() }
    #[test] fn app_27_invite_accept_sets_witness_password() { todo!() }

    // web-03
    #[test] fn app_28_reset_request_sends_mail() { todo!() }
    #[test] fn web_expired_or_used_link_shows_error_page() { todo!() }

    // web-04
    #[test] fn app_31_worker_view_shows_titles_debts_totals() { todo!() }

    // web-05
    #[test] fn app_32_witness_view_never_contains_task_title() { todo!() }

    // web-06
    #[test] fn app_18_claim_action_second_claim_rejected() { todo!() }
    #[test] fn app_21_mark_sent_action_worker_only() { todo!() }
    #[test] fn app_22_mark_received_action_owner_only() { todo!() }
    #[test] fn app_16_amount_propose_and_approve_actions() { todo!() }

    // web-07
    #[test] fn app_33_manifest_served() { todo!() }
    #[test] fn app_33_service_worker_served() { todo!() }
    #[test] fn app_33_push_subscribe_stores_device() { todo!() }

    // web-08
    #[test] fn app_38_pages_default_to_french() { todo!() }
    #[test] fn app_39_account_language_switch_to_english_and_back() { todo!() }
    #[test] fn app_40_pre_login_pages_in_french() { todo!() }
}
