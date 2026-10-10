use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::db::SharedState;

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/auth", post(crate::auth::login))
        .route(
            "/api/sync",
            get(crate::sync::sync_pull).post(crate::sync::sync_push),
        )
        .route(
            "/api/sync/replicas",
            get(crate::sync::replicas_get).post(crate::sync::replicas_post),
        )
        .route(
            "/api/sync/replica-keys",
            get(crate::sync::replica_keys_get)
                .post(crate::sync::replica_keys_post)
                .delete(crate::sync::replica_keys_delete),
        )
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

/// Convenience accessor used by handlers that need the DB behind the mutex.
pub fn with_db<T>(
    state: &SharedState,
    f: impl FnOnce(&mut rusqlite::Connection) -> Result<T, crate::error::ApiError>,
) -> Result<T, crate::error::ApiError> {
    let mut conn = state
        .db
        .lock()
        .map_err(|_| crate::error::ApiError::server(anyhow::anyhow!("db mutex poisoned")))?;
    f(&mut conn)
}
