//! OPDS 1.2 (Atom XML) catalog over the shared book library. The Readest
//! client already browses OPDS catalogs natively (Tauri: direct HTTP, no
//! CORS involved; auth: HTTP Basic), so this exposes the server's
//! deduplicated catalog at /opds with zero client changes.
//!
//! Auth maps the client's catalog credential fields onto existing accounts:
//! Basic username = email, password = login code (argon2-verified; its cost
//! is the brute-force guard — home-LAN exposure is the intended deployment).

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine;
use rusqlite::params;

use crate::config::Config;
use crate::db::SharedState;
use crate::users::{self, User};

const PAGE_SIZE: i64 = 50;
const NAV_TYPE: &str = "application/atom+xml;profile=opds-catalog;kind=navigation";
const ACQ_TYPE: &str = "application/atom+xml;profile=opds-catalog;kind=acquisition";

pub fn router(state: SharedState) -> Router<SharedState> {
    Router::new()
        .route("/", get(root_feed))
        .route("/books", get(books_feed))
        .route("/works", get(works_feed))
        .route("/work/{id}", get(work_feed))
        .route("/search", get(search_feed))
        .route("/opensearch.xml", get(opensearch))
        .route("/download/{hash}", get(download))
        .route("/cover/{hash}", get(cover))
        .with_state(state)
}

// ── auth ────────────────────────────────────────────────────────────────────

fn challenge() -> Response {
    let mut resp = StatusCode::UNAUTHORIZED.into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        r#"Basic realm="readest-library""#.parse().unwrap(),
    );
    resp
}

fn basic_user(state: &SharedState, headers: &HeaderMap) -> Result<User, Response> {
    let credentials = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok())
        .and_then(|raw| String::from_utf8(raw).ok())
        .and_then(|v| {
            v.split_once(':')
                .map(|(u, p)| (u.trim().to_string(), p.to_string()))
        });
    let Some((email, code)) = credentials else {
        return Err(challenge());
    };
    let conn = state.db.lock().map_err(|_| challenge())?;
    // Wrong email / wrong code / disabled user all collapse into one 401 —
    // never reveal which part failed.
    users::verify_login_email(&conn, &email, &code)
        .map_err(|_| challenge())?
        .ok_or_else(challenge)
}

// ── catalog queries ─────────────────────────────────────────────────────────

struct CatalogRow {
    hash: String,
    title: String,
    author: String,
    language: String,
    added_at: String,
    work_id: Option<String>,
    work_title: Option<String>,
}

fn catalog_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogRow> {
    Ok(CatalogRow {
        hash: r.get(0)?,
        title: r.get(1)?,
        author: r.get(2)?,
        language: r.get(3)?,
        added_at: r.get(4)?,
        work_id: r.get(5)?,
        work_title: r.get(6)?,
    })
}

fn query_catalog(
    state: &SharedState,
    uid: &str,
    search: &str,
    page: i64,
) -> (Vec<CatalogRow>, i64) {
    let Ok(conn) = state.db.lock() else {
        return (Vec::new(), 0);
    };
    // library_shared=0 scopes the catalog to the caller's own uploads; the
    // uid is server-issued so quoting it is belt-and-braces.
    let scope = if state.cfg.library_shared {
        "1=1".to_string()
    } else {
        format!("cb.added_by = '{}'", uid.replace('\'', "''"))
    };
    let like = format!("%{}%", search.replace('\'', "''"));
    let total: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM catalog_books cb WHERE {scope}
                 AND (?1 = '' OR cb.title LIKE ?2 OR cb.author LIKE ?2)"
            ),
            params![search, like],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT cb.hash, cb.title, cb.author, cb.language, cb.added_at, cb.work_id, w.title
         FROM catalog_books cb LEFT JOIN works w ON w.id = cb.work_id
         WHERE {scope} AND (?1 = '' OR cb.title LIKE ?2 OR cb.author LIKE ?2)
         ORDER BY cb.added_at DESC, cb.hash LIMIT ?3 OFFSET ?4"
    )) else {
        return (Vec::new(), total);
    };
    let rows = stmt
        .query_map(
            params![search, like, PAGE_SIZE, (page - 1) * PAGE_SIZE],
            catalog_row,
        )
        .map(|rows| rows.flatten().collect::<Vec<_>>())
        .unwrap_or_default();
    (rows, total)
}

/// (work id, work title, number of editions) — only works with visible books.
fn query_works(state: &SharedState, uid: &str) -> Vec<(String, String, i64)> {
    let Ok(conn) = state.db.lock() else {
        return Vec::new();
    };
    let scope = if state.cfg.library_shared {
        "1=1".to_string()
    } else {
        format!("cb.added_by = '{}'", uid.replace('\'', "''"))
    };
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT w.id, w.title, COUNT(cb.hash) FROM works w
         JOIN catalog_books cb ON cb.work_id = w.id
         WHERE {scope} GROUP BY w.id, w.title ORDER BY w.title"
    )) else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn query_work_books(state: &SharedState, uid: &str, work_id: &str) -> Vec<CatalogRow> {
    let Ok(conn) = state.db.lock() else {
        return Vec::new();
    };
    let scope = if state.cfg.library_shared {
        "1=1".to_string()
    } else {
        format!("cb.added_by = '{}'", uid.replace('\'', "''"))
    };
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT cb.hash, cb.title, cb.author, cb.language, cb.added_at, cb.work_id, w.title
         FROM catalog_books cb LEFT JOIN works w ON w.id = cb.work_id
         WHERE cb.work_id = ?1 AND {scope}
         ORDER BY cb.added_at DESC, cb.hash"
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![work_id], catalog_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

// ── file discovery ──────────────────────────────────────────────────────────

struct Holder {
    path: PathBuf,
    name: String,
}

/// Every non-cover file stored under any user's Books/<hash>/ directory.
/// Home-scale full walk per request: a few hundred entries is sub-millisecond.
fn find_holders(cfg: &Config, hash: &str) -> Vec<Holder> {
    let mut out = Vec::new();
    let rel = &mut String::new();
    collect_book_files(&cfg.files_root(), hash, rel, &mut out);
    // Prefer formats the reader opens natively; covers are never acquisitions.
    let rank = |name: &str| -> u8 {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "epub" => 0,
            "pdf" => 1,
            "mobi" | "azw3" | "azw" => 2,
            "fb2" => 3,
            "cbz" | "cbr" => 4,
            "txt" | "html" | "htm" | "md" => 5,
            _ => 8,
        }
    };
    out.sort_by_key(|h| rank(&h.name));
    out
}

fn collect_book_files(dir: &FsPath, hash: &str, rel: &mut String, out: &mut Vec<Holder>) {
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
            collect_book_files(&entry.path(), hash, rel, out);
        } else {
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let is_cover = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp");
            if !is_cover && crate::storage::book_hash_from_key(rel).as_deref() == Some(hash) {
                out.push(Holder {
                    path: entry.path(),
                    name: name.to_string(),
                });
            }
        }
        rel.truncate(pushed);
    }
}

fn find_cover(cfg: &Config, hash: &str) -> Option<PathBuf> {
    let mut covers = Vec::new();
    let rel = &mut String::new();
    collect_cover_files(&cfg.files_root(), hash, rel, &mut covers);
    // Prefer png (what the client uploads), then jpg/webp.
    covers.sort_by_key(|p: &Holder| match p.name.rsplit('.').next().unwrap_or("") {
        "png" => 0,
        "jpg" | "jpeg" => 1,
        _ => 2,
    });
    covers.into_iter().next().map(|h| h.path)
}

fn collect_cover_files(dir: &FsPath, hash: &str, rel: &mut String, out: &mut Vec<Holder>) {
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
            collect_cover_files(&entry.path(), hash, rel, out);
        } else {
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp");
            if is_image && crate::storage::book_hash_from_key(rel).as_deref() == Some(hash) {
                out.push(Holder {
                    path: entry.path(),
                    name: name.to_string(),
                });
            }
        }
        rel.truncate(pushed);
    }
}

fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "epub" => "application/epub+zip",
        "pdf" => "application/pdf",
        "mobi" | "azw" => "application/x-mobipocket-ebook",
        "azw3" => "application/x-mobi8-ebook",
        "fb2" => "application/x-fictionbook+xml",
        "cbz" => "application/vnd.comicbook+zip",
        "cbr" => "application/vnd.comicbook-rar",
        "txt" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// RFC 5987 percent-encoding for Content-Disposition filename*=UTF-8''.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn file_response(path: &FsPath, mime: &str, download_name: Option<&str>) -> Response {
    let Ok(bytes) = std::fs::read(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut resp = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len());
    if let Some(name) = download_name {
        resp = resp.header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename*=UTF-8''{}", pct_encode(name)),
        );
    }
    resp.body(axum::body::Body::from(bytes)).unwrap()
}

// ── XML ─────────────────────────────────────────────────────────────────────

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn feed_open(id: &str, title: &str, self_href: &str, self_type: &str) -> String {
    let now = crate::rfc3339::now_rfc3339();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <feed xmlns=\"http://www.w3.org/2005/Atom\" \
               xmlns:opds=\"http://opds-spec.org/2010/catalog\" \
               xmlns:dcterms=\"http://purl.org/dc/terms/\">\n\
         \x20 <id>{id}</id>\n\
         \x20 <title>{}</title>\n\
         \x20 <updated>{now}</updated>\n\
         \x20 <author><name>Readest sync-server</name></author>\n\
         \x20 <link rel=\"self\" href=\"{self_href}\" type=\"{self_type}\"/>\n\
         \x20 <link rel=\"search\" href=\"/opds/opensearch.xml\" type=\"application/opensearchdescription+xml\"/>\n",
        xml_escape(title),
    )
}

fn entry_xml(row: &CatalogRow, holders: &[Holder]) -> String {
    let author = if row.author.is_empty() {
        String::new()
    } else {
        format!("<author><name>{}</name></author>\n", xml_escape(&row.author))
    };
    let language = if row.language.is_empty() {
        String::new()
    } else {
        format!(
            "<dcterms:language>{}</dcterms:language>\n",
            xml_escape(&row.language)
        )
    };
    let acquisition = holders.first().map(|h| {
        format!(
            "<link rel=\"http://opds-spec.org/acquisition\" href=\"/opds/download/{}\" type=\"{}\"/>\n",
            xml_escape(&row.hash),
            mime_for(&h.name),
        )
    });
    // Sibling editions of the same work are one collection hop away.
    let collection = match (&row.work_id, &row.work_title) {
        (Some(id), Some(title)) => format!(
            "<link rel=\"collection\" href=\"/opds/work/{}\" title=\"{}\" type=\"{ACQ_TYPE}\"/>\n",
            xml_escape(id),
            xml_escape(title),
        ),
        _ => String::new(),
    };
    format!(
        "<entry>\n\
         \x20 <id>urn:readest:book:{}</id>\n\
         \x20 <title>{}</title>\n\
         \x20 {}{}\
         \x20 <updated>{}</updated>\n\
         \x20 {}\
         \x20 {collection}\
         \x20 <link rel=\"http://opds-spec.org/image\" href=\"/opds/cover/{}\" type=\"image/png\"/>\n\
         \x20 <link rel=\"http://opds-spec.org/image/thumbnail\" href=\"/opds/cover/{}\" type=\"image/png\"/>\n\
         </entry>\n",
        xml_escape(&row.hash),
        xml_escape(&row.title),
        author,
        language,
        xml_escape(&row.added_at),
        acquisition.unwrap_or_default(),
        xml_escape(&row.hash),
        xml_escape(&row.hash),
    )
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn root_feed(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = basic_user(&state, &headers)?;
    let (_, total) = query_catalog(&state, &user.id, "", 1);
    let xml = format!(
        "{}<entry>\n\
         \x20 <id>urn:readest:library:books</id>\n\
         \x20 <title>All Books</title>\n\
         \x20 <updated>{}</updated>\n\
         \x20 <content type=\"text\">{total} book(s)</content>\n\
         \x20 <link rel=\"subsection\" href=\"/opds/books\" type=\"{ACQ_TYPE}\"/>\n\
         </entry>\n\
         <entry>\n\
         \x20 <id>urn:readest:library:works</id>\n\
         \x20 <title>By Work</title>\n\
         \x20 <updated>{}</updated>\n\
         \x20 <content type=\"text\">editions grouped by work</content>\n\
         \x20 <link rel=\"subsection\" href=\"/opds/works\" type=\"{NAV_TYPE}\"/>\n\
         </entry>\n</feed>\n",
        feed_open("urn:readest:library", "Readest Library", "/opds", NAV_TYPE),
        crate::rfc3339::now_rfc3339(),
        crate::rfc3339::now_rfc3339(),
    );
    Ok((
        [(header::CONTENT_TYPE, NAV_TYPE)],
        xml,
    )
        .into_response())
}

async fn books_feed(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, Response> {
    books_feed_impl(state, headers, q, "").await
}

async fn search_feed(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, Response> {
    let query = q.get("q").cloned().unwrap_or_default();
    books_feed_impl(state, headers, q, &query).await
}

async fn works_feed(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = basic_user(&state, &headers)?;
    let works = query_works(&state, &user.id);
    let mut xml = feed_open("urn:readest:library:works", "By Work", "/opds/works", NAV_TYPE);
    let now = crate::rfc3339::now_rfc3339();
    for (id, title, count) in &works {
        xml.push_str(&format!(
            "<entry>\n\
             \x20 <id>urn:readest:work:{}</id>\n\
             \x20 <title>{}</title>\n\
             \x20 <updated>{now}</updated>\n\
             \x20 <content type=\"text\">{count} edition(s)</content>\n\
             \x20 <link rel=\"subsection\" href=\"/opds/work/{}\" type=\"{ACQ_TYPE}\"/>\n\
             </entry>\n",
            xml_escape(id),
            xml_escape(title),
            xml_escape(id),
        ));
    }
    xml.push_str("</feed>\n");
    Ok(([(header::CONTENT_TYPE, NAV_TYPE)], xml).into_response())
}

async fn work_feed(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(work_id): Path<String>,
) -> Result<Response, Response> {
    let user = basic_user(&state, &headers)?;
    let rows = query_work_books(&state, &user.id, &work_id);
    let title = rows
        .first()
        .and_then(|r| r.work_title.clone())
        .unwrap_or_else(|| work_id.clone());
    let mut xml = feed_open(
        &format!("urn:readest:work:{work_id}"),
        &title,
        &format!("/opds/work/{work_id}"),
        ACQ_TYPE,
    );
    for row in &rows {
        let holders = find_holders(&state.cfg, &row.hash);
        xml.push_str(&entry_xml(row, &holders));
    }
    xml.push_str("</feed>\n");
    Ok(([(header::CONTENT_TYPE, ACQ_TYPE)], xml).into_response())
}

async fn books_feed_impl(
    state: SharedState,
    headers: HeaderMap,
    q: HashMap<String, String>,
    search: &str,
) -> Result<Response, Response> {
    let user = basic_user(&state, &headers)?;
    let page: i64 = q.get("page").and_then(|v| v.parse().ok()).unwrap_or(1);
    let page = page.max(1);
    let (rows, total) = query_catalog(&state, &user.id, search, page);

    let (title, self_href) = if search.is_empty() {
        ("All Books".to_string(), "/opds/books".to_string())
    } else {
        (
            format!("Search: {search}"),
            format!("/opds/search?q={}", pct_encode(search)),
        )
    };
    let mut xml = feed_open("urn:readest:library:books", &title, &self_href, ACQ_TYPE);
    for row in &rows {
        let holders = find_holders(&state.cfg, &row.hash);
        xml.push_str(&entry_xml(row, &holders));
    }
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    if page < total_pages {
        let next = if search.is_empty() {
            format!("/opds/books?page={}", page + 1)
        } else {
            format!("/opds/search?q={}&page={}", pct_encode(search), page + 1)
        };
        xml.push_str(&format!(
            "<link rel=\"next\" href=\"{next}\" type=\"{ACQ_TYPE}\"/>\n"
        ));
    }
    xml.push_str("</feed>\n");
    Ok(([(header::CONTENT_TYPE, ACQ_TYPE)], xml).into_response())
}

async fn opensearch() -> Response {
    let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
       <OpenSearchDescription xmlns=\"http://a9.com/-/spec/opensearch/1.1/\">\n\
       \x20 <ShortName>Readest Library</ShortName>\n\
       \x20 <Description>Search the self-hosted Readest library</Description>\n\
       \x20 <InputEncoding>UTF-8</InputEncoding>\n\
       \x20 <OutputEncoding>UTF-8</OutputEncoding>\n\
       \x20 <Url type=\"application/atom+xml;profile=opds-catalog;kind=acquisition\" template=\"/opds/search?q={searchTerms}\"/>\n\
       </OpenSearchDescription>\n";
    (
        [(header::CONTENT_TYPE, "application/opensearchdescription+xml")],
        xml,
    )
        .into_response()
}

async fn download(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Result<Response, Response> {
    basic_user(&state, &headers)?;
    let holders = find_holders(&state.cfg, &hash);
    let Some(holder) = holders.first() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok(file_response(
        &holder.path,
        mime_for(&holder.name),
        Some(&holder.name),
    ))
}

async fn cover(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Result<Response, Response> {
    basic_user(&state, &headers)?;
    let Some(path) = find_cover(&state.cfg, &hash) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let mime = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    Ok(file_response(&path, mime, None))
}
