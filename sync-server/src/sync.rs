//! /api/sync (books/notes/configs/stats channels), /api/sync/replicas and
//! /api/sync/replica-keys — wire behavior per docs/selfhosted/sync-protocol.md §2–§3.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::value::RawValue;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db::SharedState;
use crate::error::ApiError;
use crate::rfc3339;
use crate::store;
use crate::transform::{
    book_to_db, config_to_db, note_to_db, PushedBook, PushedConfig, PushedNote,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncResult {
    books: Vec<Box<RawValue>>,
    notes: Vec<Box<RawValue>>,
    configs: Vec<Box<RawValue>>,
    stat_books: Vec<Value>,
    stat_pages: Vec<Value>,
}

fn empty_result() -> SyncResult {
    SyncResult {
        books: vec![],
        notes: vec![],
        configs: vec![],
        stat_books: vec![],
        stat_pages: vec![],
    }
}

fn raw_values(items: Vec<String>) -> Vec<Box<RawValue>> {
    items
        .into_iter()
        .filter_map(|s| RawValue::from_string(s).ok())
        .collect()
}

/// GET /api/sync?since=<ms>&type=books|notes|configs|stats
pub async fn sync_pull(
    State(state): State<SharedState>,
    user: AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let since_ms: i64 = q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
    let typ = q.get("type").map(String::as_str).unwrap_or("");
    let uid = user.id.as_str();

    let db_lock = state.db.lock().expect("db mutex poisoned");
    let result = match typ {
        "" | "books" => {
            let mut r = empty_result();
            // Go: isoFromMs (RFC3339Nano, trailing-zero-trimmed) over the ms cursor.
            let rows = store::pull_books(&db_lock, uid, &rfc3339::iso_from_ms(since_ms), 1000)?;
            r.books = rows
                .into_iter()
                .filter_map(|b| RawValue::from_string(b.data).ok())
                .collect();
            r
        }
        "notes" => {
            let mut r = empty_result();
            r.notes = raw_values(store::pull_notes(&db_lock, uid, since_ms)?);
            r
        }
        "configs" => {
            let mut r = empty_result();
            r.configs = raw_values(store::pull_configs(&db_lock, uid, since_ms)?);
            r
        }
        "stats" => {
            let mut r = empty_result();
            r.stat_books = store::pull_stat_books(&db_lock, uid, since_ms)?;
            r.stat_pages = store::pull_stat_pages(&db_lock, uid, since_ms)?;
            r
        }
        _ => {
            return Ok((
                StatusCode::BAD_REQUEST,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                "{\"error\":\"unknown type\"}\n",
            )
                .into_response());
        }
    };
    Ok(Json(result).into_response())
}

/// POST /api/sync — push payload; records are transformed to DB shape before
/// storage. Malformed individual records are skipped silently (Go behavior).
pub async fn sync_push(
    State(state): State<SharedState>,
    user: AuthUser,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let payload: Value = serde_json::from_slice(&body).map_err(|_| ApiError::Validation)?;
    let uid = user.id.as_str();
    let arr = |v: &Value, key: &str| -> Vec<Value> {
        v.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let now = rfc3339::now_rfc3339();

    let db_lock = state.db.lock().expect("db mutex poisoned");

    for raw in arr(&payload, "books") {
        let Ok(p) = serde_json::from_value::<PushedBook>(raw) else {
            continue;
        };
        if p.hash.is_empty() {
            continue;
        }
        let db = book_to_db(&p, uid, &now);
        let data = serde_json::to_value(&db).map_err(ApiError::server)?;
        let row = store::BookRow {
            book_hash: db.book_hash.clone(),
            meta_hash: db.meta_hash.clone(),
            updated_at: p.updated_at,
            deleted_at: p.deleted_at,
            synced_at: now.clone(),
            data,
        };
        store::upsert_book(&db_lock, uid, &row)?;
    }

    for raw in arr(&payload, "notes") {
        let Ok(p) = serde_json::from_value::<PushedNote>(raw) else {
            continue;
        };
        if p.id.is_empty() {
            continue;
        }
        let db = note_to_db(&p, uid);
        let data = serde_json::to_value(&db).map_err(ApiError::server)?;
        store::upsert_note(&db_lock, uid, &data)?;
    }

    for raw in arr(&payload, "configs") {
        let Ok(p) = serde_json::from_value::<PushedConfig>(raw.clone()) else {
            continue;
        };
        if p.book_hash.is_empty() {
            continue;
        }
        let db = config_to_db(&p, uid);
        let data = serde_json::to_value(&db).map_err(ApiError::server)?;
        store::upsert_config(&db_lock, uid, &data)?;
        piggyback_book_progress(&db_lock, uid, &raw)?;
    }

    let stat_books: Vec<store::StatBookIn> = arr(&payload, "statBooks")
        .into_iter()
        .filter_map(|v| {
            let book_hash = v.get("book_hash").and_then(Value::as_str)?.to_string();
            Some(store::StatBookIn {
                book_hash,
                title: v
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                authors: v
                    .get("authors")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                updated_at_ms: effective_updated_at_ms(v.get("updated_at_ms")),
                deleted_at: v.get("deleted_at").and_then(Value::as_i64),
            })
        })
        .collect();
    if !stat_books.is_empty() {
        store::upsert_stat_books(&db_lock, uid, &stat_books)?;
    }

    let stat_pages: Vec<store::StatPageIn> = arr(&payload, "statPages")
        .into_iter()
        .filter_map(|v| {
            let book_hash = v.get("book_hash").and_then(Value::as_str)?.to_string();
            Some(store::StatPageIn {
                book_hash,
                page: v.get("page").and_then(Value::as_i64)?,
                start_time: v.get("start_time").and_then(Value::as_i64)?,
                duration: v.get("duration").and_then(Value::as_i64)?,
                total_pages: v.get("total_pages").and_then(Value::as_i64)?,
                updated_at_ms: effective_updated_at_ms(v.get("updated_at_ms")),
                deleted_at: v.get("deleted_at").and_then(Value::as_i64),
            })
        })
        .collect();
    if !stat_pages.is_empty() {
        store::upsert_stat_pages(&db_lock, uid, &stat_pages)?;
    }

    Ok(Json(json!({})).into_response())
}

/// Go decodes stat rows with pointer timestamps and defaults 0/missing to
/// "now" before storing.
fn effective_updated_at_ms(v: Option<&Value>) -> Option<i64> {
    match v.and_then(Value::as_i64) {
        Some(0) | None => Some(rfc3339::now_unix_ms()),
        other => other,
    }
}

/// Mirrors the official server's config→books progress piggyback: refresh the
/// book row's progress when the config's clock is newer, so other devices see
/// fresh library progress.
fn piggyback_book_progress(
    db_lock: &std::sync::MutexGuard<'_, rusqlite::Connection>,
    uid: &str,
    config_wire: &Value,
) -> Result<(), ApiError> {
    let book_hash = config_wire
        .get("bookHash")
        .and_then(Value::as_str)
        .unwrap_or("");
    let progress = config_wire.get("progress");
    let updated_at_ms = config_wire.get("updatedAt").and_then(Value::as_i64);
    let (Some(_progress_present), Some(updated_at_ms)) = (progress, updated_at_ms) else {
        return Ok(());
    };
    if book_hash.is_empty() {
        return Ok(());
    }

    let Some(existing) = store::get_book(db_lock, uid, book_hash)? else {
        return Ok(());
    };
    if existing.updated_at.is_some_and(|ts| ts >= updated_at_ms) {
        return Ok(());
    }
    let Some(mut book_obj) = existing.data.as_object().cloned() else {
        return Ok(());
    };
    book_obj.insert("progress".into(), progress.cloned().unwrap_or(Value::Null));
    if let Some(iso) = rfc3339::epoch_ms_to_rfc3339(updated_at_ms) {
        book_obj.insert("updated_at".into(), Value::String(iso));
    }
    let row = store::BookRow {
        book_hash: book_hash.into(),
        meta_hash: String::new(),
        updated_at: Some(updated_at_ms),
        deleted_at: existing.deleted_at,
        synced_at: rfc3339::now_rfc3339(),
        data: Value::Object(book_obj),
    };
    crate::store::upsert_book_raw(db_lock, uid, &row)
}

// ── replicas ────────────────────────────────────────────────────────────────

fn replica_wire(uid: &str, kind: &str, r: &store::ReplicaOut) -> Result<Value, ApiError> {
    let fields = RawValue::from_string(r.fields.clone()).map_err(ApiError::server)?;
    let manifest = match &r.manifest {
        Some(m) => Value::String(m.clone()),
        None => Value::Null,
    };
    Ok(json!({
        "user_id": uid,
        "kind": kind,
        "replica_id": r.replica_id,
        "fields_jsonb": fields,
        "manifest_jsonb": manifest,
        "deleted_at_ts": r.deleted_at_ts,
        "reincarnation": r.reincarnation,
        "updated_at_ts": r.updated_at_ts,
        "schema_version": r.schema_version,
    }))
}

/// GET /api/sync/replicas?kind=&since=<HLC>
pub async fn replicas_get(
    State(state): State<SharedState>,
    user: AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let uid = user.id.as_str();
    let kind = q.get("kind").map(String::as_str).unwrap_or("");
    let since = q.get("since").filter(|s| !s.is_empty()).map(String::as_str);
    let db_lock = state.db.lock().expect("db mutex poisoned");
    let rows = store::pull_replicas(&db_lock, uid, kind, since)?;
    let out: Result<Vec<Value>, ApiError> =
        rows.iter().map(|r| replica_wire(uid, kind, r)).collect();
    Ok(Json(json!({ "rows": out? })).into_response())
}

#[derive(serde::Deserialize, Clone)]
struct ReplicaCursorWire {
    kind: String,
    since: Option<String>,
}

fn parse_replica_in(v: &Value) -> Option<store::ReplicaIn> {
    Some(store::ReplicaIn {
        kind: v.get("kind")?.as_str()?.to_string(),
        replica_id: v.get("replica_id")?.as_str()?.to_string(),
        fields: match v.get("fields_jsonb") {
            Some(Value::Null) | None => "{}".to_string(),
            Some(other) => other.to_string(),
        },
        manifest: match v.get("manifest_jsonb") {
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        },
        deleted_at_ts: v
            .get("deleted_at_ts")
            .and_then(Value::as_str)
            .map(String::from),
        reincarnation: v
            .get("reincarnation")
            .and_then(Value::as_str)
            .map(String::from),
        updated_at_ts: v.get("updated_at_ts")?.as_str()?.to_string(),
        schema_version: v.get("schema_version").and_then(Value::as_i64).unwrap_or(0),
    })
}

/// POST /api/sync/replicas — `{"rows":[...]}` upserts then echoes (the Go
/// echo re-pulls with an empty kind filter; kept identical), or
/// `{"cursors":[{kind,since}]}` batch-pulls into `{"results":[...]}`.
pub async fn replicas_post(
    State(state): State<SharedState>,
    user: AuthUser,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let payload: Value = serde_json::from_slice(&body).map_err(|_| ApiError::Validation)?;
    let uid = user.id.as_str();
    let db_lock = state.db.lock().expect("db mutex poisoned");

    if let Some(rows) = payload.get("rows").and_then(Value::as_array) {
        if !rows.is_empty() {
            for row in rows {
                let Some(r) = parse_replica_in(row) else {
                    continue;
                };
                store::upsert_replica(&db_lock, uid, &r)?;
            }
            // Echo: same filter shape as the reference (kind="", since=None).
            let stored = store::pull_replicas(&db_lock, uid, "", None)?;
            let out: Result<Vec<Value>, ApiError> =
                stored.iter().map(|r| replica_wire(uid, "", r)).collect();
            return Ok(Json(json!({ "rows": out? })).into_response());
        }
    }

    if let Some(cursors) = payload.get("cursors").and_then(Value::as_array) {
        if !cursors.is_empty() {
            let parsed: Vec<ReplicaCursorWire> = cursors
                .iter()
                .filter_map(|c| serde_json::from_value(c.clone()).ok())
                .collect();
            let mut results = Vec::with_capacity(parsed.len());
            for c in parsed {
                let rows = store::pull_replicas(&db_lock, uid, &c.kind, c.since.as_deref())?;
                let out: Result<Vec<Value>, ApiError> =
                    rows.iter().map(|r| replica_wire(uid, &c.kind, r)).collect();
                results.push(json!({ "kind": c.kind, "rows": out? }));
            }
            return Ok(Json(json!({ "results": results })).into_response());
        }
    }

    Ok(Json(json!({ "rows": [] })).into_response())
}

// ── replica keys ────────────────────────────────────────────────────────────

fn key_wire(k: &store::ReplicaKeyOut) -> Value {
    // camelCase wire keys, mirroring the reference exactly.
    json!({
        "saltId": k.salt_id,
        "alg": k.alg,
        "salt": k.salt,
        "createdAt": k.created_at,
    })
}

pub async fn replica_keys_get(
    State(state): State<SharedState>,
    user: AuthUser,
) -> Result<Response, ApiError> {
    let db_lock = state.db.lock().expect("db mutex poisoned");
    let rows = store::list_replica_keys(&db_lock, &user.id)?;
    Ok(Json(json!({ "rows": rows.iter().map(key_wire).collect::<Vec<_>>() })).into_response())
}

pub async fn replica_keys_post(
    State(state): State<SharedState>,
    user: AuthUser,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let payload: Value = serde_json::from_slice(&body).map_err(|_| ApiError::Validation)?;
    let alg = payload.get("alg").and_then(Value::as_str).unwrap_or("");
    if alg.is_empty() {
        return Err(ApiError::Validation);
    }
    let salt_id = random_hex(8);
    let salt = random_hex(32);
    let uid = user.id.as_str();
    {
        let db_lock = state.db.lock().expect("db mutex poisoned");
        store::upsert_replica_key(&db_lock, uid, alg, &salt_id, &salt)?;
        let rows = store::list_replica_keys(&db_lock, uid)?;
        let created = rows
            .iter()
            .find(|k| k.salt_id == salt_id)
            .ok_or_else(|| ApiError::server(anyhow::anyhow!("created key not found")))?;
        Ok(Json(json!({ "row": key_wire(created) })).into_response())
    }
}

pub async fn replica_keys_delete(
    State(state): State<SharedState>,
    user: AuthUser,
) -> Result<Response, ApiError> {
    let db_lock = state.db.lock().expect("db mutex poisoned");
    store::delete_replica_keys(&db_lock, &user.id)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn random_hex(n_bytes: usize) -> String {
    use rand::Rng;
    let bytes: Vec<u8> = (0..n_bytes).map(|_| rand::rng().random()).collect();
    hex::encode(bytes)
}
