//! /api/archive/* — per-user portable archives (ROADMAP 待办⑫): one tar
//! packages the caller's book files (content-addressed blobs) plus every
//! per-user record table, importable into any account on this or another
//! server. Layout and contract: docs/selfhosted/sync-protocol.md §6.8.
//!
//! Export streams `manifest.json`, `records.json`, and — for
//! `scope=full` — `blobs/<sha256>`. Import re-keys every row to the caller's
//! uid and feeds it through the regular `store::upsert_*` LWW paths, so an
//! import never clobbers newer data and re-importing is idempotent. A
//! metadata archive keeps the records but skips missing blobs: entries
//! become visible while zero new bytes land in the blob store.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::Path;

use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::auth::AuthUser;
use crate::db::SharedState;
use crate::error::ApiError;
use crate::store;
use crate::tarstream::ChannelWriter;

const ARCHIVE_FORMAT: &str = "readest-archive";
const ARCHIVE_VERSION: i64 = 1;

fn archive_error(status: StatusCode, message: &str, code: &str) -> Response {
    (status, Json(json!({ "error": message, "code": code }))).into_response()
}

fn db_poisoned() -> Response {
    archive_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db lock poisoned",
        "SERVER",
    )
}

/// Resets `AppState::archive_busy` when the archive task ends — success,
/// failure, or client disconnect alike.
struct ResetArchiveBusy(SharedState);

impl Drop for ResetArchiveBusy {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.0.archive_busy.store(false, Ordering::Release);
    }
}

/// `blobs/` entry names are lowercase hex sha256 by construction; anything
/// else in an archive's blobs/ section is skipped rather than trusted.
fn is_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ── records snapshot ────────────────────────────────────────────────────────

/// Raw rows of every per-user table (the same set the admin user-delete
/// cascade clears), snake_case mirroring the columns. notes/configs export
/// just their `data` JSON — the upsert paths derive the key columns from it.
fn collect_records(conn: &rusqlite::Connection, uid: &str) -> Result<Value, ApiError> {
    let row_err = |e: rusqlite::Error| ApiError::server(e);

    let mut books = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT book_hash, meta_hash, updated_at, deleted_at, synced_at, data
                 FROM books WHERE user_id=? ORDER BY book_hash",
            )
            .map_err(row_err)?;
        let rows = stmt
            .query_map(params![uid], |r| {
                Ok(json!({
                    "book_hash": r.get::<_, String>(0)?,
                    "meta_hash": r.get::<_, String>(1)?,
                    "updated_at": r.get::<_, Option<i64>>(2)?,
                    "deleted_at": r.get::<_, Option<i64>>(3)?,
                    "synced_at": r.get::<_, String>(4)?,
                    "data": serde_json::from_str::<Value>(&r.get::<_, String>(5)?)
                        .unwrap_or(Value::Null),
                }))
            })
            .map_err(row_err)?;
        for row in rows {
            books.push(row.map_err(row_err)?);
        }
    }

    let json_column = |sql: &str| -> Result<Vec<Value>, ApiError> {
        let mut stmt = conn.prepare(sql).map_err(row_err)?;
        let rows = stmt
            .query_map(params![uid], |r| {
                Ok(serde_json::from_str::<Value>(&r.get::<_, String>(0)?).unwrap_or(Value::Null))
            })
            .map_err(row_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(row_err)?);
        }
        Ok(out)
    };
    let notes = json_column("SELECT data FROM notes WHERE user_id=? ORDER BY note_id")?;
    let configs = json_column("SELECT data FROM configs WHERE user_id=? ORDER BY config_id")?;

    let mut stat_books = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT book_hash, title, authors, updated_at_ms, deleted_at
                 FROM stat_books WHERE user_id=? ORDER BY book_hash",
            )
            .map_err(row_err)?;
        let rows = stmt
            .query_map(params![uid], |r| {
                Ok(json!({
                    "book_hash": r.get::<_, String>(0)?,
                    "title": r.get::<_, String>(1)?,
                    "authors": r.get::<_, String>(2)?,
                    "updated_at_ms": r.get::<_, Option<i64>>(3)?,
                    "deleted_at": r.get::<_, Option<i64>>(4)?,
                }))
            })
            .map_err(row_err)?;
        for row in rows {
            stat_books.push(row.map_err(row_err)?);
        }
    }

    let mut stat_pages = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT book_hash, page, start_time, duration, total_pages, updated_at_ms, deleted_at
                 FROM stat_pages WHERE user_id=? ORDER BY book_hash, page, start_time",
            )
            .map_err(row_err)?;
        let rows = stmt
            .query_map(params![uid], |r| {
                Ok(json!({
                    "book_hash": r.get::<_, String>(0)?,
                    "page": r.get::<_, i64>(1)?,
                    "start_time": r.get::<_, i64>(2)?,
                    "duration": r.get::<_, i64>(3)?,
                    "total_pages": r.get::<_, i64>(4)?,
                    "updated_at_ms": r.get::<_, Option<i64>>(5)?,
                    "deleted_at": r.get::<_, Option<i64>>(6)?,
                }))
            })
            .map_err(row_err)?;
        for row in rows {
            stat_pages.push(row.map_err(row_err)?);
        }
    }

    let mut replicas = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT kind, replica_id, fields_jsonb, manifest_jsonb, deleted_at_ts, reincarnation, updated_at_ts, schema_version
                 FROM replicas WHERE user_id=? ORDER BY kind, replica_id",
            )
            .map_err(row_err)?;
        let rows = stmt
            .query_map(params![uid], |r| {
                Ok(json!({
                    "kind": r.get::<_, String>(0)?,
                    "replica_id": r.get::<_, String>(1)?,
                    // Raw JSON strings exactly as stored — parsing and
                    // re-serializing would risk key-order churn for no gain.
                    "fields": r.get::<_, String>(2)?,
                    "manifest": r.get::<_, Option<String>>(3)?,
                    "deleted_at_ts": r.get::<_, Option<String>>(4)?,
                    "reincarnation": r.get::<_, Option<String>>(5)?,
                    "updated_at_ts": r.get::<_, String>(6)?,
                    "schema_version": r.get::<_, i64>(7)?,
                }))
            })
            .map_err(row_err)?;
        for row in rows {
            replicas.push(row.map_err(row_err)?);
        }
    }

    let mut replica_keys = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT salt_id, alg, salt, created_at
                 FROM replica_keys WHERE user_id=? ORDER BY salt_id",
            )
            .map_err(row_err)?;
        let rows = stmt
            .query_map(params![uid], |r| {
                Ok(json!({
                    "salt_id": r.get::<_, String>(0)?,
                    "alg": r.get::<_, String>(1)?,
                    "salt": r.get::<_, String>(2)?,
                    "created_at": r.get::<_, String>(3)?,
                }))
            })
            .map_err(row_err)?;
        for row in rows {
            replica_keys.push(row.map_err(row_err)?);
        }
    }

    Ok(json!({
        "books": books, "notes": notes, "configs": configs,
        "statBooks": stat_books, "statPages": stat_pages,
        "replicas": replicas, "replicaKeys": replica_keys,
    }))
}

// ── export ──────────────────────────────────────────────────────────────────

/// GET /api/archive/export?scope=full|metadata — one tar, streamed. A second
/// concurrent archive operation is refused with 409: exports and imports
/// share the flag because both walk the blob store.
pub async fn export(
    State(state): State<SharedState>,
    user: AuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, Response> {
    use std::sync::atomic::Ordering;
    let scope = match q.get("scope").map(String::as_str) {
        None | Some("full") => "full",
        Some("metadata") => "metadata",
        Some(_) => {
            return Err(archive_error(
                StatusCode::BAD_REQUEST,
                "invalid scope",
                "VALIDATION",
            ))
        }
    };
    if state
        .archive_busy
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Err(archive_error(
            StatusCode::CONFLICT,
            "archive operation already in progress",
            "SERVER",
        ));
    }

    let uid = user.id.clone();
    let records = {
        let conn = state.db.lock().map_err(|_| db_poisoned())?;
        collect_records(&conn, &uid).map_err(|e| {
            archive_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string(), "SERVER")
        })?
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(8);
    let err_tx = tx.clone();
    let task_state = state.clone();
    let scope_owned = scope.to_string();
    tokio::task::spawn_blocking(move || {
        let _reset = ResetArchiveBusy(task_state.clone());
        let outcome = write_archive_tar(&task_state, &uid, &scope_owned, records, tx);
        // Surface a mid-stream failure in-band so the client's tar read
        // errors out instead of silently accepting a truncated archive.
        if let Err(e) = outcome {
            let _ = err_tx.blocking_send(Err(e));
        }
    });

    let stamp = export_stamp();
    let name_base = sanitize_filename(
        user.email
            .as_deref()
            .filter(|e| !e.is_empty())
            .unwrap_or(&user.id),
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-tar")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"readest-archive-{name_base}-{stamp}.tar\""),
        )
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .map_err(|_| {
            archive_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "response build failed",
                "SERVER",
            )
        })
}

fn export_stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn sanitize_filename(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "user".into()
    } else {
        out
    }
}

struct ArchiveFile {
    hash: String,
    /// Path under the user's files/ tree (no uid prefix).
    path: String,
    bytes: u64,
}

/// Walk the caller's files/ tree resolving every entry to its blob name.
/// The blob store is keyed by sha256 of the content — the client book hash
/// in `Books/<hash>/` paths is a different digest (partialMD5), so every
/// file is hashed on export; that doubles as an integrity check against the
/// blobs/ entry drifting from its name.
fn collect_files(state: &SharedState, uid: &str) -> Result<Vec<ArchiveFile>, ApiError> {
    let files_root = state.cfg.files_root();
    let prefix = format!("{uid}/");
    let mut out = Vec::new();
    for f in crate::storage::list_prefix(state, uid)? {
        let path = crate::storage::key_to_path(&files_root, &f.key)
            .ok_or_else(|| ApiError::server(anyhow::anyhow!("rejected key {}", f.key)))?;
        out.push(ArchiveFile {
            hash: hash_file(&path)?,
            path: f.key.strip_prefix(&prefix).unwrap_or(&f.key).to_string(),
            bytes: f.size as u64,
        });
    }
    Ok(out)
}

fn hash_file(path: &Path) -> Result<String, ApiError> {
    let mut file = std::fs::File::open(path).map_err(ApiError::server)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(ApiError::server)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn write_archive_tar(
    state: &SharedState,
    uid: &str,
    scope: &str,
    records: Value,
    tx: tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>,
) -> std::io::Result<()> {
    let files = collect_files(state, uid).map_err(std::io::Error::other)?;
    let blobs_root = state.cfg.blobs_root();
    let manifest = build_manifest(scope, &files, &records);
    let mut writer = ChannelWriter::new(tx);
    // Inner scope: tar::Builder holds `writer` until its Drop runs.
    {
        let mut builder = tar::Builder::new(&mut writer);
        append_json(&mut builder, "manifest.json", &manifest)?;
        append_json(&mut builder, "records.json", &records)?;
        if scope == "full" {
            // One entry per distinct hash — several per-uid paths may share
            // a blob. Missing blobs/ entries are skipped: records still
            // export, and the import side tolerates the gap.
            let mut seen: BTreeSet<String> = BTreeSet::new();
            for f in &files {
                if !seen.insert(f.hash.clone()) {
                    continue;
                }
                let blob = blobs_root.join(&f.hash);
                if blob.exists() {
                    builder.append_path_with_name(&blob, format!("blobs/{}", f.hash))?;
                }
            }
        }
        builder.finish()?;
    }
    writer.flush()
}

fn append_json<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    name: &str,
    v: &Value,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(v).map_err(std::io::Error::other)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, name, body.as_slice())
}

fn build_manifest(scope: &str, files: &[ArchiveFile], records: &Value) -> Value {
    let count = |k: &str| {
        records
            .get(k)
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0)
    };
    json!({
        "format": ARCHIVE_FORMAT,
        "version": ARCHIVE_VERSION,
        "scope": scope,
        "exportedAt": crate::rfc3339::now_rfc3339(),
        "serverVersion": env!("CARGO_PKG_VERSION"),
        "counts": {
            "books": count("books"), "notes": count("notes"), "configs": count("configs"),
            "statBooks": count("statBooks"), "statPages": count("statPages"),
            "replicas": count("replicas"), "replicaKeys": count("replicaKeys"),
            "files": files.len(),
        },
        "files": files
            .iter()
            .map(|f| json!({ "hash": f.hash, "path": f.path, "bytes": f.bytes }))
            .collect::<Vec<_>>(),
    })
}

// ── import ──────────────────────────────────────────────────────────────────

/// POST /api/archive/import — body is a raw readest-archive tar. Bounded by
/// the router-wide body limit (same ceiling as book uploads); larger
/// libraries should export/import as `scope=metadata` and let the file bytes
/// come back through normal storage sync.
pub async fn import(
    State(state): State<SharedState>,
    user: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    use std::sync::atomic::Ordering;
    let uid = user.id.clone();
    // Quota pre-check on the tar size, mirroring blob_put's Content-Length
    // gate. Deliberately over-strict (the tar also carries records and
    // already-stored blobs): cheap, and self-host quotas are roomy.
    if let Some(cl) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
    {
        if cl > 0 {
            let used = crate::storage::compute_usage(&state, &uid).map_err(|e| {
                archive_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string(), "SERVER")
            })?;
            if used + cl > state.cfg.quota_bytes {
                return Err(archive_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "storage quota exceeded",
                    "QUOTA_EXCEEDED",
                ));
            }
        }
    }
    if state
        .archive_busy
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Err(archive_error(
            StatusCode::CONFLICT,
            "archive operation already in progress",
            "SERVER",
        ));
    }

    let task_state = state.clone();
    let summary = tokio::task::spawn_blocking(move || {
        let _reset = ResetArchiveBusy(task_state.clone());
        run_import(&task_state, &uid, &body)
    })
    .await
    .map_err(|e| ImportError::Server(e.to_string()).into_response())?
    .map_err(ImportError::into_response)?;
    Ok(Json(summary).into_response())
}

#[derive(Default)]
struct ImportSummary {
    books: usize,
    notes: usize,
    configs: usize,
    stat_books: usize,
    stat_pages: usize,
    replicas: usize,
    replica_keys: usize,
    blobs_stored: usize,
    blobs_deduped: usize,
    blobs_skipped: usize,
    files_linked: usize,
    files_skipped: usize,
}

/// Small error enum — the Result payload stays lean (clippy's
/// `result_large_err` fires on a full `Response` in every Err position).
enum ImportError {
    Validation(String),
    Server(String),
}

impl ImportError {
    fn into_response(self) -> Response {
        match self {
            ImportError::Validation(m) => archive_error(StatusCode::BAD_REQUEST, &m, "VALIDATION"),
            ImportError::Server(m) => {
                archive_error(StatusCode::INTERNAL_SERVER_ERROR, &m, "SERVER")
            }
        }
    }
}

/// Runs the whole import; returns the summary JSON on success.
fn run_import(state: &SharedState, uid: &str, body: &[u8]) -> Result<Value, ImportError> {
    // Pass 1: read the two JSON members only; a bad manifest must reject the
    // archive before anything touches the blob store.
    let (manifest, records) = read_tar_metadata(body).map_err(ImportError::Validation)?;
    let format_ok = manifest.get("format").and_then(Value::as_str) == Some(ARCHIVE_FORMAT)
        && manifest.get("version").and_then(Value::as_i64) == Some(ARCHIVE_VERSION);
    if !format_ok {
        return Err(ImportError::Validation("unsupported archive format".into()));
    }

    let blobs_root = state.cfg.blobs_root();
    std::fs::create_dir_all(&blobs_root).map_err(|e| ImportError::Server(e.to_string()))?;

    let mut summary = ImportSummary::default();

    // Pass 2: blobs. Write-if-absent through a temp file, hashing while
    // streaming: an entry only enters the store when its content matches its
    // name, so a corrupted or renamed blob never lands.
    {
        let mut archive = tar::Archive::new(std::io::Cursor::new(body));
        let entries = archive
            .entries()
            .map_err(|e| ImportError::Validation(format!("invalid tar: {e}")))?;
        for entry in entries {
            let Ok(mut entry) = entry else { continue };
            let path = entry
                .path()
                .map_err(|e| ImportError::Validation(format!("invalid tar entry: {e}")))?
                .to_string_lossy()
                .into_owned();
            let Some(name) = path.strip_prefix("blobs/").map(String::from) else {
                continue;
            };
            if !is_hex_sha256(&name) {
                summary.blobs_skipped += 1;
                continue;
            }
            let blob = blobs_root.join(&name);
            let tmp = blobs_root.join(format!(".import-{}", std::process::id()));
            match stream_blob(&mut entry, &tmp, &name) {
                Ok(()) => {
                    if blob.exists() {
                        let _ = std::fs::remove_file(&tmp);
                        summary.blobs_deduped += 1;
                    } else {
                        match std::fs::rename(&tmp, &blob) {
                            Ok(()) => summary.blobs_stored += 1,
                            // A concurrent writer may have stored the same
                            // blob between the check and the rename.
                            Err(_) if blob.exists() => {
                                let _ = std::fs::remove_file(&tmp);
                                summary.blobs_deduped += 1;
                            }
                            Err(_) => {
                                let _ = std::fs::remove_file(&tmp);
                                summary.blobs_skipped += 1;
                            }
                        }
                    }
                }
                Err(_) => {
                    let _ = std::fs::remove_file(&tmp);
                    summary.blobs_skipped += 1;
                }
            }
        }
    }

    // Files: hard-link each manifest entry under the caller's uid. Existing
    // destinations are left alone (their record may be newer); a missing blob
    // (metadata archive) just means the link is skipped.
    let files_root = state.cfg.files_root();
    let mut linked_book_keys: Vec<String> = Vec::new();
    if let Some(list) = manifest.get("files").and_then(Value::as_array) {
        for f in list {
            let (Some(hash), Some(path)) = (
                f.get("hash").and_then(Value::as_str),
                f.get("path").and_then(Value::as_str),
            ) else {
                summary.files_skipped += 1;
                continue;
            };
            if !is_hex_sha256(hash) {
                summary.files_skipped += 1;
                continue;
            }
            let Some(norm) = crate::storage::normalize_key(path) else {
                summary.files_skipped += 1;
                continue;
            };
            let key = format!("{uid}/{norm}");
            let blob = blobs_root.join(hash);
            if !blob.exists() {
                summary.files_skipped += 1;
                continue;
            }
            let Some(dest) = crate::storage::key_to_path(&files_root, &key) else {
                summary.files_skipped += 1;
                continue;
            };
            if dest.exists() {
                summary.files_skipped += 1;
                continue;
            }
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::hard_link(&blob, &dest).is_ok() {
                summary.files_linked += 1;
                linked_book_keys.push(key);
            } else {
                summary.files_skipped += 1;
            }
        }
    }

    // Records: re-keyed to the caller, through the regular LWW upserts — an
    // imported row only lands when it is at least as new as what is stored.
    {
        let mut conn = state
            .db
            .lock()
            .map_err(|_| ImportError::Server("db lock poisoned".into()))?;
        let mut imported_hashes: Vec<String> = Vec::new();
        if let Some(rows) = records.get("books").and_then(Value::as_array) {
            for v in rows {
                let Some(row) = book_row_from_value(v) else {
                    continue;
                };
                store::upsert_book(&conn, uid, &row)
                    .map_err(|e| ImportError::Server(e.to_string()))?;
                imported_hashes.push(row.book_hash);
                summary.books += 1;
            }
        }
        // Re-stamp the server receive clock so clients whose pull cursor
        // already moved past the original synced_at still see the rows.
        // synced_at plays no role in LWW, so this is visibility-only.
        let now = crate::rfc3339::now_rfc3339();
        for hash in &imported_hashes {
            let _ = conn.execute(
                "UPDATE books SET synced_at=?1 WHERE user_id=?2 AND book_hash=?3",
                params![now, uid, hash],
            );
        }
        if let Some(rows) = records.get("notes").and_then(Value::as_array) {
            for v in rows {
                if store::upsert_note(&conn, uid, v).is_ok() {
                    summary.notes += 1;
                }
            }
        }
        if let Some(rows) = records.get("configs").and_then(Value::as_array) {
            for v in rows {
                if store::upsert_config(&conn, uid, v).is_ok() {
                    summary.configs += 1;
                }
            }
        }
        if let Some(rows) = records.get("statBooks").and_then(Value::as_array) {
            let ins: Vec<store::StatBookIn> =
                rows.iter().filter_map(stat_book_from_value).collect();
            if !ins.is_empty() {
                store::upsert_stat_books(&conn, uid, &ins)
                    .map_err(|e| ImportError::Server(e.to_string()))?;
                summary.stat_books = ins.len();
            }
        }
        if let Some(rows) = records.get("statPages").and_then(Value::as_array) {
            let ins: Vec<store::StatPageIn> =
                rows.iter().filter_map(stat_page_from_value).collect();
            if !ins.is_empty() {
                store::upsert_stat_pages(&conn, uid, &ins)
                    .map_err(|e| ImportError::Server(e.to_string()))?;
                summary.stat_pages = ins.len();
            }
        }
        if let Some(rows) = records.get("replicas").and_then(Value::as_array) {
            for v in rows {
                let Some(r) = replica_from_value(v) else {
                    continue;
                };
                if store::upsert_replica(&conn, uid, &r).is_ok() {
                    summary.replicas += 1;
                }
            }
        }
        if let Some(rows) = records.get("replicaKeys").and_then(Value::as_array) {
            for v in rows {
                let (Some(alg), Some(salt_id), Some(salt)) = (
                    v.get("alg").and_then(Value::as_str),
                    v.get("salt_id").and_then(Value::as_str),
                    v.get("salt").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if store::upsert_replica_key(&conn, uid, alg, salt_id, salt).is_ok() {
                    summary.replica_keys += 1;
                }
            }
        }
        #[cfg(feature = "library")]
        {
            // Catalog registration is best-effort: never fail the import.
            for key in &linked_book_keys {
                let _ = crate::library::catalog::register_upload(&mut conn, uid, key);
            }
        }
    }

    Ok(json!({
        "books": summary.books, "notes": summary.notes, "configs": summary.configs,
        "statBooks": summary.stat_books, "statPages": summary.stat_pages,
        "replicas": summary.replicas, "replicaKeys": summary.replica_keys,
        "blobsStored": summary.blobs_stored, "blobsDeduped": summary.blobs_deduped,
        "blobsSkipped": summary.blobs_skipped,
        "filesLinked": summary.files_linked, "filesSkipped": summary.files_skipped,
    }))
}

/// First tar pass: pull out the two JSON members without touching blobs.
fn read_tar_metadata(body: &[u8]) -> Result<(Value, Value), String> {
    let mut manifest: Option<Value> = None;
    let mut records: Option<Value> = None;
    let mut archive = tar::Archive::new(std::io::Cursor::new(body));
    let entries = archive.entries().map_err(|e| format!("invalid tar: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("invalid tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("invalid tar entry: {e}"))?
            .to_string_lossy()
            .into_owned();
        let slot = match path.as_str() {
            "manifest.json" => &mut manifest,
            "records.json" => &mut records,
            _ => continue,
        };
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .map_err(|e| format!("cannot read {path}: {e}"))?;
        *slot = Some(serde_json::from_str(&text).map_err(|e| format!("invalid {path}: {e}"))?);
    }
    match (manifest, records) {
        (Some(m), Some(r)) => Ok((m, r)),
        (None, _) => Err("missing manifest.json".into()),
        (_, None) => Err("missing records.json".into()),
    }
}

/// Stream one tar entry into `tmp`, verifying its sha256 matches `expected`
/// along the way. On mismatch the temp file is the caller's to drop.
fn stream_blob<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    tmp: &Path,
    expected_hex: &str,
) -> std::io::Result<()> {
    let mut file = std::fs::File::create(tmp)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = entry.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
    }
    file.flush()?;
    if hex::encode(hasher.finalize()) != expected_hex {
        return Err(std::io::Error::other("content hash mismatch"));
    }
    Ok(())
}

fn book_row_from_value(v: &Value) -> Option<store::BookRow> {
    Some(store::BookRow {
        book_hash: v.get("book_hash")?.as_str()?.to_string(),
        meta_hash: v
            .get("meta_hash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        updated_at: v.get("updated_at").and_then(Value::as_i64),
        deleted_at: v.get("deleted_at").and_then(Value::as_i64),
        synced_at: v
            .get("synced_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        data: v.get("data").cloned().unwrap_or(Value::Null),
    })
}

fn stat_book_from_value(v: &Value) -> Option<store::StatBookIn> {
    Some(store::StatBookIn {
        book_hash: v.get("book_hash")?.as_str()?.to_string(),
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        authors: v
            .get("authors")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        updated_at_ms: v.get("updated_at_ms").and_then(Value::as_i64),
        deleted_at: v.get("deleted_at").and_then(Value::as_i64),
    })
}

fn stat_page_from_value(v: &Value) -> Option<store::StatPageIn> {
    Some(store::StatPageIn {
        book_hash: v.get("book_hash")?.as_str()?.to_string(),
        page: v.get("page").and_then(Value::as_i64).unwrap_or(0),
        start_time: v.get("start_time").and_then(Value::as_i64).unwrap_or(0),
        duration: v.get("duration").and_then(Value::as_i64).unwrap_or(0),
        total_pages: v.get("total_pages").and_then(Value::as_i64).unwrap_or(0),
        updated_at_ms: v.get("updated_at_ms").and_then(Value::as_i64),
        deleted_at: v.get("deleted_at").and_then(Value::as_i64),
    })
}

fn replica_from_value(v: &Value) -> Option<store::ReplicaIn> {
    Some(store::ReplicaIn {
        kind: v.get("kind")?.as_str()?.to_string(),
        replica_id: v.get("replica_id")?.as_str()?.to_string(),
        fields: v
            .get("fields")
            .and_then(Value::as_str)
            .unwrap_or("{}")
            .to_string(),
        manifest: v
            .get("manifest")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        deleted_at_ts: v
            .get("deleted_at_ts")
            .and_then(Value::as_str)
            .map(String::from),
        reincarnation: v
            .get("reincarnation")
            .and_then(Value::as_str)
            .map(String::from),
        updated_at_ts: v
            .get("updated_at_ts")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        schema_version: v.get("schema_version").and_then(Value::as_i64).unwrap_or(0),
    })
}
