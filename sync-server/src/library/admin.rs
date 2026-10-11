//! Minimal admin API + page: catalog maintenance (works grouping, metadata
//! fixes, source_hash lineage), blob GC, and user enable/disable. Auth is a
//! single shared bearer token (`ADMIN_TOKEN` env); with the variable unset
//! the API routes return 404 entirely — the safe default. Account creation
//! and code resets stay CLI-only.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};

use crate::db::SharedState;

pub fn router(state: SharedState) -> Router<SharedState> {
    Router::new()
        .route("/books", get(list_books))
        .route("/books/{hash}", post(update_book))
        .route("/works", get(list_works).post(create_work))
        .route("/enrich", post(enrich))
        .route("/gc", post(gc))
        .route("/users", get(list_users))
        .route("/users/{email}/state", post(set_user_state))
        .with_state(state)
}

/// Single-file admin page. No build chain: inline HTML+JS, token kept in
/// localStorage, everything else is fetch + render.
pub async fn admin_page() -> Response {
    let html = include_str!("admin.html");
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

// ── auth ────────────────────────────────────────────────────────────────────

async fn require_admin(state: &SharedState, headers: &HeaderMap) -> Result<(), Response> {
    let Some(expected) = state.cfg.admin_token.as_deref() else {
        // No token configured: admin surface stays disabled.
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if presented.is_empty() {
        return Err(StatusCode::UNAUTHORIZED.into_response());
    }
    // Constant-time-ish comparison via SHA-256 of both sides.
    use sha2::{Digest, Sha256};
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(presented.as_bytes());
    if a.as_slice() != b.as_slice() {
        return Err(StatusCode::UNAUTHORIZED.into_response());
    }
    Ok(())
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn list_books(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let conn = state.db.lock().map_err(|_| super::admin_error("db lock poisoned"))?;
    let mut stmt = conn
        .prepare(
            "SELECT hash, title, author, language, work_id, source_hash, added_at, added_by
             FROM catalog_books ORDER BY added_at DESC",
        )
        .map_err(|e| super::admin_error(e))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "hash": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "author": r.get::<_, String>(2)?,
                "language": r.get::<_, String>(3)?,
                "workId": r.get::<_, Option<String>>(4)?,
                "sourceHash": r.get::<_, Option<String>>(5)?,
                "addedAt": r.get::<_, String>(6)?,
                "addedBy": r.get::<_, String>(7)?,
            }))
        })
        .map_err(|e| super::admin_error(e))?
        .flatten()
        .collect::<Vec<_>>();
    Ok(Json(json!({ "books": rows })).into_response())
}

async fn update_book(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let payload: Value =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;

    // A work_id means "group under this work" — create a placeholder work row
    // for manual ids so the works listing stays consistent; `null` ungroups.
    let work_id = match payload.get("workId") {
        Some(Value::Null) => None,
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(_) => return Err(StatusCode::BAD_REQUEST.into_response()),
        None => None,
    };
    let setting_work = matches!(payload.get("workId"), Some(Value::String(s)) if !s.is_empty());

    let mut sets = Vec::new();
    let mut args: Vec<String> = Vec::new();
    for field in ["title", "author", "language", "sourceHash"] {
        if let Some(v) = payload.get(field).and_then(Value::as_str) {
            let col = match field {
                "sourceHash" => "source_hash",
                other => other,
            };
            sets.push(format!("{col} = ?{}", args.len() + 1));
            args.push(v.to_string());
        }
    }
    if let Some(w) = &work_id {
        sets.push(format!("work_id = ?{}", args.len() + 1));
        args.push(w.clone());
    }
    if sets.is_empty() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    args.push(hash.clone());

    let conn = state.db.lock().map_err(|_| super::admin_error("db lock poisoned"))?;
    if setting_work {
        conn.execute(
            "INSERT OR IGNORE INTO works (id, title, author) VALUES (?1, '', '')",
            params![work_id],
        )
        .map_err(|e| super::admin_error(e))?;
    }
    let n = conn
        .execute(
            &format!("UPDATE catalog_books SET {} WHERE hash = ?{}", sets.join(", "), args.len()),
            rusqlite::params_from_iter(args.iter()),
        )
        .map_err(|e| super::admin_error(e))?;
    Ok(Json(json!({ "updated": n })).into_response())
}

async fn list_works(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let conn = state.db.lock().map_err(|_| super::admin_error("db lock poisoned"))?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.title, w.author, COUNT(b.hash)
             FROM works w LEFT JOIN catalog_books b ON b.work_id = w.id
             GROUP BY w.id ORDER BY w.id",
        )
        .map_err(|e| super::admin_error(e))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "author": r.get::<_, String>(2)?,
                "books": r.get::<_, i64>(3)?,
            }))
        })
        .map_err(|e| super::admin_error(e))?
        .flatten()
        .collect::<Vec<_>>();
    Ok(Json(json!({ "works": rows })).into_response())
}

async fn create_work(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let payload: Value =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let title = payload.get("title").and_then(Value::as_str).unwrap_or("");
    let author = payload.get("author").and_then(Value::as_str).unwrap_or("");
    use rand::Rng;
    let id = format!("manual-{}", hex::encode(rand::rng().random::<[u8; 4]>()));
    let conn = state.db.lock().map_err(|_| super::admin_error("db lock poisoned"))?;
    conn.execute(
        "INSERT INTO works (id, title, author) VALUES (?1, ?2, ?3)",
        params![id, title, author],
    )
    .map_err(|e| super::admin_error(e))?;
    Ok(Json(json!({ "id": id })).into_response())
}

/// Auto-group ungrouped books via Open Library. Network failures are
/// tolerated per book and reported; offline use just yields failures.
async fn enrich(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let titles: Vec<(String, String, String)> = {
        let conn = state.db.lock().map_err(|_| super::admin_error("db lock poisoned"))?;
        let mut stmt = conn
            .prepare(
                "SELECT hash, title, author FROM catalog_books
                 WHERE work_id IS NULL AND title != ''",
            )
            .map_err(|e| super::admin_error(e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })
            .map_err(|e| super::admin_error(e))?
            .flatten();
        rows.collect()
    };
    let client = reqwest::Client::new();
    let mut enriched = 0;
    let mut failed = Vec::new();
    for (hash, title, author) in &titles {
        match super::ol::lookup_work(&client, title, author).await {
            Ok(Some(hit)) => {
                let conn = state.db.lock().map_err(|_| super::admin_error("db lock poisoned"))?;
                conn.execute(
                    "INSERT OR IGNORE INTO works (id, title, author) VALUES (?1, '', '')",
                    params![hit.work_id],
                )
                .map_err(|e| super::admin_error(e))?;
                conn.execute(
                    "UPDATE catalog_books SET work_id = ?2,
                     language = CASE WHEN language = '' THEN ?3 ELSE language END
                     WHERE hash = ?1",
                    params![hash, hit.work_id, hit.language],
                )
                .map_err(|e| super::admin_error(e))?;
                enriched += 1;
            }
            Ok(None) => failed.push(json!({ "hash": hash, "error": "no OL match" })),
            Err(e) => failed.push(json!({ "hash": hash, "error": e.to_string() })),
        }
    }
    Ok(Json(json!({ "enriched": enriched, "failed": failed })).into_response())
}

async fn gc(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let removed = crate::storage::gc_blobs(&state.cfg);
    Ok(Json(json!({ "removed": removed })).into_response())
}

async fn list_users(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let conn = state.db.lock().map_err(|_| super::admin_error("db lock poisoned"))?;
    let rows = crate::users::list_users(&conn).map_err(|e| super::admin_error(e))?;
    let users: Vec<Value> = rows
        .iter()
        .map(|(email, id, disabled, created)| {
            json!({ "email": email, "id": id, "disabled": disabled, "createdAt": created })
        })
        .collect();
    Ok(Json(json!({ "users": users })).into_response())
}

async fn set_user_state(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(email): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let payload: Value =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let disabled = payload.get("disabled").and_then(Value::as_bool).unwrap_or(false);
    let conn = state.db.lock().map_err(|_| super::admin_error("db lock poisoned"))?;
    let n = crate::users::set_disabled(&conn, &email, disabled).map_err(|e| super::admin_error(e))?;
    Ok(Json(json!({ "updated": n })).into_response())
}
