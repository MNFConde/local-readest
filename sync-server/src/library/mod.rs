//! The online-library side: content-addressed catalog, OPDS feeds, works
//! auto-grouping, and the admin page. One switchable module tree (`library`
//! cargo feature) so the sync core ships without it and a future split into
//! its own binary stays a mechanical move. Everything here treats sync data
//! (users/books/blobs) as read-only except the catalog tables it owns.

pub mod admin;
pub mod catalog;
pub mod ol;
pub mod opds;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Internal-error response for the library handlers.
pub(crate) fn admin_error(e: impl std::fmt::Display) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}
