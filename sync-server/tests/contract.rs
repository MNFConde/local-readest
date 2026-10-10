//! Contract tests: every endpoint is driven through the real router with an
//! in-memory SQLite, asserting the exact wire shapes documented in
//! docs/selfhosted/sync-protocol.md.

use std::sync::Arc;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use sync_server::db::{self, AppState, SharedState};
use sync_server::{config::Config, routes};
use tower::ServiceExt;

/// Build a test config pointing at a throwaway directory.
pub fn test_config(tag: &str) -> Config {
    let data_dir = std::env::temp_dir().join(format!("sync-server-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    Config {
        listen_addr: "127.0.0.1:0".into(),
        jwt_secret: "test-secret-0123456789abcdef".into(),
        data_dir,
        quota_bytes: 1024 * 1024,
        deepl_api_key: None,
        deepl_endpoint: None,
    }
}

pub fn test_state(cfg: Config) -> SharedState {
    let conn = db::open_db_in_memory().expect("in-memory db");
    Arc::new(AppState {
        db: Mutex::new(conn),
        cfg,
    })
}

pub async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header("Authorization", format!("Bearer {t}"));
    }
    let req = builder
        .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json, headers)
}

#[tokio::test]
async fn health_reports_ok() {
    let app = routes::router(test_state(test_config("health")));
    let (status, body, _) = request(app, "GET", "/api/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}
