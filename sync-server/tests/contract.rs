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
    let data_dir =
        std::env::temp_dir().join(format!("sync-server-test-{tag}-{}", std::process::id()));
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
        login_limiter: Mutex::new(sync_server::ratelimit::LoginLimiter::new()),
    })
}

pub async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    request_with_headers(app, method, uri, token, body, &[]).await
}

pub async fn request_with_headers(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header("Authorization", format!("Bearer {t}"));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
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

// ── auth (T5) ─────────────────────────────────────────────────────────────

fn seed_user(state: &SharedState, email: &str, code: &str) -> String {
    let conn = state.db.lock().unwrap();
    sync_server::users::ensure_user(&conn, email, code).unwrap();
    // ensure_user may reuse; fetch the id via verify
    match sync_server::users::verify_login(&conn, code).unwrap() {
        Some(u) => u.id,
        None => panic!("seeded user must verify"),
    }
}

#[tokio::test]
async fn login_success_returns_bearer_jwt_with_pro_plan() {
    let state = test_state(test_config("login-ok"));
    let user_id = seed_user(&state, "alice@example.com", "code-alice-123");

    let app = routes::router(state);
    let (status, body, _) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "code": "code-alice-123" })),
        &[("X-Device-Id", "dev-1")],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["token_type"], "bearer");
    let token = body["access_token"].as_str().expect("access_token");
    let claims = sync_server::jwt::verify("test-secret-0123456789abcdef", token).unwrap();
    assert_eq!(claims.sub, user_id);
    assert_eq!(claims.plan, "pro");
    assert_eq!(claims.email.as_deref(), Some("alice@example.com"));
}

#[tokio::test]
async fn login_wrong_code_is_unauthorized_with_exact_body() {
    let state = test_state(test_config("login-bad"));
    seed_user(&state, "bob@example.com", "code-bob-123");

    let app = routes::router(state);
    let (status, body, _) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "code": "wrong" })),
        &[("X-Device-Id", "dev-2")],
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, serde_json::json!({ "error": "invalid code" }));
}

#[tokio::test]
async fn login_locks_device_after_three_consecutive_failures() {
    let state = test_state(test_config("login-lock"));
    seed_user(&state, "carol@example.com", "code-carol-123");
    let headers = [("X-Device-Id", "dev-attacker")];

    for _ in 0..3 {
        let app = routes::router(state.clone());
        let (status, _, _) = request_with_headers(
            app,
            "POST",
            "/api/auth",
            None,
            Some(serde_json::json!({ "code": "wrong" })),
            &headers,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // Even the correct code is rejected while the lock is active.
    let app = routes::router(state);
    let (status, body, resp_headers) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "code": "code-carol-123" })),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "LOCKED");
    assert!(
        resp_headers.get("Retry-After").is_some(),
        "Retry-After must be present"
    );
}

#[tokio::test]
async fn login_malformed_body_is_bad_request() {
    let state = test_state(test_config("login-malformed"));
    let app = routes::router(state);
    let (status, body, _) = request(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "nope": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, serde_json::json!({ "error": "bad request" }));
}

#[tokio::test]
async fn disabled_user_cannot_login() {
    let state = test_state(test_config("login-disabled"));
    seed_user(&state, "eve@example.com", "code-eve-123");
    {
        let conn = state.db.lock().unwrap();
        sync_server::users::set_disabled(&conn, "eve@example.com", true).unwrap();
    }
    let app = routes::router(state);
    let (status, _, _) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "code": "code-eve-123" })),
        &[("X-Device-Id", "dev-eve")],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
