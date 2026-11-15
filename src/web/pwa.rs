//! PWA pieces (APP-33): the web manifest, the service worker at the site
//! root (so its scope is the whole app), and push subscription storage.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::session::{self, internal, lock};
use super::AppState;
use crate::auth::Database;
use crate::notify::VapidKeys;
use crate::store::StoredSubscription;

/// `GET /manifest.webmanifest`.
pub(super) async fn manifest() -> Response {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../../assets/manifest.webmanifest"),
    )
        .into_response()
}

/// `GET /sw.js`.
pub(super) async fn service_worker() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("../../assets/sw.js"),
    )
        .into_response()
}

/// The VAPID public key for `pushManager.subscribe`, if the key file can
/// be read. Without it the settings page leaves the button disabled.
// DECISION: read from `config.vapid_key` when the settings page renders:
// `Notifier` does not expose its keys, and the file is tiny.
pub(super) fn public_key(state: &AppState) -> Option<String> {
    VapidKeys::load(&state.config.vapid_key)
        .map(|k| k.public_key())
        .ok()
}

/// Browser push keys, as `PushSubscription.toJSON()` sends them.
#[derive(Deserialize)]
pub(super) struct Keys {
    p256dh: String,
    auth: String,
}

/// `PushSubscription.toJSON()`; other fields are ignored.
#[derive(Deserialize)]
pub(super) struct Subscribe {
    endpoint: String,
    keys: Keys,
}

/// `POST /push/subscribe`: store one more device of the signed-in
/// account, in the file that holds the account (APP-33, DEV-3).
pub(super) async fn subscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Subscribe>,
) -> Response {
    let actor = match session::actor(&state, &headers) {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => return internal(&e),
    };
    // DECISION: push services are HTTPS; anything else, or empty keys, is
    // refused rather than stored and failing at every send.
    if !body.endpoint.starts_with("https://") || body.keys.p256dh.is_empty() || body.keys.auth.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let sub = StoredSubscription {
        account: actor.account.id,
        endpoint: body.endpoint,
        p256dh: body.keys.p256dh,
        auth: body.keys.auth,
    };
    let stored = match actor.home {
        Database::Ledger => lock(&state.ledger).upsert_subscription(&sub),
        Database::Dev => lock(&state.dev).upsert_subscription(&sub),
    };
    match stored {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(&e),
    }
}
