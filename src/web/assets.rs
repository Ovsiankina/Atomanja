//! Static files compiled into the binary: design-system styles, the
//! page frame stylesheet, vendored htmx 2.0.4 (never a CDN at runtime,
//! HLD), the client script and the icon.

use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

// DECISION: a fixed table of `include_str!` files matched by name, so
// nothing on disk is read at runtime and an unknown name is a plain 404.
const FILES: &[(&str, &str, &str)] = &[
    ("tokens.css", "text/css; charset=utf-8", include_str!("../../assets/tokens.css")),
    ("bundle.css", "text/css; charset=utf-8", include_str!("../../assets/bundle.css")),
    ("app.css", "text/css; charset=utf-8", include_str!("../../assets/app.css")),
    ("htmx.min.js", "text/javascript; charset=utf-8", include_str!("../../assets/htmx.min.js")),
    ("app.js", "text/javascript; charset=utf-8", include_str!("../../assets/app.js")),
    ("icon.svg", "image/svg+xml", include_str!("../../assets/icon.svg")),
];

/// `GET /assets/{name}`.
pub(super) async fn asset(Path(name): Path<String>) -> Response {
    match FILES.iter().find(|(n, _, _)| *n == name) {
        Some((_, mime, body)) => (
            [
                (header::CONTENT_TYPE, *mime),
                (header::CACHE_CONTROL, "public, max-age=3600"),
            ],
            *body,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
