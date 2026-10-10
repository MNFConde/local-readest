use axum::routing::{any, delete, get, post, put};
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
        .route("/api/storage/upload", post(crate::storage::upload))
        .route(
            "/api/storage/download",
            get(crate::storage::download).post(crate::storage::download_batch),
        )
        .route("/api/storage/list", get(crate::storage::list))
        .route("/api/storage/stats", get(crate::storage::stats))
        .route("/api/storage/delete", delete(crate::storage::delete))
        .route("/api/storage/purge", delete(crate::storage::purge))
        .route(
            "/api/storage/blob/{*key}",
            put(crate::storage::blob_put).get(crate::storage::blob_get),
        )
        .route("/api/deepl/translate", post(crate::proxy::deepl_translate))
        .route(
            "/api/yandex-translate",
            post(crate::proxy::yandex_translate),
        )
        .route("/api/tts/edge", any(crate::proxy::edge_tts))
        .route("/api/metadata/search", get(crate::proxy::metadata_search))
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
