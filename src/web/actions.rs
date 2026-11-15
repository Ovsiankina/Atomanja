//! Debt and amount actions: claim, mark sent, mark received, propose,
//! approve. Each is an htmx POST answered with the re-rendered fragment,
//! or a plain form POST answered with a redirect (APP-16..APP-22).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use super::handlers::require;
use super::i18n::msgs;
use super::AppState;
use crate::auth::{self, Action};

/// `POST /debts/{id}/claim` (APP-18).
pub(super) async fn claim(
    State(state): State<AppState>,
    Path(debt): Path<u32>,
    headers: HeaderMap,
) -> Response {
    let actor = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let m = msgs(actor.account.lang);
    if !auth::permits(&actor.account, Action::Claim) {
        return (StatusCode::FORBIDDEN, m.err_forbidden).into_response();
    }
    let _ = debt;
    StatusCode::NOT_IMPLEMENTED.into_response()
}
