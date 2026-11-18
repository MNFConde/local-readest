//! /api/pushes/* — same-server user-to-user book push (ROADMAP 待办⑬):
//! one account offers a book (file + metadata snapshot, never notes or
//! progress) to another; the recipient sees it in a pending inbox and
//! accepts or declines. Contract: docs/selfhosted/sync-protocol.md §6.9.
//!
//! The push row snapshots the sender's `books` row at creation time, so what
//! was offered stays deterministic even if the sender edits metadata later.
//! Acceptance re-keys the snapshot to the recipient and walks it through the
//! regular `store::upsert_book` LWW path (an accept never clobbers newer
//! data), then hard-links the sender's stored files into the recipient's
//! `files/<uid>/` tree directly — file→file links keep the same blob alive,
//! so a fresh copy costs zero new bytes in the blob store and survives the
//! sender later deleting their own copy.

use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::params;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db::SharedState;
use crate::store;

/// Optional note attached to a push; bounded so the inbox stays a list of
/// one-liners, not a message board.
const MAX_MESSAGE_CHARS: usize = 500;

fn push_error(status: StatusCode, message: &str, code: &str) -> Response {
    (status, Json(json!({ "error": message, "code": code }))).into_response()
}

/// Small error enum — the Result payload stays lean (clippy's
/// `result_large_err` fires on a full `Response` in every Err position).
enum PushError {
    Validation(String),
    NotFound(String),
    Conflict(String),
    Quota(String),
    Server(String),
}

impl PushError {
    fn into_response(self) -> Response {
        match self {
            PushError::Validation(m) => push_error(StatusCode::BAD_REQUEST, &m, "VALIDATION"),
            PushError::NotFound(m) => push_error(StatusCode::NOT_FOUND, &m, "NOT_FOUND"),
            PushError::Conflict(m) => push_error(StatusCode::CONFLICT, &m, "CONFLICT"),
            PushError::Quota(m) => push_error(StatusCode::PAYLOAD_TOO_LARGE, &m, "QUOTA_EXCEEDED"),
            PushError::Server(m) => push_error(StatusCode::INTERNAL_SERVER_ERROR, &m, "SERVER"),
        }
    }
}

struct PushRow {
    from_uid: String,
    to_uid: String,
    book_hash: String,
    payload_json: String,
    status: String,
}

fn load_push(conn: &rusqlite::Connection, id: i64) -> Result<Option<PushRow>, PushError> {
    conn.query_row(
        "SELECT from_uid, to_uid, book_hash, payload_json, status
         FROM pushes WHERE id = ?1",
        params![id],
        |r| {
            Ok(PushRow {
                from_uid: r.get(0)?,
                to_uid: r.get(1)?,
                book_hash: r.get(2)?,
                payload_json: r.get(3)?,
                status: r.get(4)?,
            })
        },
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(PushError::Server(other.to_string())),
    })
}

/// File keys under a user's tree that belong to one book. Client uploads nest
/// book files as `…/Books/<book_hash>/<file>`; the book hash is the client's
/// partialMD5, a different digest from the blob store's sha256 names.
fn book_file_keys(state: &SharedState, uid: &str, book_hash: &str) -> Vec<String> {
    let marker = format!("/Books/{book_hash}/");
    crate::storage::list_prefix(state, uid)
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.key)
        .filter(|key| key.contains(&marker))
        .collect()
}

fn db_poisoned() -> PushError {
    PushError::Server("db lock poisoned".into())
}

// ── create ──────────────────────────────────────────────────────────────────

/// POST /api/pushes — `{to, bookHash, message?}`. Validates the recipient and
/// that the sender holds both the books row and at least one stored file;
/// snapshots the books row and inserts a pending push.
pub async fn create(
    State(state): State<SharedState>,
    user: AuthUser,
    body: Bytes,
) -> Result<Response, Response> {
    let from_uid = user.id.clone();
    let body: Value = serde_json::from_slice(&body)
        .map_err(|_| PushError::Validation("invalid JSON body".into()).into_response())?;
    let to_id = body
        .get("to")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let book_hash = body
        .get("bookHash")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if to_id.is_empty() || book_hash.is_empty() || book_hash.len() > 128 || book_hash.contains('/')
    {
        return Err(
            PushError::Validation("missing or invalid recipient/book".into()).into_response(),
        );
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(
            PushError::Validation("message too long (max 500 chars)".into()).into_response(),
        );
    }
    // The file-on-server gate runs before anything is inserted: an offer
    // whose file never made it up is a client bug we name precisely, and the
    // walk needs no db lock.
    if book_file_keys(&state, &from_uid, &book_hash).is_empty() {
        return Err(
            PushError::NotFound("book file is not on the server (upload it first)".into())
                .into_response(),
        );
    }

    let created = crate::rfc3339::now_rfc3339();
    let (title, push_id) = {
        let conn = state.db.lock().map_err(|_| db_poisoned().into_response())?;
        // ⑪-P1: pushes are friend-to-friend recommendations — device
        // identities sit out on both sides. Check the sender first so a
        // device account gets the same rejection whichever role it plays.
        let sender_kind: String = conn
            .query_row(
                "SELECT kind FROM users WHERE id = ?1",
                params![from_uid],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "person".to_string());
        if sender_kind == "device" {
            return Err(
                PushError::Validation("device accounts cannot push books".into()).into_response(),
            );
        }
        // Resolve the recipient by the account name (the users.email column
        // is the human handle; login and the admin endpoints match it
        // exactly too).
        let recipient: Option<(String, i64, String)> = conn
            .query_row(
                "SELECT id, disabled, kind FROM users WHERE email = ?1",
                params![to_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let Some((to_uid, disabled, kind)) = recipient else {
            return Err(PushError::NotFound("no such user".into()).into_response());
        };
        if disabled != 0 {
            return Err(
                PushError::Validation("recipient account is disabled".into()).into_response(),
            );
        }
        if kind == "device" {
            return Err(
                PushError::Validation("cannot push to a device account".into()).into_response(),
            );
        }
        if to_uid == from_uid {
            return Err(PushError::Validation("cannot push to yourself".into()).into_response());
        }
        // Duplicate pending offer for the same book: refuse — the inbox is
        // not a retry queue (re-push after they decide instead).
        let dup: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM pushes
                 WHERE from_uid=?1 AND to_uid=?2 AND book_hash=?3 AND status='pending'",
                params![from_uid, to_uid, book_hash],
                |r| r.get(0),
            )
            .ok();
        if dup.is_some() {
            return Err(
                PushError::Conflict("an identical push is still pending".into()).into_response(),
            );
        }
        // Snapshot the sender's books row (same shape as §6.8 records.json
        // books[]) — the offer must survive later metadata edits or deletes.
        let snapshot: Option<Value> = conn
            .query_row(
                "SELECT book_hash, meta_hash, updated_at, deleted_at, synced_at, data
                 FROM books WHERE user_id=? AND book_hash=?",
                params![from_uid, book_hash],
                |r| {
                    Ok(json!({
                        "book_hash": r.get::<_, String>(0)?,
                        "meta_hash": r.get::<_, String>(1)?,
                        "updated_at": r.get::<_, Option<i64>>(2)?,
                        "deleted_at": r.get::<_, Option<i64>>(3)?,
                        "synced_at": r.get::<_, String>(4)?,
                        "data": serde_json::from_str::<Value>(&r.get::<_, String>(5)?)
                            .unwrap_or(Value::Null),
                    }))
                },
            )
            .ok();
        let Some(snapshot) = snapshot else {
            return Err(PushError::NotFound("you do not have this book".into()).into_response());
        };
        if snapshot.get("deleted_at").and_then(Value::as_i64).is_some() {
            return Err(
                PushError::Validation("this book is deleted in your library".into())
                    .into_response(),
            );
        }
        let title = snapshot
            .get("data")
            .and_then(|d| d.get("title"))
            .cloned()
            .unwrap_or(Value::Null);
        conn.execute(
            "INSERT INTO pushes
                 (from_uid, to_uid, book_hash, message, payload_json, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
            params![
                from_uid,
                to_uid,
                book_hash,
                message,
                serde_json::to_string(&snapshot)
                    .map_err(|e| { PushError::Server(e.to_string()).into_response() })?,
                created,
            ],
        )
        .map_err(|e| PushError::Server(e.to_string()).into_response())?;
        (title, conn.last_insert_rowid())
    };

    Ok(Json(json!({
        "pushed": true,
        "id": push_id,
        "to": to_id,
        "bookHash": book_hash,
        "createdAt": created,
        "title": title,
    }))
    .into_response())
}

// ── inbox ───────────────────────────────────────────────────────────────────

/// GET /api/pushes/inbox — the caller's pending pushes, newest first. The
/// client counts this response for the menu badge; no separate count route.
pub async fn inbox(State(state): State<SharedState>, user: AuthUser) -> Result<Response, Response> {
    let uid = user.id.clone();
    let items: Vec<Value> = {
        let conn = state.db.lock().map_err(|_| db_poisoned().into_response())?;
        let mut stmt = conn
            .prepare(
                "SELECT p.id, u.email, p.book_hash, p.message, p.created_at, p.payload_json
                 FROM pushes p JOIN users u ON u.id = p.from_uid
                 WHERE p.to_uid = ?1 AND p.status = 'pending'
                 ORDER BY p.id DESC",
            )
            .map_err(|e| PushError::Server(e.to_string()).into_response())?;
        let rows = stmt
            .query_map(params![uid], |r| {
                let payload: Value =
                    serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null);
                let data = payload.get("data").cloned().unwrap_or(Value::Null);
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "from": r.get::<_, String>(1)?,
                    "bookHash": r.get::<_, String>(2)?,
                    "title": data.get("title").cloned().unwrap_or(Value::Null),
                    "author": data
                        .get("author")
                        .or_else(|| data.get("authors"))
                        .cloned()
                        .unwrap_or(Value::Null),
                    "message": r.get::<_, String>(3)?,
                    "createdAt": r.get::<_, String>(4)?,
                }))
            })
            .map_err(|e| PushError::Server(e.to_string()).into_response())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| PushError::Server(e.to_string()).into_response())?);
        }
        out
    };
    Ok(Json(json!({ "items": items })).into_response())
}

// ── accept ──────────────────────────────────────────────────────────────────

/// POST /api/pushes/{id}/accept — link the sender's files into the
/// recipient's tree, then land the snapshot through the LWW upsert. Quota is
/// checked against the file bytes actually about to be linked; a failure
/// leaves the push pending so the recipient can free space and retry.
pub async fn accept(
    State(state): State<SharedState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, Response> {
    let uid = user.id.clone();
    // Pre-check outside the ingest so 404/409 answers don't touch the
    // filesystem; the ingest re-guards the status flip inside the db lock.
    {
        let conn = state.db.lock().map_err(|_| db_poisoned().into_response())?;
        let Some(push) = load_push(&conn, id).map_err(|e| e.into_response())? else {
            return Err(PushError::NotFound("no such push".into()).into_response());
        };
        if push.to_uid != uid {
            return Err(PushError::NotFound("no such push".into()).into_response());
        }
        if push.status != "pending" {
            return Err(PushError::Conflict("this push is already decided".into()).into_response());
        }
    }
    let summary = tokio::task::spawn_blocking(move || run_accept(&state, &uid, id))
        .await
        .map_err(|e| PushError::Server(e.to_string()).into_response())?
        .map_err(|e| e.into_response())?;
    Ok(Json(summary).into_response())
}

fn run_accept(state: &SharedState, to_uid: &str, id: i64) -> Result<Value, PushError> {
    let (from_uid, book_hash, payload_json) = {
        let conn = state.db.lock().map_err(|_| db_poisoned())?;
        let push = load_push(&conn, id)?;
        let Some(push) = push else {
            return Err(PushError::NotFound("no such push".into()));
        };
        if push.to_uid != to_uid || push.status != "pending" {
            return Err(PushError::Conflict("this push is already decided".into()));
        }
        (push.from_uid, push.book_hash, push.payload_json)
    };
    let payload: Value = serde_json::from_str(&payload_json)
        .map_err(|e| PushError::Server(format!("stored snapshot unreadable: {e}")))?;
    let Some(row) = crate::archive::book_row_from_value(&payload) else {
        return Err(PushError::Server("stored snapshot unreadable".into()));
    };

    // Sender's stored copies of this book. If they are gone (sender deleted
    // the book meanwhile), the push stays pending and the recipient sees why.
    let files_root = state.cfg.files_root();
    let mut sources = Vec::new();
    for key in book_file_keys(state, &from_uid, &book_hash) {
        let path = crate::storage::key_to_path(&files_root, &key)
            .ok_or_else(|| PushError::Server(format!("rejected key {key}")))?;
        let size = std::fs::metadata(&path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        sources.push((key, path, size));
    }
    if sources.is_empty() {
        return Err(PushError::Conflict(
            "sender's copy is no longer on the server".into(),
        ));
    }

    // Quota: hardlinked files report full size per holder, so the recipient's
    // usage grows by the book's real size even though the blob dedups.
    let add_bytes: i64 = sources.iter().map(|(_, _, s)| *s).sum();
    let used = crate::storage::compute_usage(state, to_uid)
        .map_err(|e| PushError::Server(e.to_string()))?;
    if used + add_bytes > state.cfg.quota_bytes {
        return Err(PushError::Quota("storage quota exceeded".into()));
    }

    // File→file hard links, skip-if-exists; a duplicate only happens when the
    // recipient already stores the same path (retry after a partial accept),
    // and skipping keeps the ingest idempotent.
    let mut linked = 0usize;
    let mut skipped = 0usize;
    let mut created: Vec<PathBuf> = Vec::new();
    let mut linked_keys: Vec<String> = Vec::new();
    for (key, src, _) in sources {
        let rel = key.split_once('/').map(|x| x.1).unwrap_or("");
        let dest_key = format!("{to_uid}/{rel}");
        let Some(dest) = crate::storage::key_to_path(&files_root, &dest_key) else {
            skipped += 1;
            continue;
        };
        if dest.exists() {
            skipped += 1;
            continue;
        }
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::hard_link(&src, &dest).is_ok() {
            linked += 1;
            created.push(dest);
            linked_keys.push(dest_key);
        } else {
            skipped += 1;
        }
    }

    // Records + status flip under one lock; `AND status='pending'` makes a
    // concurrent double-accept a no-op. If the flip loses the race we unlink
    // what we just linked so a declined push leaves nothing behind.
    let now = crate::rfc3339::now_rfc3339();
    {
        let mut conn = state.db.lock().map_err(|_| db_poisoned())?;
        store::upsert_book(&conn, to_uid, &row).map_err(|e| PushError::Server(e.to_string()))?;
        // Re-stamp the receive clock so a pull cursor already past the
        // snapshot's synced_at still sees the book (visibility only, no LWW
        // role) — same as §6.8 import.
        let _ = conn.execute(
            "UPDATE books SET synced_at=?1 WHERE user_id=?2 AND book_hash=?3",
            params![now, to_uid, row.book_hash],
        );
        let flipped = conn
            .execute(
                "UPDATE pushes SET status='accepted', decided_at=?1
                 WHERE id=?2 AND status='pending'",
                params![now, id],
            )
            .map_err(|e| PushError::Server(e.to_string()))?;
        if flipped == 0 {
            for path in &created {
                let _ = std::fs::remove_file(path);
            }
            return Err(PushError::Conflict("this push is already decided".into()));
        }
        #[cfg(feature = "library")]
        {
            // Catalog registration is best-effort: never fail the accept.
            for key in &linked_keys {
                let _ = crate::library::catalog::register_upload(&mut conn, to_uid, key);
            }
        }
    }

    Ok(json!({ "accepted": true, "filesLinked": linked, "filesSkipped": skipped }))
}

// ── decline ─────────────────────────────────────────────────────────────────

/// POST /api/pushes/{id}/decline — mark declined; the sender's data is never
/// touched and the row stays as the receipt.
pub async fn decline(
    State(state): State<SharedState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, Response> {
    let uid = user.id.clone();
    let now = crate::rfc3339::now_rfc3339();
    let conn = state.db.lock().map_err(|_| db_poisoned().into_response())?;
    let Some(push) = load_push(&conn, id).map_err(|e| e.into_response())? else {
        return Err(PushError::NotFound("no such push".into()).into_response());
    };
    if push.to_uid != uid {
        return Err(PushError::NotFound("no such push".into()).into_response());
    }
    if push.status != "pending" {
        return Err(PushError::Conflict("this push is already decided".into()).into_response());
    }
    conn.execute(
        "UPDATE pushes SET status='declined', decided_at=?1 WHERE id=?2",
        params![now, id],
    )
    .map_err(|e| PushError::Server(e.to_string()).into_response())?;
    Ok(Json(json!({ "declined": id })).into_response())
}
