//! Admin API + page: user account management (create/delete, password
//! resets, roles, enable/disable), catalog maintenance (works grouping,
//! metadata fixes, source_hash lineage, book deletion), and blob GC.
//!
//! Auth is dual-track: a logged-in admin user's JWT (role claim `admin`) is
//! the primary path — this is what the cross-platform client admin page uses —
//! while the shared `ADMIN_TOKEN` env remains as an operator fallback for the
//! embedded static page and scripting. Without either, every /api/admin route
//! answers 401 (403 for a valid non-admin JWT). Account creation used to be
//! CLI-only; it is available both places now.

use std::collections::HashSet;
use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};

use crate::db::SharedState;

pub fn router(state: SharedState) -> Router<SharedState> {
    Router::new()
        .route("/books", get(list_books))
        .route("/books/{hash}", post(update_book).delete(delete_book))
        .route("/works", get(list_works).post(create_work))
        .route("/enrich", post(enrich))
        .route("/gc", post(gc))
        .route("/users", get(list_users).post(create_user))
        .route("/users/{email}", delete(delete_user))
        .route("/users/{email}/password", post(reset_password))
        .route("/users/{email}/role", post(set_user_role))
        .route("/users/{email}/state", post(set_user_state))
        // Supervisor metrics feed (ROADMAP 待办⑦): the process manager POSTs
        // a snapshot every few seconds; the admin page reads the newest one.
        .route(
            "/host-snapshot",
            get(get_host_snapshot).post(post_host_snapshot),
        )
        .with_state(state)
}

/// Single-file admin page. No build chain: inline HTML+JS, token kept in
/// localStorage, everything else is fetch + render.
pub async fn admin_page() -> Response {
    let html = include_str!("admin.html");
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

// ── auth ────────────────────────────────────────────────────────────────────

fn bearer_token(headers: &HeaderMap) -> &str {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}

/// Admin JWT or the shared operator token, whichever verifies. 401 without a
/// usable credential, 403 for a valid JWT whose role is not `admin`.
async fn require_admin(state: &SharedState, headers: &HeaderMap) -> Result<(), Response> {
    let presented = bearer_token(headers);
    if presented.is_empty() {
        return Err(StatusCode::UNAUTHORIZED.into_response());
    }
    // Track 1: an admin user's own login JWT (the multi-platform admin page).
    if let Ok(claims) = crate::jwt::verify(&state.cfg.jwt_secret, presented) {
        if claims.role.as_deref() == Some("admin") {
            return Ok(());
        }
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    // Track 2: the shared operator token (embedded page / scripting). Kept as
    // an ops fallback; unset ADMIN_TOKEN simply leaves only track 1.
    if let Some(expected) = state.cfg.admin_token.as_deref() {
        // Constant-time-ish comparison via SHA-256 of both sides.
        use sha2::{Digest, Sha256};
        let a = Sha256::digest(expected.as_bytes());
        let b = Sha256::digest(presented.as_bytes());
        if a.as_slice() == b.as_slice() {
            return Ok(());
        }
    }
    Err(StatusCode::UNAUTHORIZED.into_response())
}

/// Email of the JWT caller, for the self-guards (never set on the operator
/// token track). Only called after `require_admin` succeeded.
fn caller_email(state: &SharedState, headers: &HeaderMap) -> Option<String> {
    crate::jwt::verify(&state.cfg.jwt_secret, bearer_token(headers))
        .ok()
        .and_then(|claims| claims.email)
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn list_books(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    let mut stmt = conn
        .prepare(
            "SELECT hash, title, author, language, work_id, source_hash, added_at, added_by
             FROM catalog_books ORDER BY added_at DESC",
        )
        .map_err(super::admin_error)?;
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
        .map_err(super::admin_error)?
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

    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    if setting_work {
        conn.execute(
            "INSERT OR IGNORE INTO works (id, title, author) VALUES (?1, '', '')",
            params![work_id],
        )
        .map_err(super::admin_error)?;
    }
    let n = conn
        .execute(
            &format!(
                "UPDATE catalog_books SET {} WHERE hash = ?{}",
                sets.join(", "),
                args.len()
            ),
            rusqlite::params_from_iter(args.iter()),
        )
        .map_err(super::admin_error)?;
    Ok(Json(json!({ "updated": n })).into_response())
}

async fn list_works(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.title, w.author, COUNT(b.hash)
             FROM works w LEFT JOIN catalog_books b ON b.work_id = w.id
             GROUP BY w.id ORDER BY w.id",
        )
        .map_err(super::admin_error)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "author": r.get::<_, String>(2)?,
                "books": r.get::<_, i64>(3)?,
            }))
        })
        .map_err(super::admin_error)?
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
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    conn.execute(
        "INSERT INTO works (id, title, author) VALUES (?1, ?2, ?3)",
        params![id, title, author],
    )
    .map_err(super::admin_error)?;
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
        let conn = state
            .db
            .lock()
            .map_err(|_| super::admin_error("db lock poisoned"))?;
        let mut stmt = conn
            .prepare(
                "SELECT hash, title, author FROM catalog_books
                 WHERE work_id IS NULL AND title != ''",
            )
            .map_err(super::admin_error)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(super::admin_error)?
            .flatten();
        rows.collect()
    };
    let client = reqwest::Client::new();
    let mut enriched = 0;
    let mut failed = Vec::new();
    for (hash, title, author) in &titles {
        match super::ol::lookup_work(&client, title, author).await {
            Ok(Some(hit)) => {
                let conn = state
                    .db
                    .lock()
                    .map_err(|_| super::admin_error("db lock poisoned"))?;
                conn.execute(
                    "INSERT OR IGNORE INTO works (id, title, author) VALUES (?1, '', '')",
                    params![hit.work_id],
                )
                .map_err(super::admin_error)?;
                conn.execute(
                    "UPDATE catalog_books SET work_id = ?2,
                     language = CASE WHEN language = '' THEN ?3 ELSE language END
                     WHERE hash = ?1",
                    params![hash, hit.work_id, hit.language],
                )
                .map_err(super::admin_error)?;
                enriched += 1;
            }
            Ok(None) => failed.push(json!({ "hash": hash, "error": "no OL match" })),
            Err(e) => failed.push(json!({ "hash": hash, "error": e.to_string() })),
        }
    }
    Ok(Json(json!({ "enriched": enriched, "failed": failed })).into_response())
}

async fn gc(State(state): State<SharedState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let removed = crate::storage::gc_blobs(&state.cfg);
    Ok(Json(json!({ "removed": removed })).into_response())
}

async fn list_users(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    let rows = crate::users::list_users(&conn).map_err(super::admin_error)?;
    let users: Vec<Value> = rows
        .iter()
        .map(|(email, id, disabled, created, role)| {
            json!({
                "email": email,
                "id": id,
                "disabled": disabled,
                "createdAt": created,
                "role": role,
            })
        })
        .collect();
    Ok(Json(json!({ "users": users })).into_response())
}

// ── user account management ─────────────────────────────────────────────────

/// POST /api/admin/users — create an account. Body:
/// `{"email": "...", "password": "...", "role": "user"|"admin"}`. Password is
/// optional: absent means the server generates a random one and returns it
/// exactly once; an empty string creates a passwordless account (login needs
/// only the email — the server warns about these at startup).
async fn create_user(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let payload: Value =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let email = payload
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?
        .to_string();
    let role = match payload.get("role") {
        None | Some(Value::Null) => "user".to_string(),
        Some(Value::String(s)) if s == "admin" || s == "user" => s.clone(),
        Some(_) => return Err(StatusCode::BAD_REQUEST.into_response()),
    };
    let password = match payload.get("password") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(StatusCode::BAD_REQUEST.into_response()),
    };

    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM users WHERE email = ?1",
            params![email],
            |r| r.get(0),
        )
        .ok();
    if exists.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": "email already exists" })),
        )
            .into_response());
    }
    let created =
        crate::users::add_user(&conn, &email, password, &role).map_err(super::admin_error)?;
    Ok(Json(json!({
        "email": created.email,
        // Returned exactly once; the UI must show it before it can't be recovered.
        "password": created.code,
        "role": role,
    }))
    .into_response())
}

/// DELETE /api/admin/users/{email} — remove the account, all of its synced
/// data rows, and its stored file copies. Catalog entries whose content no
/// longer has any holder copy are dropped too, so the shared library never
/// lists a book nobody can download. Deleting your own account is refused.
async fn delete_user(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(email): Path<String>,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    if caller_email(&state, &headers).as_deref() == Some(email.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "cannot delete your own account" })),
        )
            .into_response());
    }
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    let uid: Option<String> = conn
        .query_row(
            "SELECT id FROM users WHERE email = ?1",
            params![email],
            |r| r.get(0),
        )
        .ok();
    let Some(uid) = uid else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such user" })),
        )
            .into_response());
    };
    for table in [
        "books",
        "notes",
        "configs",
        "stat_books",
        "stat_pages",
        "replicas",
        "replica_keys",
    ] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE user_id = ?1"),
            params![uid],
        )
        .map_err(super::admin_error)?;
    }
    conn.execute("DELETE FROM users WHERE email = ?1", params![email])
        .map_err(super::admin_error)?;
    drop(conn);

    // Their uploaded copies are gone with the account; the files/ tree entry
    // is per-uid, so remove the whole directory.
    let user_dir = state.cfg.files_root().join(&uid);
    if user_dir.exists() {
        let _ = std::fs::remove_dir_all(&user_dir);
    }
    drop_catalog_without_holders(&state);
    Ok(Json(json!({ "deleted": email })).into_response())
}

/// POST /api/admin/users/{email}/password — body `{"password": "..."}`. An
/// empty string clears the password (passwordless account). Without a
/// `password` field a random one is generated and returned exactly once.
async fn reset_password(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(email): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let explicit: Option<Option<String>> = if body.is_empty() {
        None
    } else {
        let payload: Value =
            serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
        match payload.get("password") {
            None => None,
            Some(Value::String(s)) => Some(Some(s.clone())),
            Some(_) => return Err(StatusCode::BAD_REQUEST.into_response()),
        }
    };
    let had_explicit_password = matches!(explicit, Some(Some(_)));
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    match crate::users::reset_code(&conn, &email, explicit.flatten()) {
        Ok(Some(code)) => {
            if had_explicit_password {
                Ok(Json(json!({ "updated": 1 })).into_response())
            } else {
                Ok(Json(json!({ "updated": 1, "password": code })).into_response())
            }
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such user" })),
        )
            .into_response()),
        Err(e) => Err(super::admin_error(e)),
    }
}

/// POST /api/admin/users/{email}/role — body `{"role": "admin"|"user"}`.
/// Changing your own role is refused (demoting the last admin is a footgun;
/// use the CLI deliberately if ever needed).
async fn set_user_role(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(email): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let payload: Value =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .filter(|r| *r == "admin" || *r == "user")
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    if caller_email(&state, &headers).as_deref() == Some(email.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "cannot change your own role" })),
        )
            .into_response());
    }
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    match crate::users::set_role(&conn, &email, role) {
        Ok(true) => Ok(Json(json!({ "updated": 1, "role": role })).into_response()),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such user" })),
        )
            .into_response()),
        Err(e) => Err(super::admin_error(e)),
    }
}

// ── book deletion + catalog/file helpers ────────────────────────────────────

/// The hash ends up joined onto file paths, so reject anything that could
/// traverse or smuggle separators. Real content hashes are 64-char hex, but
/// older entries may carry other shapes — stay permissive beyond traversal.
fn is_safe_hash(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains("..")
        && s.chars().all(|c| c.is_ascii_graphic())
}

/// Every stored file: (key relative to files_root, full path).
fn collect_files(dir: &FsPath, rel: &mut String, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let pushed = rel.len();
        if !rel.is_empty() {
            rel.push('/');
        }
        rel.push_str(name);
        if is_dir {
            collect_files(&entry.path(), rel, out);
        } else {
            out.push((rel.clone(), entry.path()));
        }
        rel.truncate(pushed);
    }
}

/// Catalog rows whose content no longer has a single stored copy anywhere.
/// Any file under `Readest/Books/<hash>/` (covers included) counts as held.
fn drop_catalog_without_holders(state: &SharedState) {
    let mut files = Vec::new();
    collect_files(&state.cfg.files_root(), &mut String::new(), &mut files);
    let held: HashSet<String> = files
        .iter()
        .filter_map(|(key, _)| crate::storage::book_hash_from_key(key))
        .collect();
    let Ok(conn) = state.db.lock() else {
        return;
    };
    let catalog: Vec<String> = {
        let mut stmt = match conn.prepare("SELECT hash FROM catalog_books") {
            Ok(stmt) => stmt,
            Err(_) => return,
        };
        stmt.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    };
    for hash in catalog {
        if !held.contains(&hash) {
            let _ = conn.execute("DELETE FROM catalog_books WHERE hash = ?1", params![hash]);
        }
    }
}

/// DELETE /api/admin/books/{hash} — remove the catalog entry, every stored
/// copy (files and the content blob), and its covers. Per-user library rows
/// are the clients' own synced data and stay; a user who still holds the
/// book locally can re-share it by re-uploading.
async fn delete_book(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    if !is_safe_hash(&hash) {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    let n = conn
        .execute("DELETE FROM catalog_books WHERE hash = ?1", params![hash])
        .map_err(super::admin_error)?;
    drop(conn);
    if n == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no such book" })),
        )
            .into_response());
    }

    let mut files = Vec::new();
    collect_files(&state.cfg.files_root(), &mut String::new(), &mut files);
    for (key, path) in files {
        if crate::storage::book_hash_from_key(&key).as_deref() == Some(hash.as_str()) {
            // remove_file only unlinks one directory entry; hard-linked
            // siblings and the blob are untouched (never fs::write!).
            let _ = std::fs::remove_file(&path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent); // prune Books/<hash>/ when empty
            }
        }
    }
    let _ = std::fs::remove_file(state.cfg.blobs_root().join(&hash));
    Ok(Json(json!({ "deleted": 1 })).into_response())
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
    let disabled = payload
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let conn = state
        .db
        .lock()
        .map_err(|_| super::admin_error("db lock poisoned"))?;
    let n = crate::users::set_disabled(&conn, &email, disabled).map_err(super::admin_error)?;
    Ok(Json(json!({ "updated": n })).into_response())
}

// ── supervisor host snapshot ────────────────────────────────────────────────
// The supervisor (sync-server/supervisor/) pushes process/host metrics here
// every few seconds with the operator token; the admin page's server monitor
// reads the newest entry. In-memory only — a restart just marks it "not
// connected" until the next push. The server stores the payload verbatim
// (schema contract: docs/selfhosted/sync-protocol.md §6.6).

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn post_host_snapshot(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let snapshot: Value =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    if !snapshot.is_object() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    let mut slot = state
        .host_snapshot
        .lock()
        .map_err(|_| super::admin_error("snapshot lock poisoned"))?;
    *slot = Some(crate::db::HostSnapshotEntry {
        received_at_unix: unix_now(),
        body: snapshot,
    });
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn get_host_snapshot(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let slot = state
        .host_snapshot
        .lock()
        .map_err(|_| super::admin_error("snapshot lock poisoned"))?;
    let now = unix_now();
    Ok(match slot.as_ref() {
        None => Json(json!({ "available": false })).into_response(),
        Some(entry) => Json(json!({
            "available": true,
            "received_at": entry.received_at_unix,
            "age_secs": now.saturating_sub(entry.received_at_unix),
            "snapshot": entry.body,
        }))
        .into_response(),
    })
}
