//! Catalog registration: one authoritative bibliography row per distinct
//! book content hash. Rows are created when a blob lands under a
//! `Readest/Books/<hash>/` key; metadata is taken from the uploader's book
//! record (the client syncs full book JSON into `books`) and only filled
//! where still empty, so later uploads never clobber admin/OL enrichments.

use rusqlite::{params, Connection};

use crate::storage::book_hash_from_key;

/// Best-effort catalog registration for an uploaded file. `Ok(false)` means
/// "nothing to do" (not a book key); errors bubble only as reported results —
/// callers must treat failures as non-fatal.
pub fn register_upload(conn: &mut Connection, uid: &str, key: &str) -> anyhow::Result<bool> {
    let Some(hash) = book_hash_from_key(key) else {
        return Ok(false);
    };
    let tx = conn.transaction()?;

    // Cover/metadata files share the book's hash directory; only a real book
    // file (an extension the reader can open) anchors the catalog entry — but
    // register the hash as soon as anything lands so downloads can find it.
    let file_name = key.rsplit('/').next().unwrap_or("");
    let fallback_title = file_name.rsplit_once('.').map_or(file_name, |(t, _)| t);

    let (mut title, mut author, mut language) = (
        fallback_title.to_string(),
        String::new(),
        String::new(),
    );
    if let Ok(Some(row)) = crate::store::get_book(&tx, uid, &hash) {
        if let Some(obj) = row.data.as_object() {
            let pick = |field: &str| -> Option<String> {
                obj.get(field)
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        obj.get("metadata")
                            .and_then(|m| m.get(field))
                            .and_then(serde_json::Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    })
            };
            if let Some(t) = pick("title") {
                title = t;
            }
            if let Some(a) = pick("author") {
                author = a;
            }
            if let Some(l) = pick("language") {
                language = l;
            }
        }
    }

    let now = crate::rfc3339::now_rfc3339();
    tx.execute(
        "INSERT INTO catalog_books (hash, title, author, language, work_id, source_hash, added_at, added_by)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6)
         ON CONFLICT(hash) DO UPDATE SET
             title = CASE WHEN catalog_books.title = '' THEN excluded.title ELSE catalog_books.title END,
             author = CASE WHEN catalog_books.author = '' THEN excluded.author ELSE catalog_books.author END,
             language = CASE WHEN catalog_books.language = '' THEN excluded.language ELSE catalog_books.language END",
        params![hash, title, author, language, now, uid],
    )?;
    tx.commit()?;
    Ok(true)
}
