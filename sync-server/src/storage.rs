//! /api/storage/* — local-disk blob store plus the metadata endpoints, per
//! docs/selfhosted/sync-protocol.md §4. Keys are always `<uid>/<client path>`;
//! blob URLs are relative (`/api/storage/blob/<key>`) so the client's
//! `toFullUrl` prefixes its server base and attaches the Bearer header.

use std::path::{Path, PathBuf};

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db::SharedState;
use crate::error::ApiError;

// ── key/path safety ─────────────────────────────────────────────────────────

/// Normalize a storage key the way Go's `filepath.Clean("/"+key)` does:
/// fold `.`/`..` segments (never escaping the root), drop empties.
pub fn normalize_key(key: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for seg in key.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn key_to_path(root: &Path, key: &str) -> Option<PathBuf> {
    let normalized = normalize_key(key)?;
    let mut path = root.to_path_buf();
    for seg in normalized.split('/') {
        path.push(seg);
    }
    Some(path)
}

struct FileMeta {
    key: String,
    size: i64,
    updated_at: String,
}

fn walk_dir(dir: &Path, rel: &mut String, out: &mut Vec<FileMeta>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if rel.is_empty() {
            rel.push_str(name);
        } else {
            rel.push('/');
            rel.push_str(name);
        }
        if is_dir {
            walk_dir(&entry.path(), rel, out);
        } else if let Ok(meta) = entry.metadata() {
            let updated_at = meta
                .modified()
                .ok()
                .and_then(|m| {
                    let dt = time::OffsetDateTime::from(m);
                    dt.format(&time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_default();
            // Reference format is second-precision `2006-01-02T15:04:05Z`.
            let updated_at = updated_at.split('.').next().unwrap_or("").to_string();
            out.push(FileMeta {
                key: rel.clone(),
                size: meta.len() as i64,
                updated_at,
            });
        }
        // undo push (keep no trailing slash)
        match rel.rfind('/') {
            Some(i) => rel.truncate(i),
            None => rel.clear(),
        }
    }
}

fn list_prefix(state: &SharedState, uid: &str) -> Result<Vec<FileMeta>, ApiError> {
    let root = state.cfg.files_root();
    let base = root.join(uid);
    let mut out = Vec::new();
    if base.exists() {
        walk_dir(&base, &mut String::new(), &mut out);
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out
        .into_iter()
        .map(|mut f| {
            f.key = format!("{uid}/{}", f.key);
            f
        })
        .collect())
}

fn compute_usage(state: &SharedState, uid: &str) -> Result<i64, ApiError> {
    Ok(list_prefix(state, uid)?.iter().map(|f| f.size).sum())
}

fn storage_error_response(status: StatusCode, message: &str, code: &str) -> Response {
    (status, Json(json!({ "error": message, "code": code }))).into_response()
}

// ── endpoints ───────────────────────────────────────────────────────────────

/// POST /api/storage/upload — `{fileName, fileSize, bookHash?, replicaKind?,
/// replicaId?, temp?, media?}` → `{uploadUrl, downloadUrl?}`.
///
/// The stored key MUST be `uid + "/" + fileName` verbatim (the client sends
/// the full cloud path and later derives download keys the same way).
pub async fn upload(
    State(_state): State<SharedState>,
    user: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let payload: Value = serde_json::from_slice(&body).map_err(|_| ApiError::Validation)?;
    let file_name = payload
        .get("fileName")
        .and_then(Value::as_str)
        .ok_or(ApiError::Validation)?;
    let uid = user.id.as_str();
    let key = format!("{uid}/{file_name}");
    let blob_url = format!("/api/storage/blob/{key}");
    // LocalDisk URLs are always non-empty, so the reference always includes it.
    Ok(Json(json!({ "uploadUrl": blob_url, "downloadUrl": blob_url })).into_response())
}

fn blob_path(state: &SharedState, key: &str) -> Option<PathBuf> {
    key_to_path(&state.cfg.files_root(), key)
}

/// Store `body` content-addressed under `blobs/<sha256>` and hard-link the
/// caller's `path` to it, so identical book bytes occupy disk once server-wide
/// while every per-uid key still exists (quota/list walk unchanged: hardlinks
/// report full size). Fall back to a plain write where hardlinks don't work.
///
/// The blobs/ entry is written exactly once, write-if-absent via temp+rename:
/// `fs::write` on an existing hardlinked path would truncate the shared blob
/// and corrupt every other user's copy of the same book.
fn store_dedup(state: &SharedState, path: &Path, body: &[u8]) -> std::io::Result<()> {
    let digest = Sha256::digest(body);
    let blobs = state.cfg.blobs_root();
    std::fs::create_dir_all(&blobs)?;
    let blob = blobs.join(hex::encode(digest));
    if !blob.exists() {
        let tmp = blobs.join(format!(".incoming-{}", std::process::id()));
        std::fs::write(&tmp, body)?;
        // Rename may race a concurrent identical upload; either outcome leaves
        // a complete blob at `blob`, so rename errors are non-fatal here.
        match std::fs::rename(&tmp, &blob) {
            Ok(()) => {}
            Err(_) if blob.exists() => {
                let _ = std::fs::remove_file(&tmp);
            }
            Err(e) => return Err(e),
        }
    }
    match std::fs::hard_link(&blob, path) {
        Ok(()) => Ok(()),
        // Overwriting an existing key: swap the link to the (new) blob.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path)?;
            std::fs::hard_link(&blob, path)
        }
        // Cross-device or filesystems without hardlink support.
        Err(_) => std::fs::write(path, body),
    }
}

/// Hard-link count of a file (1 = no other links). Best-effort: unreadable
/// entries count as linked so GC never deletes what it cannot inspect.
fn link_count(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        match std::fs::symlink_metadata(path) {
            Ok(meta) => std::os::unix::fs::MetadataExt::nlink(&meta),
            Err(_) => 2,
        }
    }
    #[cfg(windows)]
    {
        // std's number_of_links() is unstable (windows_by_handle); ask
        // kernel32 directly — BY_HANDLE_FILE_INFORMATION.nNumberOfLinks.
        #[repr(C)]
        struct ByHandleInfo {
            attributes: u32,
            creation: [u32; 2],
            access: [u32; 2],
            write: [u32; 2],
            volume_serial: u32,
            size_high: u32,
            size_low: u32,
            links: u32,
            index_high: u32,
            index_low: u32,
        }
        extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                info: *mut ByHandleInfo,
            ) -> i32;
        }
        use std::os::windows::io::AsRawHandle;
        let Ok(file) = std::fs::File::open(path) else {
            return 2;
        };
        let mut info = ByHandleInfo {
            attributes: 0,
            creation: [0; 2],
            access: [0; 2],
            write: [0; 2],
            volume_serial: 0,
            size_high: 0,
            size_low: 0,
            links: 0,
            index_high: 0,
            index_low: 0,
        };
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        if ok == 0 {
            2
        } else {
            info.links as u64
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        2
    }
}

/// Delete content-addressed blobs whose per-uid hard links are all gone
/// (link count 1 = only the blobs/ entry remains). Runs at boot; also
/// exposed to the admin endpoint.
pub fn gc_blobs(cfg: &crate::config::Config) -> usize {
    let Ok(entries) = std::fs::read_dir(cfg.blobs_root()) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(true) {
            continue;
        }
        if link_count(&entry.path()) <= 1 && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// PUT /api/storage/blob/<key> — quota-checked before accepting the body.
pub async fn blob_put(
    State(state): State<SharedState>,
    user: AuthUser,
    axum::extract::Path(key): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = user.id.as_str();
    if !key.starts_with(&format!("{uid}/")) {
        return Ok(storage_error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            "AUTH",
        ));
    }
    if let Some(cl) = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
    {
        if cl > 0 {
            let used = compute_usage(&state, uid)?;
            if used + cl > state.cfg.quota_bytes {
                return Ok(storage_error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "storage quota exceeded",
                    "QUOTA_EXCEEDED",
                ));
            }
        }
    }
    let path =
        blob_path(&state, &key).ok_or_else(|| ApiError::server(anyhow::anyhow!("rejected key")))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    }
    store_dedup(&state, &path, &body).map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    #[cfg(feature = "library")]
    {
        if let Ok(mut conn) = state.db.lock() {
            // Catalog registration is best-effort: never fail the upload over it.
            let _ = crate::library::catalog::register_upload(&mut conn, uid, &key);
        }
    }
    Ok(StatusCode::OK.into_response())
}

/// GET /api/storage/blob/<key> — single-range support, mirroring the
/// reference's status codes (403 empty on scope violation, 404 empty when
/// missing, 416 with `bytes */<total>` when unsatisfiable).
pub async fn blob_get(
    State(state): State<SharedState>,
    user: AuthUser,
    axum::extract::Path(key): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    let uid = user.id.as_str();
    if !key.starts_with(&format!("{uid}/")) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let path = match key_to_path(&state.cfg.files_root(), &key) {
        Some(p) => p,
        None => return Ok(StatusCode::NOT_FOUND.into_response()),
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let total = meta.len() as i64;

    let range = headers
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok());
    let (offset, length, ranged) = parse_range(range);

    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(&path).map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let (offset, length) = if ranged {
        if offset >= total {
            let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            resp.headers_mut()
                .insert("Content-Range", format!("bytes */{total}").parse().unwrap());
            return Ok(resp);
        }
        let length = if length <= 0 || offset + length > total {
            total - offset
        } else {
            length
        };
        (offset, length)
    } else {
        (0i64, total)
    };

    file.seek(SeekFrom::Start(offset as u64))
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let mut buf = vec![0u8; length as usize];
    file.read_exact(&mut buf)
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;

    let mut resp = Response::builder()
        .header("Content-Type", "application/octet-stream")
        .header("Accept-Ranges", "bytes");
    if ranged {
        resp = resp
            .status(StatusCode::PARTIAL_CONTENT)
            .header(
                "Content-Range",
                format!("bytes {offset}-{}/{total}", offset + length - 1),
            )
            .header("Content-Length", length.to_string());
    } else {
        resp = resp
            .status(StatusCode::OK)
            .header("Content-Length", total.to_string());
    }
    Ok(resp.body(axum::body::Body::from(buf)).unwrap())
}

/// Single `bytes=start-end` ranges only; malformed/unsupported forms fall
/// back to serving the whole object (valid per RFC 9110 §14.2).
fn parse_range(header: Option<&str>) -> (i64, i64, bool) {
    let Some(h) = header.filter(|h| h.starts_with("bytes=")) else {
        return (0, 0, false);
    };
    let spec = &h["bytes=".len()..];
    if spec.contains(',') {
        return (0, 0, false);
    }
    let Some((start, end)) = spec.split_once('-') else {
        return (0, 0, false);
    };
    if start.is_empty() {
        return (0, 0, false); // suffix range: serve whole object
    }
    let Ok(start) = start.parse::<i64>() else {
        return (0, 0, false);
    };
    if start < 0 {
        return (0, 0, false);
    }
    if end.is_empty() {
        return (start, -1, true);
    }
    let Ok(end) = end.parse::<i64>() else {
        return (0, 0, false);
    };
    if end < start {
        return (0, 0, false);
    }
    (start, end - start + 1, true)
}

/// GET /api/storage/download?fileKey=
pub async fn download(Query(q): Query<HashMap<String, String>>) -> Result<Response, ApiError> {
    let Some(key) = q.get("fileKey").filter(|k| !k.is_empty()) else {
        return Ok(storage_error_response(
            StatusCode::BAD_REQUEST,
            "missing fileKey",
            "VALIDATION",
        ));
    };
    Ok(Json(json!({ "downloadUrl": format!("/api/storage/blob/{key}") })).into_response())
}

/// POST /api/storage/download — batch URL lookup.
pub async fn download_batch(body: Bytes) -> Result<Response, ApiError> {
    let payload: Value = serde_json::from_slice(&body).map_err(|_| ApiError::Validation)?;
    let mut out = serde_json::Map::new();
    if let Some(keys) = payload.get("fileKeys").and_then(Value::as_array) {
        for k in keys {
            if let Some(key) = k.as_str() {
                out.insert(key.to_string(), json!(format!("/api/storage/blob/{key}")));
            }
        }
    }
    Ok(Json(json!({ "downloadUrls": out })).into_response())
}

pub fn book_hash_from_key(key: &str) -> Option<String> {
    let parts: Vec<&str> = key.split('/').collect();
    for i in 0..parts.len() {
        if parts[i] == "Books" && i > 0 && parts[i - 1] == "Readest" && i + 1 < parts.len() {
            return Some(parts[i + 1].to_string());
        }
    }
    for i in 0..parts.len() {
        if parts[i] == "books" && i + 1 < parts.len() {
            return Some(parts[i + 1].to_string());
        }
    }
    None
}

fn replica_info_from_key(key: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = key.split('/').collect();
    for i in 0..parts.len() {
        if parts[i] == "Replicas" && i > 0 && parts[i - 1] == "Readest" && i + 2 < parts.len() {
            return (
                Some(parts[i + 1].to_string()),
                Some(parts[i + 2].to_string()),
            );
        }
    }
    for i in 0..parts.len() {
        if parts[i] == "replicas" && i + 2 < parts.len() {
            return (
                Some(parts[i + 1].to_string()),
                Some(parts[i + 2].to_string()),
            );
        }
    }
    (None, None)
}

/// GET /api/storage/list — pagination + bookHash/search filters + sorting.
pub async fn list(
    State(state): State<SharedState>,
    user: AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let uid = user.id.as_str();
    let page: i64 = q.get("page").and_then(|v| v.parse().ok()).unwrap_or(0);
    let page = if page < 1 { 1 } else { page };
    let page_size: i64 = q.get("pageSize").and_then(|v| v.parse().ok()).unwrap_or(0);
    let page_size = if page_size < 1 { 50 } else { page_size };
    let sort_by = q.get("sortBy").map(String::as_str).unwrap_or("key");
    let sort_order = q.get("sortOrder").map(String::as_str).unwrap_or("asc");
    let asc = sort_order != "desc";
    let book_hash = q.get("bookHash").cloned().unwrap_or_default();
    let search = q.get("search").cloned().unwrap_or_default();

    let mut files = list_prefix(&state, uid)?;
    if !book_hash.is_empty() {
        files.retain(|f| {
            f.key.contains(&format!("/Readest/Books/{book_hash}/"))
                || f.key.contains(&format!("/books/{book_hash}/"))
        });
    }
    if !search.is_empty() {
        files.retain(|f| f.key.contains(&search));
    }
    files.sort_by(|a, b| {
        let ord = match sort_by {
            "size" => b
                .size
                .cmp(&a.size)
                .reverse()
                .then_with(|| a.key.cmp(&b.key)),
            "updated_at" => a
                .updated_at
                .cmp(&b.updated_at)
                .then_with(|| a.key.cmp(&b.key)),
            _ => a.key.cmp(&b.key),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });

    let total = files.len() as i64;
    let total_pages = ((total + page_size - 1) / page_size).max(1);
    let start = ((page - 1) * page_size).min(total);
    let end = (start + page_size).min(total);

    let page_items: Vec<Value> = files[start as usize..end as usize]
        .iter()
        .map(|f| {
            let bh = book_hash_from_key(&f.key);
            let (rk, rid) = replica_info_from_key(&f.key);
            json!({
                "file_key": f.key,
                "file_size": f.size,
                "book_hash": bh,
                "replica_kind": rk,
                "replica_id": rid,
                "created_at": f.updated_at,
                "updated_at": f.updated_at,
            })
        })
        .collect();

    Ok(Json(json!({
        "files": page_items,
        "total": total,
        "page": page,
        "pageSize": page_size,
        "totalPages": total_pages,
    }))
    .into_response())
}

/// GET /api/storage/stats
pub async fn stats(State(state): State<SharedState>, user: AuthUser) -> Result<Response, ApiError> {
    let uid = user.id.as_str();
    let files = list_prefix(&state, uid)?;
    let usage: i64 = files.iter().map(|f| f.size).sum();

    // byBookHash: files without a recognizable book hash share the "" key.
    let mut by_book: HashMap<String, (i64, i64)> = HashMap::new();
    for f in &files {
        let key = book_hash_from_key(&f.key).unwrap_or_default();
        let e = by_book.entry(key).or_insert((0, 0));
        e.0 += 1;
        e.1 += f.size;
    }
    let mut by_book_arr: Vec<Value> = by_book
        .into_iter()
        .map(|(bh, (count, size))| json!({ "bookHash": bh, "fileCount": count, "totalSize": size }))
        .collect();
    by_book_arr.sort_by(|a, b| {
        a["bookHash"]
            .as_str()
            .unwrap_or("")
            .cmp(b["bookHash"].as_str().unwrap_or(""))
    });

    let quota = state.cfg.quota_bytes;
    let pct = usage as f64 / quota as f64 * 100.0;
    Ok(Json(json!({
        "totalFiles": files.len(),
        "totalSize": usage,
        "usage": usage,
        "quota": quota,
        "usagePercentage": pct,
        "byBookHash": by_book_arr,
    }))
    .into_response())
}

/// DELETE /api/storage/delete?fileKey= — scoped to the caller's key space.
pub async fn delete(
    State(state): State<SharedState>,
    user: AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let uid = user.id.as_str();
    let Some(key) = q.get("fileKey").filter(|k| !k.is_empty()) else {
        return Ok(storage_error_response(
            StatusCode::BAD_REQUEST,
            "missing fileKey",
            "VALIDATION",
        ));
    };
    if !key.starts_with(&format!("{uid}/")) {
        return Ok(storage_error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            "AUTH",
        ));
    }
    let path = key_to_path(&state.cfg.files_root(), key)
        .ok_or_else(|| ApiError::server(anyhow::anyhow!("invalid key")))?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(ApiError::server(anyhow::anyhow!(e))),
    }
    Ok(StatusCode::OK.into_response())
}

/// DELETE /api/storage/purge — `{"fileKeys":[...]}`; per-key result list.
pub async fn purge(
    State(state): State<SharedState>,
    user: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let uid = user.id.as_str();
    let payload: Value = serde_json::from_slice(&body).map_err(|_| ApiError::Validation)?;
    let mut success = Vec::new();
    let mut failed = Vec::new();
    if let Some(keys) = payload.get("fileKeys").and_then(Value::as_array) {
        for k in keys {
            let Some(key) = k.as_str() else { continue };
            if !key.starts_with(&format!("{uid}/")) {
                failed.push(json!({ "fileKey": key, "error": "forbidden" }));
                continue;
            }
            let path = key_to_path(&state.cfg.files_root(), key);
            let removed = match path {
                Some(p) => match std::fs::remove_file(&p) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e.to_string()),
                },
                None => Err("invalid key".into()),
            };
            match removed {
                Ok(()) => success.push(json!(key)),
                Err(err) => failed.push(json!({ "fileKey": key, "error": err })),
            }
        }
    }
    Ok(Json(json!({
        "success": success,
        "failed": failed,
        "deletedCount": success.len(),
        "failedCount": failed.len(),
    }))
    .into_response())
}
