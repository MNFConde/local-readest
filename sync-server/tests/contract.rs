//! Contract tests: every endpoint is driven through the real router with an
//! in-memory SQLite, asserting the exact wire shapes documented in
//! docs/selfhosted/sync-protocol.md.

use std::sync::Arc;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
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
        max_body_bytes: 512 * 1024 * 1024,
        library_shared: true,
        admin_token: None,
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
    let (status, bytes, headers) =
        request_raw(app, method, uri, token, body, extra_headers).await;
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json, headers)
}

/// Raw variant for non-JSON responses (OPDS XML, book downloads).
pub async fn request_raw(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
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
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.to_vec(), headers)
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

// ── auth, account+password model (T5b) ────────────────────────────────────

#[tokio::test]
async fn login_with_email_and_password_returns_bearer_jwt() {
    let state = test_state(test_config("login-email"));
    let user_id = seed_user(&state, "dana@example.com", "code-dana-123");

    let app = routes::router(state);
    let (status, body, _) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "email": "dana@example.com", "password": "code-dana-123" })),
        &[("X-Device-Id", "dev-dana")],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    // An existing login code works verbatim as the password (zero migration).
    let token = body["access_token"].as_str().expect("access_token");
    let claims = sync_server::jwt::verify("test-secret-0123456789abcdef", token).unwrap();
    assert_eq!(claims.sub, user_id);
    assert_eq!(claims.email.as_deref(), Some("dana@example.com"));
}

#[tokio::test]
async fn login_email_with_wrong_password_is_unauthorized() {
    let state = test_state(test_config("login-email-bad"));
    seed_user(&state, "erin@example.com", "code-erin-123");

    let app = routes::router(state);
    let (status, body, _) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "email": "erin@example.com", "password": "nope" })),
        &[("X-Device-Id", "dev-erin")],
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, serde_json::json!({ "error": "invalid code" }));
}

#[tokio::test]
async fn login_rejects_right_password_with_wrong_email() {
    let state = test_state(test_config("login-email-cross"));
    seed_user(&state, "frank@example.com", "code-frank-123");

    let app = routes::router(state);
    let (status, _, _) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "email": "other@example.com", "password": "code-frank-123" })),
        &[("X-Device-Id", "dev-frank")],
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn passwordless_account_needs_only_email() {
    let state = test_state(test_config("login-empty-pw"));
    {
        let conn = state.db.lock().unwrap();
        sync_server::users::add_user(&conn, "free@example.com", Some(String::new())).unwrap();
        sync_server::users::ensure_user(&conn, "guard@example.com", "code-guard-123").unwrap();
        // Startup-warning source lists exactly the passwordless accounts.
        assert_eq!(
            sync_server::users::empty_password_emails(&conn).unwrap(),
            vec!["free@example.com".to_string()]
        );
    }

    // Empty password logs into the passwordless account…
    let app = routes::router(state.clone());
    let (status, body, _) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "email": "free@example.com", "password": "" })),
        &[("X-Device-Id", "dev-free")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let token = body["access_token"].as_str().expect("access_token");
    let claims = sync_server::jwt::verify("test-secret-0123456789abcdef", token).unwrap();
    assert_eq!(claims.email.as_deref(), Some("free@example.com"));

    // …but never into an account that has a real password.
    let app = routes::router(state);
    let (status, _, _) = request_with_headers(
        app,
        "POST",
        "/api/auth",
        None,
        Some(serde_json::json!({ "email": "guard@example.com", "password": "" })),
        &[("X-Device-Id", "dev-guard")],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── sync (T6) ─────────────────────────────────────────────────────────────

const SECRET: &str = "test-secret-0123456789abcdef";

fn token_for(state: &SharedState, uid: &str) -> String {
    sync_server::jwt::issue(&state.cfg.jwt_secret, uid, Some(uid), "pro").unwrap()
}

#[tokio::test]
async fn sync_push_book_then_pull_returns_db_shape() {
    let state = test_state(test_config("sync-book"));
    let uid = "user-1";
    let token = token_for(&state, uid);

    let book = serde_json::json!({
        "hash": "abc123", "format": "EPUB", "title": "T\u{0000}itle", "author": "Auth",
        "metaHash": "m1", "groupId": "g1",
        "metadata": { "title": "Meta" },
        "createdAt": 1_700_000_000_000i64, "updatedAt": 1_700_000_000_500i64
    });
    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "POST",
        "/api/sync",
        Some(&token),
        Some(json!({ "books": [book] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({}));

    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "GET",
        "/api/sync?since=0&type=books",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let books = body["books"].as_array().expect("books array");
    assert_eq!(books.len(), 1);
    let b = &books[0];
    assert_eq!(b["book_hash"], "abc123");
    assert_eq!(b["meta_hash"], "m1");
    assert_eq!(b["group_id"], "g1");
    assert_eq!(b["title"], "Title");
    assert_eq!(
        b["metadata"],
        serde_json::json!({"title": "Meta"}).to_string()
    );
    assert_eq!(b["created_at"], "2023-11-14T22:13:20Z");
    assert_eq!(b["user_id"], uid);
    assert!(b.get("hash").is_none(), "no camelCase fields in DB shape");
    for key in ["notes", "configs", "statBooks", "statPages"] {
        assert_eq!(body[key], json!([]), "empty {key}");
    }
}

#[tokio::test]
async fn sync_config_progress_piggybacks_onto_book_row() {
    let state = test_state(test_config("sync-piggy"));
    let uid = "user-2";
    let token = token_for(&state, uid);

    let app = routes::router(state.clone());
    let (status, _, _) = request(
        app,
        "POST",
        "/api/sync",
        Some(&token),
        Some(json!({
            "books": [{ "hash": "bk1", "title": "B", "updatedAt": 1_000_000 }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let app = routes::router(state.clone());
    let (status, _, _) = request(app, "POST", "/api/sync", Some(&token), Some(json!({
        "configs": [{ "bookHash": "bk1", "progress": { "location": "p3" }, "updatedAt": 2_000_000 }]
    }))).await;
    assert_eq!(status, StatusCode::OK);

    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "GET",
        "/api/sync?type=books&since=0",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let book = &body["books"][0];
    assert_eq!(
        book["progress"],
        serde_json::json!({"location": "p3"}),
        "config progress must ride along onto the book row"
    );
    assert_eq!(book["updated_at"], "1970-01-01T00:33:20Z");
}

#[tokio::test]
async fn sync_notes_lww_and_deletion_propagate() {
    let state = test_state(test_config("sync-notes"));
    let uid = "user-3";
    let token = token_for(&state, uid);
    let note = |text: &str, updated: i64, deleted: Option<i64>| {
        json!({ "id": "n1", "bookHash": "bh", "type": "highlight", "text": text,
                "updatedAt": updated, "deletedAt": deleted })
    };

    let app = routes::router(state.clone());
    let (_, _, _) = request(
        app,
        "POST",
        "/api/sync",
        Some(&token),
        Some(json!({ "notes": [note("v1", 100, None)] })),
    )
    .await;
    let app = routes::router(state.clone());
    let (_, _, _) = request(
        app,
        "POST",
        "/api/sync",
        Some(&token),
        Some(json!({ "notes": [note("v0-stale", 50, None)] })),
    )
    .await;
    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "GET",
        "/api/sync?type=notes&since=0",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["notes"][0]["text"], "v1");
    assert_eq!(body["notes"][0]["updated_at"], 100);

    let app = routes::router(state.clone());
    let (_, _, _) = request(
        app,
        "POST",
        "/api/sync",
        Some(&token),
        Some(json!({ "notes": [note("", 200, Some(200))] })),
    )
    .await;
    let app = routes::router(state.clone());
    let (_, body, _) = request(
        app,
        "GET",
        "/api/sync?type=notes&since=150",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["notes"].as_array().unwrap().len(),
        1,
        "deleted note must appear after cursor"
    );
    assert_eq!(body["notes"][0]["deleted_at"], 200);
}

#[tokio::test]
async fn sync_books_field_graft_preserves_metadata_clock_edits() {
    let state = test_state(test_config("sync-merge"));
    let uid = "user-4";
    let token = token_for(&state, uid);

    let app = routes::router(state.clone());
    let (_, _, _) = request(
        app,
        "POST",
        "/api/sync",
        Some(&token),
        Some(json!({
            "books": [{ "hash": "m1", "title": "T", "metadata": { "v": 1 },
                        "metadataUpdatedAt": 1_000, "updatedAt": 1_000 }]
        })),
    )
    .await;
    let app = routes::router(state.clone());
    let (_, _, _) = request(
        app,
        "POST",
        "/api/sync",
        Some(&token),
        Some(json!({
            "books": [{ "hash": "m1", "title": "T", "updatedAt": 2_000 }]
        })),
    )
    .await;
    let app = routes::router(state.clone());
    let (_, body, _) = request(
        app,
        "GET",
        "/api/sync?type=books&since=0",
        Some(&token),
        None,
    )
    .await;
    let book = &body["books"][0];
    assert_eq!(
        book["metadata"],
        serde_json::json!({"v": 1}).to_string(),
        "row clock winner must not drop metadata-clock edits"
    );
}

#[tokio::test]
async fn sync_replicas_upsert_filter_and_batch() {
    let state = test_state(test_config("sync-replicas"));
    let uid = "user-5";
    let token = token_for(&state, uid);
    let row = |replica_id: &str, ts: &str| {
        json!({
            "kind": "reader", "replica_id": replica_id,
            "fields_jsonb": { "page": 3 }, "manifest_jsonb": { "keys": [] },
            "updated_at_ts": ts, "schema_version": 1
        })
    };

    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "POST",
        "/api/sync/replicas",
        Some(&token),
        Some(json!({ "rows": [row("r1", "hlc-002"), row("r2", "hlc-001")] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "GET",
        "/api/sync/replicas?kind=reader&since=hlc-001",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "since filter is strict >");
    assert_eq!(rows[0]["replica_id"], "r1", "strictly newer than since");
    assert_eq!(rows[0]["fields_jsonb"]["page"], 3);

    let app = routes::router(state.clone());
    let (_, _, _) = request(
        app,
        "POST",
        "/api/sync/replicas",
        Some(&token),
        Some(json!({ "rows": [row("r2", "hlc-000")] })),
    )
    .await;
    let app = routes::router(state.clone());
    let (_, body, _) = request(
        app,
        "GET",
        "/api/sync/replicas?kind=reader",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["rows"][0]["updated_at_ts"], "hlc-001",
        "stale HLC push must lose"
    );

    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "POST",
        "/api/sync/replicas",
        Some(&token),
        Some(json!({ "cursors": [ { "kind": "reader" } ] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert_eq!(results[0]["kind"], "reader");
    assert_eq!(results[0]["rows"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn sync_replica_keys_crud() {
    let state = test_state(test_config("sync-keys"));
    let uid = "user-6";
    let token = token_for(&state, uid);

    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "POST",
        "/api/sync/replica-keys",
        Some(&token),
        Some(json!({ "alg": "PBKDF2" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let salt_id = body["row"]["saltId"].as_str().unwrap().to_string();
    assert!(!body["row"]["salt"].as_str().unwrap().is_empty());

    let app = routes::router(state.clone());
    let (_, body, _) = request(app, "GET", "/api/sync/replica-keys", Some(&token), None).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 1);
    assert_eq!(body["rows"][0]["saltId"], salt_id.as_str());

    let app = routes::router(state.clone());
    let (status, _, _) = request(app, "DELETE", "/api/sync/replica-keys", Some(&token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let app = routes::router(state);
    let (_, body, _) = request(app, "GET", "/api/sync/replica-keys", Some(&token), None).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn sync_requires_bearer_token() {
    let state = test_state(test_config("sync-auth"));
    let app = routes::router(state.clone());
    let (status, body, _) = request(app, "GET", "/api/sync?since=0", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "AUTH");

    let app = routes::router(state);
    let (status, body, _) = request(app, "GET", "/api/sync?since=0", Some("not-a-jwt"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({ "error": "invalid token", "code": "AUTH" }));
}

// ── storage (T7) ──────────────────────────────────────────────────────────

async fn put_blob(
    app: axum::Router,
    token: &str,
    key: &str,
    data: Vec<u8>,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let req = axum::http::Request::builder()
        .method("PUT")
        .uri(format!("/api/storage/blob/{key}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Length", data.len().to_string())
        .body(axum::body::Body::from(data))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    (status, Value::Null, resp.headers().clone())
}

#[tokio::test]
async fn storage_upload_put_get_roundtrip_with_range() {
    let state = test_state(test_config("storage-rt"));
    let uid = "user-s1";
    let token = token_for(&state, uid);
    let key = format!("{uid}/Readest/Books/bh1/book.epub");

    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "POST",
        "/api/storage/upload",
        Some(&token),
        Some(json!({ "fileName": "Readest/Books/bh1/book.epub", "fileSize": 5 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let expected = format!("/api/storage/blob/{key}");
    assert_eq!(body["uploadUrl"], expected.as_str());
    assert_eq!(body["downloadUrl"], expected.as_str());

    let data = b"hello".to_vec();
    let app = routes::router(state.clone());
    let (status, _, _) = put_blob(app, &token, &key, data).await;
    assert_eq!(status, StatusCode::OK);

    // full GET
    let req = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/api/storage/blob/{key}"))
        .header("Authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = routes::router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"hello");

    // ranged GET
    let req = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/api/storage/blob/{key}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Range", "bytes=1-3")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = routes::router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers()["Content-Range"], "bytes 1-3/5");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"ell");

    // unsatisfiable range
    let req = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/api/storage/blob/{key}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Range", "bytes=99-100")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = routes::router(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(resp.headers()["Content-Range"], "bytes */5");
}

#[tokio::test]
async fn storage_blob_rejects_foreign_scope_and_traversal() {
    let state = test_state(test_config("storage-scope"));
    let _token_a = token_for(&state, "user-a");
    let token_b = token_for(&state, "user-b");
    let foreign_key = "user-a/Readest/Books/x/f";

    let app = routes::router(state.clone());
    let (status, _, _) = put_blob(app, &token_b, foreign_key, b"x".to_vec()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // traversal folded under root never escapes, but a uid mismatch still 403s
    let app = routes::router(state.clone());
    let (status, _, _) = put_blob(app, &token_b, "../../etc/passwd", b"x".to_vec()).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "folded key loses the uid prefix"
    );
}

#[tokio::test]
async fn storage_list_stats_delete_purge_flow() {
    let state = test_state(test_config("storage-list"));
    let uid = "user-s2";
    let token = token_for(&state, uid);

    for (name, content) in [
        ("Readest/Books/bh1/a.epub", "aaaa"),
        ("Readest/Books/bh2/b.epub", "bb"),
    ] {
        let key = format!("{uid}/{name}");
        let app = routes::router(state.clone());
        let (status, _, _) = put_blob(app, &token, &key, content.as_bytes().to_vec()).await;
        assert_eq!(status, StatusCode::OK, "{key}");
    }

    let app = routes::router(state.clone());
    let (status, body, _) = request(app, "GET", "/api/storage/list", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let files = body["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["file_size"], 4, "files: {files:#?}");
    assert_eq!(files[0]["book_hash"], "bh1");
    assert_eq!(body["total"], 2);

    let app = routes::router(state.clone());
    let (_, body, _) = request(
        app,
        "GET",
        "/api/storage/list?bookHash=bh2",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["files"].as_array().unwrap().len(), 1);
    assert_eq!(body["files"][0]["book_hash"], "bh2");

    let app = routes::router(state.clone());
    let (status, body, _) = request(app, "GET", "/api/storage/stats", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["totalFiles"], 2);
    assert_eq!(body["totalSize"], 6);
    assert_eq!(body["quota"], 1024 * 1024i64);
    let by_book = body["byBookHash"].as_array().unwrap();
    assert_eq!(by_book.len(), 2);

    let app = routes::router(state.clone());
    let (status, _, _) = request(
        app,
        "GET",
        &format!("/api/storage/download?fileKey={uid}/Readest/Books/bh1/a.epub"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // foreign scope refused for delete
    let token_b = token_for(&state, "user-s3");
    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "DELETE",
        &format!("/api/storage/delete?fileKey={uid}/Readest/Books/bh1/a.epub"),
        Some(&token_b),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "AUTH");

    let app = routes::router(state.clone());
    let (status, _, _) = request(
        app,
        "DELETE",
        &format!("/api/storage/delete?fileKey={uid}/Readest/Books/bh1/a.epub"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let app = routes::router(state.clone());
    let (status, body, _) = request(
        app,
        "DELETE",
        "/api/storage/purge",
        Some(&token),
        Some(json!({ "fileKeys": [format!("{uid}/Readest/Books/bh2/b.epub"), "user-s3/secret"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deletedCount"], 1);
    assert_eq!(body["failedCount"], 1);
    assert_eq!(body["failed"][0]["error"], "forbidden");
}

#[tokio::test]
async fn storage_quota_enforced_on_blob_put() {
    let state = test_state(test_config("storage-quota"));
    let tiny = Config {
        listen_addr: "127.0.0.1:0".into(),
        jwt_secret: state.cfg.jwt_secret.clone(),
        data_dir: state.cfg.data_dir.clone(),
        quota_bytes: 8,
        max_body_bytes: 512 * 1024 * 1024,
        library_shared: true,
        admin_token: None,
        deepl_api_key: None,
        deepl_endpoint: None,
    };
    let tiny_state = test_state(tiny);
    let tiny_token = token_for(&tiny_state, "user-s4");
    let app = routes::router(tiny_state);
    let (status, body, _) =
        put_blob(app, &tiny_token, "user-s4/Readest/Books/x/f", vec![0u8; 16]).await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "over-quota put must 413"
    );
    assert_eq!(body, Value::Null); // plain-body error path
}

#[tokio::test]
async fn storage_blob_accepts_bodies_over_axum_default_limit() {
    // Axum's silent default body cap is 2 MiB — without an explicit
    // DefaultBodyLimit every real book upload 413s while tiny record payloads
    // pass, which looks exactly like "sync works, upload fails".
    let big_cfg = Config {
        listen_addr: "127.0.0.1:0".into(),
        jwt_secret: "test-secret-0123456789abcdef".into(),
        data_dir: std::env::temp_dir().join(format!(
            "sync-server-test-body-limit-{}",
            std::process::id()
        )),
        quota_bytes: 64 * 1024 * 1024,
        max_body_bytes: 512 * 1024 * 1024,
        library_shared: true,
        admin_token: None,
        deepl_api_key: None,
        deepl_endpoint: None,
    };
    let _ = std::fs::remove_dir_all(&big_cfg.data_dir);
    let state = test_state(big_cfg);
    let token = token_for(&state, "user-s5");
    let app = routes::router(state);
    let big = vec![0xABu8; 2 * 1024 * 1024 + 1];
    let (status, _, _) = put_blob(app, &token, "user-s5/Readest/Books/x/big", big).await;
    assert_eq!(status, StatusCode::OK, "over-2MiB put must pass");
}

// ── content-addressed dedupe + catalog (library feature) ────────────────────

fn blobs_of(cfg: &Config) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(cfg.blobs_root()) {
        for e in entries.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(true) {
                continue;
            }
            names.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

#[cfg(feature = "library")]
#[tokio::test]
async fn storage_dedup_same_bytes_into_one_blob() {
    let cfg = test_config("storage-dedup");
    let state = test_state(cfg.clone());
    let token_a = token_for(&state, "user-da");
    let token_b = token_for(&state, "user-db");
    let app = routes::router(state.clone());
    let body = vec![0x5Au8; 4096];

    let (s1, _, _) = put_blob(
        app.clone(),
        &token_a,
        "user-da/Readest/Books/hashX/book.epub",
        body.clone(),
    )
    .await;
    let (s2, _, _) = put_blob(
        app.clone(),
        &token_b,
        "user-db/Readest/Books/hashX/book.epub",
        body.clone(),
    )
    .await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(blobs_of(&cfg).len(), 1, "identical bytes must share one blob");

    // Both users read their own key back through the hardlink.
    for (tok, key) in [
        (&token_a, "user-da/Readest/Books/hashX/book.epub"),
        (&token_b, "user-db/Readest/Books/hashX/book.epub"),
    ] {
        let resp = request(
            app.clone(),
            "GET",
            &format!("/api/storage/blob/{key}"),
            Some(tok),
            None,
        )
        .await;
        assert_eq!(resp.0, StatusCode::OK);
    }

    // Deleting one user's link must not affect the other's copy.
    let (del, _, _) = request(
        app.clone(),
        "DELETE",
        "/api/storage/delete?fileKey=user-da/Readest/Books/hashX/book.epub",
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(del, StatusCode::OK);
    let resp = request(
        app.clone(),
        "GET",
        "/api/storage/blob/user-db/Readest/Books/hashX/book.epub",
        Some(&token_b),
        None,
    )
    .await;
    assert_eq!(resp.0, StatusCode::OK, "other user's copy must survive");

    // After the last link goes, boot GC sweeps the orphaned blob.
    request(
        app.clone(),
        "DELETE",
        "/api/storage/delete?fileKey=user-db/Readest/Books/hashX/book.epub",
        Some(&token_b),
        None,
    )
    .await;
    let removed = sync_server::storage::gc_blobs(&state.cfg);
    assert_eq!(removed, 1);
    assert!(blobs_of(&cfg).is_empty(), "orphaned blob must be gone");
}

#[cfg(feature = "library")]
#[tokio::test]
async fn catalog_registration_on_book_upload() {
    let cfg = test_config("catalog-reg");
    let state = test_state(cfg.clone());
    let token = token_for(&state, "user-cat");
    let app = routes::router(state.clone());

    // Seed the uploader's book record (as the client does via /api/sync).
    {
        let conn = state.db.lock().unwrap();
        sync_server::store::upsert_book_raw(
            &conn,
            "user-cat",
            &sync_server::store::BookRow {
                book_hash: "hashMeta".into(),
                meta_hash: String::new(),
                updated_at: None,
                deleted_at: None,
                synced_at: String::new(),
                data: json!({
                    "title": "流架构",
                    "author": "James Urquhart",
                    "language": "zh",
                }),
            },
        )
        .unwrap();
    }

    let (s, _, _) = put_blob(
        app,
        &token,
        "user-cat/Readest/Books/hashMeta/流架构.epub",
        b"fake epub".to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let conn = state.db.lock().unwrap();
    let (title, author, language, added_by): (String, String, String, String) = conn
        .query_row(
            "SELECT title, author, language, added_by FROM catalog_books WHERE hash='hashMeta'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(title, "流架构");
    assert_eq!(author, "James Urquhart");
    assert_eq!(language, "zh");
    assert_eq!(added_by, "user-cat");
}

#[cfg(feature = "library")]
#[tokio::test]
async fn catalog_registration_fills_without_book_record() {
    let cfg = test_config("catalog-reg-fallback");
    let state = test_state(cfg);
    let token = token_for(&state, "user-cat2");
    let app = routes::router(state.clone());

    let (s, _, _) = put_blob(
        app,
        &token,
        "user-cat2/Readest/Books/hashBare/Unknown%20Title.epub",
        b"x".to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // No books record: fall back to the filename (extension stripped), and a
    // non-book key (cover.png under a random path) must not register anything.
    let conn = state.db.lock().unwrap();
    let title: String = conn
        .query_row(
            "SELECT title FROM catalog_books WHERE hash='hashBare'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(title, "Unknown Title");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM catalog_books", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "only the Books/<hash>/ key registers");
}

// ── OPDS catalog (library feature) ──────────────────────────────────────────

fn basic_header(email: &str, code: &str) -> (String, String) {
    use base64::Engine;
    (
        "Authorization".into(),
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{email}:{code}"))
        ),
    )
}

async fn upload_catalog_book(state: &SharedState, uid: &str, hash: &str, title: &str) {
    {
        let conn = state.db.lock().unwrap();
        sync_server::store::upsert_book_raw(
            &conn,
            uid,
            &sync_server::store::BookRow {
                book_hash: hash.into(),
                meta_hash: String::new(),
                updated_at: None,
                deleted_at: None,
                synced_at: String::new(),
                data: json!({ "title": title, "author": "Someone", "language": "zh" }),
            },
        )
        .unwrap();
    }
    let token = token_for(state, uid);
    let path_title = title.replace(' ', "%20");
    let (s, _, _) = put_blob(
        routes::router(state.clone()),
        &token,
        &format!("{uid}/Readest/Books/{hash}/{path_title}.epub"),
        b"fake epub bytes".to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[cfg(feature = "library")]
async fn opds_get(
    app: axum::Router,
    uri: &str,
    auth: &(String, String),
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    request_raw(
        app,
        "GET",
        uri,
        None,
        None,
        &[(auth.0.as_str(), auth.1.as_str())],
    )
    .await
}

#[cfg(feature = "library")]
#[tokio::test]
async fn opds_requires_basic_auth() {
    let state = test_state(test_config("opds-auth"));
    let app = routes::router(state);
    let (status, _, headers) = request_raw(app, "GET", "/opds", None, None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .starts_with("Basic "));
}

#[cfg(feature = "library")]
#[tokio::test]
async fn opds_feed_download_cover_roundtrip() {
    let cfg = test_config("opds-roundtrip");
    let state = test_state(cfg.clone());
    let uid = seed_user(&state, "opds@t.test", "opds-code-1");
    let token = token_for(&state, &uid);

    upload_catalog_book(&state, &uid, "hashFeed", "FeedBook").await;

    let app = routes::router(state);
    let auth = basic_header("opds@t.test", "opds-code-1");

    // Root navigation feed.
    let (status, xml, headers) = opds_get(app.clone(), "/opds", &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .contains("opds-catalog"));
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("<feed xmlns=") && xml.contains("/opds/books"));

    // Acquisition feed carries the registered title + acquisition link.
    let (status, xml, _) = opds_get(app.clone(), "/opds/books", &auth).await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("FeedBook"), "feed must carry the book title");
    assert!(xml.contains("/opds/download/hashFeed"));
    assert!(xml.contains("application/epub+zip"));

    // Wrong code is still 401.
    let bad = basic_header("opds@t.test", "wrong-code");
    let (status, _, _) = opds_get(app.clone(), "/opds/books", &bad).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Download round-trip with the exact uploaded bytes.
    let (status, bytes, headers) =
        opds_get(app.clone(), "/opds/download/hashFeed", &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"fake epub bytes");
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/epub+zip")
    );
    assert!(headers
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .contains("filename*=UTF-8''FeedBook.epub"));

    // No cover uploaded yet → 404; upload one and it serves.
    let (status, _, _) = opds_get(app.clone(), "/opds/cover/hashFeed", &auth).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (s, _, _) = put_blob(
        app.clone(),
        &token,
        &format!("{uid}/Readest/Books/hashFeed/cover.png"),
        b"\x89PNG fake".to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (status, bytes, headers) =
        opds_get(app, "/opds/cover/hashFeed", &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"\x89PNG fake");
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("image/png")
    );
}

#[cfg(feature = "library")]
#[tokio::test]
async fn opds_shared_visibility_search_and_flag() {
    let cfg = test_config("opds-shared");
    let state = test_state(cfg.clone());
    let uid_a = seed_user(&state, "a@t.test", "code-a");
    let uid_b = seed_user(&state, "b@t.test", "code-b");

    upload_catalog_book(&state, &uid_a, "hashA", "BookOfA").await;
    upload_catalog_book(&state, &uid_b, "hashB", "BookOfB").await;

    let auth_a = basic_header("a@t.test", "code-a");
    let app = routes::router(state);

    // Shared by default: A sees both books.
    let (status, xml, _) =
        opds_get(app.clone(), "/opds/books", &auth_a).await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("BookOfA") && xml.contains("BookOfB"));

    // Search filters by title.
    let (status, xml, _) =
        opds_get(app.clone(), "/opds/search?q=BookOfB", &auth_a).await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("BookOfB") && !xml.contains("BookOfA"));

    // LIBRARY_SHARED=0: the same request now only returns A's own upload.
    let mut cfg_private = cfg.clone();
    cfg_private.library_shared = false;
    let state_private = test_state(cfg_private);
    let uid_a2 = seed_user(&state_private, "a2@t.test", "code-a2");
    upload_catalog_book(&state_private, &uid_a2, "hashA2", "OnlyMine").await;
    // seed a second user + book that must NOT appear for a2
    let uid_b2 = seed_user(&state_private, "b2@t.test", "code-b2");
    upload_catalog_book(&state_private, &uid_b2, "hashB2", "NotYours").await;

    let auth_a2 = basic_header("a2@t.test", "code-a2");
    let app_private = routes::router(state_private);
    let (status, xml, _) =
        opds_get(app_private, "/opds/books", &auth_a2).await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("OnlyMine"));
    assert!(!xml.contains("NotYours"), "private library hides other users");
}

#[cfg(feature = "library")]
#[tokio::test]
async fn opds_feed_groups_books_by_work() {
    let mut cfg = test_config("opds-works");
    cfg.admin_token = Some("admin-secret-token".into());
    let state = test_state(cfg);
    let uid = seed_user(&state, "works@t.test", "code-works");
    upload_catalog_book(&state, &uid, "hashW1", "Edition One").await;
    upload_catalog_book(&state, &uid, "hashW2", "Edition Two").await;

    // Group both editions under one work via the admin API.
    let app = routes::router(state.clone());
    let tok = Some("admin-secret-token");
    let (status, body, _) = admin_request(
        app.clone(),
        "POST",
        "/api/admin/works",
        tok,
        Some(json!({ "title": "Grouped Work", "author": "X" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let work_id = body["id"].as_str().unwrap().to_string();
    for h in ["hashW1", "hashW2"] {
        let (status, _, _) = admin_request(
            app.clone(),
            "POST",
            &format!("/api/admin/books/{h}"),
            tok,
            Some(json!({ "workId": work_id })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let auth = basic_header("works@t.test", "code-works");

    // Root feed links the works section; the works feed lists the group.
    let (status, xml, _) = opds_get(app.clone(), "/opds", &auth).await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("/opds/works"));

    let (status, xml, _) = opds_get(app.clone(), "/opds/works", &auth).await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("Grouped Work"));
    assert!(xml.contains(&format!("/opds/work/{work_id}")));

    // The work feed lists every edition with its acquisition link; the
    // plain books feed exposes the group via rel="collection" links.
    let (status, xml, _) =
        opds_get(app.clone(), &format!("/opds/work/{work_id}"), &auth).await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("Edition One") && xml.contains("Edition Two"));
    assert!(xml.contains("/opds/download/hashW1"));
    assert!(xml.contains("/opds/download/hashW2"));

    let (status, xml, _) = opds_get(app, "/opds/books", &auth).await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains(&format!(
        "rel=\"collection\" href=\"/opds/work/{work_id}\" title=\"Grouped Work\""
    )));
}

// ── admin API + page (library feature) ──────────────────────────────────────

async fn admin_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    admin_token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let bearer = admin_token.map(|t| format!("Bearer {t}"));
    let extra: Vec<(String, String)> = bearer.map(|b| vec![("Authorization".to_string(), b)]).unwrap_or_default();
    let extra_refs: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    request_with_headers(app, method, uri, None, body, &extra_refs).await
}

#[cfg(feature = "library")]
#[tokio::test]
async fn admin_disabled_without_token_and_auth_enforced() {
    // No ADMIN_TOKEN: admin API 404s entirely, page still renders.
    let state = test_state(test_config("admin-off"));
    let app = routes::router(state.clone());
    let (status, _, _) = admin_request(app.clone(), "GET", "/api/admin/books", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body, headers) =
        request_raw(app, "GET", "/admin", None, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .contains("text/html"));
    assert!(body.len() > 100);

    // With a token: wrong/missing bearer → 401, right bearer → 200.
    let mut cfg = test_config("admin-on");
    cfg.admin_token = Some("admin-secret-token".into());
    let state = test_state(cfg);
    let app = routes::router(state);
    let (status, _, _) = admin_request(app.clone(), "GET", "/api/admin/books", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) =
        admin_request(app.clone(), "GET", "/api/admin/books", Some("wrong"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body, _) =
        admin_request(app, "GET", "/api/admin/books", Some("admin-secret-token"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["books"], json!([]));
}

#[cfg(feature = "library")]
#[tokio::test]
async fn admin_book_update_work_grouping_and_source_hash() {
    let mut cfg = test_config("admin-update");
    cfg.admin_token = Some("admin-secret-token".into());
    let state = test_state(cfg.clone());
    let uid = seed_user(&state, "admin@t.test", "admin-code-1");
    upload_catalog_book(&state, &uid, "hashE1", "Book E1").await;
    upload_catalog_book(&state, &uid, "hashE2", "Book E2").await;

    let app = routes::router(state.clone());
    let tok = Some("admin-secret-token");

    // Create a manual work, group both books under it, tag lineage.
    let (status, body, _) =
        admin_request(app.clone(), "POST", "/api/admin/works", tok, Some(json!({ "title": "Same Work", "author": "X" }))).await;
    assert_eq!(status, StatusCode::OK);
    let work_id = body["id"].as_str().unwrap().to_string();

    for h in ["hashE1", "hashE2"] {
        let (status, _, _) = admin_request(
            app.clone(),
            "POST",
            &format!("/api/admin/books/{h}"),
            tok,
            Some(json!({ "workId": work_id, "sourceHash": "hashOriginal" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body, _) = admin_request(app.clone(), "GET", "/api/admin/works", tok, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["works"][0]["books"], 2, "both books grouped");

    // Metadata edit lands.
    let (status, body, _) = admin_request(
        app.clone(),
        "POST",
        "/api/admin/books/hashE1",
        tok,
        Some(json!({ "title": "Renamed", "language": "en" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body, _) = admin_request(app, "GET", "/api/admin/books", tok, None).await;
    assert_eq!(status, StatusCode::OK);
    let e1 = body["books"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["hash"] == "hashE1")
        .unwrap();
    assert_eq!(e1["title"], "Renamed");
    assert_eq!(e1["language"], "en");
    assert_eq!(e1["sourceHash"], "hashOriginal");
    assert_eq!(e1["workId"], work_id);
}

#[cfg(feature = "library")]
#[tokio::test]
async fn admin_user_state_and_enrich_offline_shape() {
    let mut cfg = test_config("admin-users");
    cfg.admin_token = Some("admin-secret-token".into());
    let state = test_state(cfg);
    seed_user(&state, "u1@t.test", "code-u1");
    seed_user(&state, "u2@t.test", "code-u2");

    let app = routes::router(state.clone());
    let tok = Some("admin-secret-token");
    let (status, body, _) = admin_request(app.clone(), "GET", "/api/admin/users", tok, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["users"].as_array().unwrap().len(), 2);

    let (status, body, _) = admin_request(
        app.clone(),
        "POST",
        "/api/admin/users/u1%40t.test/state",
        tok,
        Some(json!({ "disabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let conn = state.db.lock().unwrap();
    let disabled: i64 = conn
        .query_row("SELECT disabled FROM users WHERE email='u1@t.test'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(disabled, 1);

    // Offline enrich: unreachable network is tolerated and reported per book.
    drop(conn);
    let (status, body, _) = admin_request(app, "POST", "/api/admin/enrich", tok, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["enriched"].is_i64());
    assert!(body["failed"].is_array());
}

// ── proxies (T8) — offline paths only; upstream-forwarding paths need network ──

#[tokio::test]
async fn deepl_mock_echoes_without_key() {
    let state = test_state(test_config("proxy-deepl"));
    let token = token_for(&state, "user-p");
    let app = routes::router(state);
    let (status, body, _) = request(
        app,
        "POST",
        "/api/deepl/translate",
        Some(&token),
        Some(json!({ "text": ["a", "b"], "target_lang": "DE" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["translations"], json!([{"text": "a"}, {"text": "b"}]));
}

#[tokio::test]
async fn deepl_malformed_body_is_validation_error() {
    let state = test_state(test_config("proxy-deepl-bad"));
    let token = token_for(&state, "user-p");
    let app = routes::router(state);
    let (status, body, _) = request(
        app,
        "POST",
        "/api/deepl/translate",
        Some(&token),
        Some(json!("not-an-object")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "VALIDATION");
}

#[tokio::test]
async fn metadata_search_requires_q() {
    let state = test_state(test_config("proxy-meta"));
    let token = token_for(&state, "user-p");
    let app = routes::router(state);
    let (status, body, _) = request(app, "GET", "/api/metadata/search", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({ "error": "missing q", "code": "VALIDATION" }));
}

#[tokio::test]
async fn tts_synthesis_is_a_documented_gap() {
    let state = test_state(test_config("proxy-tts"));
    let token = token_for(&state, "user-p");
    let app = routes::router(state);
    let (status, body, _) = request(
        app,
        "POST",
        "/api/tts/edge",
        Some(&token),
        Some(json!({ "text": "hi", "voice": "en-US-AriaNeural" })),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["code"], "SERVER");
}

#[tokio::test]
async fn proxies_require_auth() {
    let state = test_state(test_config("proxy-auth"));
    let app = routes::router(state.clone());
    let (status, _, _) = request(
        app,
        "POST",
        "/api/deepl/translate",
        None,
        Some(json!({ "text": ["a"] })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let app = routes::router(state.clone());
    let (status, _, _) = request(app, "GET", "/api/metadata/search?q=x", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
