//! SQLite store operations, ported 1:1 from the Go reference's
//! internal/store/sqlite.go — including the row- and field-level LWW merge,
//! the synced_at tie-completion, and the HLC string ordering.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};

use crate::error::ApiError;
use crate::rfc3339;

// ── books ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BookRow {
    pub book_hash: String,
    pub meta_hash: String,
    pub updated_at: Option<i64>,
    pub deleted_at: Option<i64>,
    pub synced_at: String,
    pub data: Value,
}

/// (timestamp field, value field) pairs merged on their own clock when the
/// row loses at the row level.
const BOOK_FIELD_LWW: [(&str, &str); 4] = [
    ("reading_status_updated_at", "reading_status"),
    ("cover_updated_at", "cover_hash"),
    ("metadata_updated_at", "metadata"),
    ("metadata_updated_at", "pinned_at"),
];

fn snapshot_server_ts(server: &Map<String, Value>) -> Map<String, Value> {
    let mut orig = Map::new();
    for (ts_field, _) in BOOK_FIELD_LWW {
        if let Some(sf @ Value::String(_)) = server.get(ts_field) {
            orig.insert(ts_field.to_string(), sf.clone());
        }
    }
    orig
}

/// Server-wins path: graft the client's newer per-field values onto the
/// server row. Returns whether anything merged and the max grafted timestamp.
fn merge_book_fields_server_wins(
    server: &mut Map<String, Value>,
    client: &Map<String, Value>,
) -> (bool, String) {
    let mut merged = false;
    let mut max_ts = String::new();
    let orig = snapshot_server_ts(server);
    for (ts_field, val_field) in BOOK_FIELD_LWW {
        let cf = client.get(ts_field).and_then(Value::as_str).unwrap_or("");
        let sf = orig.get(ts_field).and_then(Value::as_str).unwrap_or("");
        if !cf.is_empty() && cf > sf {
            server.insert(ts_field.to_string(), Value::String(cf.to_string()));
            match client.get(val_field) {
                Some(v) => {
                    server.insert(val_field.to_string(), v.clone());
                }
                None => {
                    server.remove(val_field);
                }
            }
            if cf > max_ts.as_str() {
                max_ts = cf.to_string();
            }
            merged = true;
        }
    }
    (merged, max_ts)
}

/// Client-wins path: preserve server fields whose timestamp is >= the
/// client's (a peer's progress-only update must not drop another device's
/// metadata-clock edits). Compares against pre-merge server timestamps.
fn merge_book_fields_client_wins(client: &mut Map<String, Value>, server: &Map<String, Value>) {
    let orig = snapshot_server_ts(server);
    for (ts_field, val_field) in BOOK_FIELD_LWW {
        let sf = orig.get(ts_field).and_then(Value::as_str).unwrap_or("");
        let cf = client.get(ts_field).and_then(Value::as_str).unwrap_or("");
        if !sf.is_empty() && sf >= cf {
            client.insert(ts_field.to_string(), Value::String(sf.to_string()));
            match server.get(val_field) {
                Some(v) => {
                    client.insert(val_field.to_string(), v.clone());
                }
                None => {
                    client.remove(val_field);
                }
            }
        }
    }
}

fn ms_newer(client: Option<i64>, old: Option<i64>) -> bool {
    match (client, old) {
        (Some(c), None) => true,
        (Some(c), Some(o)) => c > o,
        _ => false,
    }
}

pub fn get_book(
    conn: &Connection,
    uid: &str,
    book_hash: &str,
) -> Result<Option<BookRow>, ApiError> {
    let row: Option<(Option<i64>, Option<i64>, String, String)> = conn
        .query_row(
            "SELECT updated_at, deleted_at, synced_at, data FROM books WHERE user_id=? AND book_hash=?",
            params![uid, book_hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(
        row.map(|(updated_at, deleted_at, synced_at, data)| BookRow {
            book_hash: book_hash.into(),
            meta_hash: String::new(),
            updated_at,
            deleted_at,
            synced_at,
            data: serde_json::from_str(&data).unwrap_or(Value::Null),
        }),
    )
}

pub fn upsert_book_raw(conn: &Connection, uid: &str, row: &BookRow) -> Result<(), ApiError> {
    conn.execute(
        "INSERT INTO books (id,user_id,book_hash,meta_hash,updated_at,deleted_at,synced_at,data)
         VALUES (?,?,?,?,?,?,?,?)
         ON CONFLICT(user_id,book_hash) DO UPDATE SET
             id=excluded.id, meta_hash=excluded.meta_hash, updated_at=excluded.updated_at,
             deleted_at=excluded.deleted_at, synced_at=excluded.synced_at, data=excluded.data",
        params![
            row.book_hash,
            uid,
            row.book_hash,
            row.meta_hash,
            row.updated_at,
            row.deleted_at,
            row.synced_at,
            row.data.to_string()
        ],
    )
    .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(())
}

/// Row-level LWW + field-level graft, mirroring UpsertBook.
pub fn upsert_book(conn: &Connection, uid: &str, row: &BookRow) -> Result<(), ApiError> {
    let Some(mut existing) = get_book(conn, uid, &row.book_hash)? else {
        return upsert_book_raw(conn, uid, row);
    };

    let client_newer = ms_newer(row.deleted_at, existing.deleted_at)
        || ms_newer(row.updated_at, existing.updated_at);

    if client_newer {
        // Incoming row wins wholesale; preserve the server's created_at and
        // any field-level values whose timestamp is >= the client's.
        if let (Some(server_obj), Some(mut client_obj)) = (
            existing.data.as_object().cloned(),
            row.data.as_object().cloned(),
        ) {
            if let Some(created) = server_obj.get("created_at") {
                client_obj.insert("created_at".into(), created.clone());
            }
            merge_book_fields_client_wins(&mut client_obj, &server_obj);
            let mut out = row.clone();
            out.data = Value::Object(client_obj);
            return upsert_book_raw(conn, uid, &out);
        }
        return upsert_book_raw(conn, uid, row);
    }

    // Server row wins; graft the client's newer fields.
    if let (Some(mut server_obj), Some(client_obj)) = (
        existing.data.as_object().cloned(),
        row.data.as_object().cloned(),
    ) {
        let (merged, max_ts) = merge_book_fields_server_wins(&mut server_obj, &client_obj);
        if merged {
            existing.data = Value::Object(server_obj);
            existing.synced_at = rfc3339::now_rfc3339();
            if !max_ts.is_empty() {
                if let Some(ms) = rfc3339::rfc3339_to_epoch_ms(&max_ts) {
                    existing.updated_at = Some(ms);
                }
            }
        }
    }
    upsert_book_raw(conn, uid, &existing)
}

pub struct PulledBook {
    pub data: String,
}

/// Pull by `synced_at` (server receive clock), LIMIT + tie-completion: all
/// rows sharing the tail synced_at are included so a client cursor never
/// skips rows pushed in the same instant.
pub fn pull_books(
    conn: &Connection,
    uid: &str,
    since_iso: &str,
    limit: i64,
) -> Result<Vec<PulledBook>, ApiError> {
    let since = if since_iso.is_empty() {
        "0001-01-01T00:00:00Z"
    } else {
        since_iso
    };
    let limit = if limit <= 0 { 1000 } else { limit };
    let mut stmt = conn
        .prepare(
            "SELECT book_hash, synced_at, data FROM books
             WHERE user_id=? AND synced_at > ? ORDER BY synced_at ASC LIMIT ?",
        )
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map(params![uid, since, limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
        .collect::<Result<_, _>>()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;

    let mut out: Vec<(String, String, String)> = rows;
    if let Some((_, tail, _)) = out.last().cloned() {
        let mut tie_stmt = conn
            .prepare("SELECT book_hash, synced_at, data FROM books WHERE user_id=? AND synced_at=?")
            .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
        let ties: Vec<(String, String, String)> = tie_stmt
            .query_map(params![uid, tail], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
            .collect::<Result<_, _>>()
            .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
        for t in ties {
            if !out.iter().any(|(h, _, _)| *h == t.0) {
                out.push(t);
            }
        }
    }
    Ok(out
        .into_iter()
        .map(|(_, _, data)| PulledBook { data })
        .collect())
}

// ── notes ───────────────────────────────────────────────────────────────────

/// LWW where deletions propagate: incoming wins if its updated_at or
/// deleted_at beats the stored row.
pub fn upsert_note(conn: &Connection, uid: &str, data: &Value) -> Result<(), ApiError> {
    let note_id = data.get("id").and_then(Value::as_str).unwrap_or("");
    let updated_at = data.get("updated_at").and_then(Value::as_i64);
    let deleted_at = data.get("deleted_at").and_then(Value::as_i64);

    let old: Option<(Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT updated_at, deleted_at FROM notes WHERE user_id=? AND note_id=?",
            params![uid, note_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;

    if let Some((old_updated, old_deleted)) = old {
        let client_newer = ms_newer(deleted_at, old_deleted) || ms_newer(updated_at, old_updated);
        if !client_newer {
            return Ok(());
        }
    }

    conn.execute(
        "INSERT INTO notes (user_id,note_id,updated_at,deleted_at,data)
         VALUES (?,?,?,?,?)
         ON CONFLICT(user_id,note_id) DO UPDATE SET
             updated_at=excluded.updated_at, deleted_at=excluded.deleted_at, data=excluded.data",
        params![uid, note_id, updated_at, deleted_at, data.to_string()],
    )
    .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(())
}

pub fn pull_notes(conn: &Connection, uid: &str, since_ms: i64) -> Result<Vec<String>, ApiError> {
    let mut stmt = conn
        .prepare(
            "SELECT data FROM notes
             WHERE user_id=? AND (COALESCE(updated_at,0) > ? OR COALESCE(deleted_at,0) > ?)
             ORDER BY COALESCE(updated_at,0) ASC",
        )
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map(params![uid, since_ms, since_ms], |r| r.get::<_, String>(0))
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
        .collect::<Result<_, _>>()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(rows)
}

// ── configs ─────────────────────────────────────────────────────────────────

/// One config per book; older pushes are dropped (equal or missing overwrite).
pub fn upsert_config(conn: &Connection, uid: &str, data: &Value) -> Result<(), ApiError> {
    let config_id = data.get("book_hash").and_then(Value::as_str).unwrap_or("");
    let updated_at = data.get("updated_at").and_then(Value::as_i64);

    let old: Option<Option<i64>> = conn
        .query_row(
            "SELECT updated_at FROM configs WHERE user_id=? AND config_id=?",
            params![uid, config_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;

    if let Some(old_ts) = old {
        if let (Some(new_ts), Some(old_ts)) = (updated_at, old_ts) {
            if new_ts < old_ts {
                return Ok(());
            }
        }
    }

    conn.execute(
        "INSERT INTO configs (user_id,config_id,updated_at,data) VALUES (?,?,?,?)
         ON CONFLICT(user_id,config_id) DO UPDATE SET updated_at=excluded.updated_at, data=excluded.data",
        params![uid, config_id, updated_at, data.to_string()],
    )
    .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(())
}

pub fn pull_configs(conn: &Connection, uid: &str, since_ms: i64) -> Result<Vec<String>, ApiError> {
    let mut stmt = conn
        .prepare(
            "SELECT data FROM configs WHERE user_id=? AND COALESCE(updated_at,0) > ? ORDER BY updated_at ASC",
        )
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map(params![uid, since_ms], |r| r.get::<_, String>(0))
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
        .collect::<Result<_, _>>()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(rows)
}

// ── stats ───────────────────────────────────────────────────────────────────

pub struct StatBookIn {
    pub book_hash: String,
    pub title: String,
    pub authors: String,
    pub updated_at_ms: Option<i64>,
    pub deleted_at: Option<i64>,
}

pub struct StatPageIn {
    pub book_hash: String,
    pub page: i64,
    pub start_time: i64,
    pub duration: i64,
    pub total_pages: i64,
    pub updated_at_ms: Option<i64>,
    pub deleted_at: Option<i64>,
}

pub fn upsert_stat_books(
    conn: &Connection,
    uid: &str,
    rows: &[StatBookIn],
) -> Result<(), ApiError> {
    for r in rows {
        conn.execute(
            "INSERT INTO stat_books (user_id,book_hash,title,authors,updated_at_ms,deleted_at)
             VALUES (?,?,?,?,?,?)
             ON CONFLICT(user_id,book_hash) DO UPDATE SET
                 title=excluded.title, authors=excluded.authors,
                 updated_at_ms=excluded.updated_at_ms, deleted_at=excluded.deleted_at",
            params![
                uid,
                r.book_hash,
                r.title,
                r.authors,
                r.updated_at_ms,
                r.deleted_at
            ],
        )
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    }
    Ok(())
}

/// Keep the longer session for the same (book, page, start_time) key.
pub fn upsert_stat_pages(
    conn: &Connection,
    uid: &str,
    rows: &[StatPageIn],
) -> Result<(), ApiError> {
    for r in rows {
        let old_dur: Option<i64> = conn
            .query_row(
                "SELECT duration FROM stat_pages WHERE user_id=? AND book_hash=? AND page=? AND start_time=?",
                params![uid, r.book_hash, r.page, r.start_time],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
            .flatten();
        if let Some(old) = old_dur {
            if r.duration < old {
                continue;
            }
        }
        conn.execute(
            "INSERT INTO stat_pages (user_id,book_hash,page,start_time,duration,total_pages,updated_at_ms,deleted_at)
             VALUES (?,?,?,?,?,?,?,?)
             ON CONFLICT(user_id,book_hash,page,start_time) DO UPDATE SET
                 duration=excluded.duration, total_pages=excluded.total_pages,
                 updated_at_ms=excluded.updated_at_ms, deleted_at=excluded.deleted_at",
            params![uid, r.book_hash, r.page, r.start_time, r.duration, r.total_pages, r.updated_at_ms, r.deleted_at],
        )
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    }
    Ok(())
}

fn stat_book_json(
    uid: &str,
    book_hash: &str,
    title: &str,
    authors: &str,
    updated_at_ms: Option<i64>,
    deleted_at: Option<i64>,
) -> Value {
    serde_json::json!({
        "user_id": uid, "book_hash": book_hash, "title": title, "authors": authors,
        "updated_at_ms": updated_at_ms, "deleted_at": deleted_at,
    })
}

pub fn pull_stat_books(
    conn: &Connection,
    uid: &str,
    since_ms: i64,
) -> Result<Vec<Value>, ApiError> {
    let mut stmt = conn
        .prepare(
            "SELECT book_hash,title,authors,updated_at_ms,deleted_at FROM stat_books
             WHERE user_id=? AND COALESCE(updated_at_ms,0) > ? ORDER BY updated_at_ms ASC",
        )
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map(params![uid, since_ms], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(rows
        .into_iter()
        .map(|(bh, t, a, u, d)| stat_book_json(uid, &bh, &t, &a, u, d))
        .collect())
}

fn stat_page_json(uid: &str, r: (String, i64, i64, i64, i64, Option<i64>, Option<i64>)) -> Value {
    serde_json::json!({
        "user_id": uid, "book_hash": r.0, "page": r.1, "start_time": r.2,
        "duration": r.3, "total_pages": r.4, "updated_at_ms": r.5, "deleted_at": r.6,
    })
}

pub fn pull_stat_pages(
    conn: &Connection,
    uid: &str,
    since_ms: i64,
) -> Result<Vec<Value>, ApiError> {
    let mut stmt = conn
        .prepare(
            "SELECT book_hash,page,start_time,duration,total_pages,updated_at_ms,deleted_at FROM stat_pages
             WHERE user_id=? AND COALESCE(updated_at_ms,0) > ? ORDER BY updated_at_ms ASC",
        )
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map(params![uid, since_ms], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
            ))
        })
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(rows.into_iter().map(|r| stat_page_json(uid, r)).collect())
}

// ── replicas (CRDT) ─────────────────────────────────────────────────────────

pub struct ReplicaIn {
    pub kind: String,
    pub replica_id: String,
    pub fields: String,
    pub manifest: String,
    pub deleted_at_ts: Option<String>,
    pub reincarnation: Option<String>,
    pub updated_at_ts: String,
    pub schema_version: i64,
}

pub struct ReplicaOut {
    pub replica_id: String,
    pub fields: String,
    pub manifest: Option<String>,
    pub deleted_at_ts: Option<String>,
    pub reincarnation: Option<String>,
    pub updated_at_ts: String,
    pub schema_version: i64,
}

/// LWW by HLC string; incoming older than stored is ignored.
pub fn upsert_replica(conn: &Connection, uid: &str, r: &ReplicaIn) -> Result<(), ApiError> {
    let old_ts: Option<String> = conn
        .query_row(
            "SELECT updated_at_ts FROM replicas WHERE user_id=? AND kind=? AND replica_id=?",
            params![uid, r.kind, r.replica_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
        .flatten();
    if let Some(old) = old_ts {
        if r.updated_at_ts < old {
            return Ok(());
        }
    }
    conn.execute(
        "INSERT INTO replicas (user_id,kind,replica_id,fields_jsonb,manifest_jsonb,deleted_at_ts,reincarnation,updated_at_ts,schema_version)
         VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(user_id,kind,replica_id) DO UPDATE SET
             fields_jsonb=excluded.fields_jsonb, manifest_jsonb=excluded.manifest_jsonb,
             deleted_at_ts=excluded.deleted_at_ts, reincarnation=excluded.reincarnation,
             updated_at_ts=excluded.updated_at_ts, schema_version=excluded.schema_version",
        params![
            uid, r.kind, r.replica_id, r.fields,
            if r.manifest.is_empty() { None::<String> } else { Some(r.manifest.clone()) },
            r.deleted_at_ts, r.reincarnation, r.updated_at_ts, r.schema_version
        ],
    )
    .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(())
}

pub fn pull_replicas(
    conn: &Connection,
    uid: &str,
    kind: &str,
    since: Option<&str>,
) -> Result<Vec<ReplicaOut>, ApiError> {
    let mut stmt = conn
        .prepare(
            "SELECT replica_id,fields_jsonb,manifest_jsonb,deleted_at_ts,reincarnation,updated_at_ts,schema_version
             FROM replicas WHERE user_id=? AND kind=? AND updated_at_ts > ? ORDER BY updated_at_ts ASC",
        )
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map(params![uid, kind, since.unwrap_or("")], |row| {
            Ok(ReplicaOut {
                replica_id: row.get(0)?,
                fields: row.get(1)?,
                manifest: row.get(2)?,
                deleted_at_ts: row.get(3)?,
                reincarnation: row.get(4)?,
                updated_at_ts: row.get(5)?,
                schema_version: row.get(6)?,
            })
        })
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(rows)
}

// ── replica keys (encryption salt envelopes) ────────────────────────────────

pub struct ReplicaKeyOut {
    pub salt_id: String,
    pub alg: String,
    pub salt: String,
    pub created_at: String,
}

pub fn upsert_replica_key(
    conn: &Connection,
    uid: &str,
    alg: &str,
    salt_id: &str,
    salt: &str,
) -> Result<(), ApiError> {
    conn.execute(
        "INSERT INTO replica_keys (user_id,salt_id,alg,salt,created_at) VALUES (?,?,?,?,?)
         ON CONFLICT(user_id,salt_id) DO UPDATE SET alg=excluded.alg, salt=excluded.salt, created_at=excluded.created_at",
        params![uid, salt_id, alg, salt, rfc3339::now_rfc3339()],
    )
    .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(())
}

pub fn list_replica_keys(conn: &Connection, uid: &str) -> Result<Vec<ReplicaKeyOut>, ApiError> {
    let mut stmt = conn
        .prepare("SELECT salt_id,alg,salt,created_at FROM replica_keys WHERE user_id=? ORDER BY created_at ASC")
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map(params![uid], |r| {
            Ok(ReplicaKeyOut {
                salt_id: r.get(0)?,
                alg: r.get(1)?,
                salt: r.get(2)?,
                created_at: r.get(3)?,
            })
        })
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(rows)
}

pub fn delete_replica_keys(conn: &Connection, uid: &str) -> Result<(), ApiError> {
    conn.execute("DELETE FROM replica_keys WHERE user_id=?", params![uid])
        .map_err(|e| ApiError::server(anyhow::anyhow!(e)))?;
    Ok(())
}
