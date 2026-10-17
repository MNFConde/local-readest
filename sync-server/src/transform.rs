//! Pushed (client-shaped, camelCase, epoch-ms) → stored (DB-shaped,
//! snake_case, ISO/ms) conversions, mirroring the Go reference's
//! internal/sync/transform.go field for field. The client parses pulled
//! records with transformBookFromDB / transformBookConfigFromDB /
//! transformBookNoteFromDB — storing pushed records verbatim crashes it.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::rfc3339;

/// sanitize strips NUL, mirroring utils/sanitize.ts sanitizeString.
pub fn sanitize(s: &str) -> String {
    s.replace('\u{0000}', "")
}

/// rawOrNil keeps a raw JSON sub-value verbatim; absent and null become None.
#[allow(non_snake_case)]
fn rawOrNil_field(v: Option<&serde_json::Value>) -> Option<Box<RawValue>> {
    match v {
        None => None,
        Some(serde_json::Value::Null) => None,
        Some(other) => RawValue::from_string(other.to_string()).ok(),
    }
}

/// JSON string form (for DB fields the client expects as strings, e.g.
/// `metadata`), absent/null → None.
fn stringified(v: Option<&serde_json::Value>) -> Option<String> {
    match v {
        None => None,
        Some(serde_json::Value::Null) => None,
        Some(other) => Some(other.to_string()),
    }
}

// ── pushed (wire) shapes ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushedBook {
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub meta_hash: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub source_title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub group_id: String,
    #[serde(default)]
    pub group_name: String,
    pub tags: Option<Vec<String>>,
    pub progress: Option<serde_json::Value>,
    #[serde(default)]
    pub reading_status: String,
    pub reading_status_updated_at: Option<i64>,
    pub cover_hash: Option<String>,
    pub cover_updated_at: Option<i64>,
    pub metadata: Option<serde_json::Value>,
    pub metadata_updated_at: Option<i64>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub deleted_at: Option<i64>,
    pub uploaded_at: Option<i64>,
    pub pinned_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushedConfig {
    #[serde(default)]
    pub book_hash: String,
    #[serde(default)]
    pub meta_hash: String,
    pub progress: Option<serde_json::Value>,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub xpointer: String,
    pub rsvp_position: Option<serde_json::Value>,
    pub search_config: Option<serde_json::Value>,
    pub view_settings: Option<serde_json::Value>,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushedNote {
    #[serde(default)]
    pub book_hash: String,
    #[serde(default)]
    pub meta_hash: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub cfi: String,
    #[serde(default)]
    pub xpointer0: String,
    #[serde(default)]
    pub xpointer1: String,
    pub page: Option<i64>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub note: String,
    pub global: Option<bool>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub deleted_at: Option<i64>,
}

// ── DB shapes (exactly the Go structs' json tags, including omitempty) ─────

fn omit_str(s: &str) -> bool {
    s.is_empty()
}

#[derive(Debug, Serialize)]
pub struct DbBook {
    pub user_id: String,
    pub book_hash: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub meta_hash: String,
    pub format: String,
    pub title: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub source_title: String,
    pub author: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub group_id: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub group_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Box<RawValue>>,
    #[serde(skip_serializing_if = "omit_str")]
    pub reading_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading_status_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_updated_at: Option<String>,
    pub metadata: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_updated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<String>,
    pub synced_at: String,
}

#[derive(Debug, Serialize)]
pub struct DbBookConfig {
    pub user_id: String,
    pub book_hash: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub meta_hash: String,
    pub progress: Option<String>,
    pub location: String,
    pub xpointer: String,
    pub rsvp_position: Option<String>,
    pub search_config: Option<String>,
    pub view_settings: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct DbBookNote {
    pub user_id: String,
    pub book_hash: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub meta_hash: String,
    pub id: String,
    pub r#type: String,
    pub cfi: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub xpointer0: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub xpointer1: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub text: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub style: String,
    #[serde(skip_serializing_if = "omit_str")]
    pub color: String,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global: Option<bool>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub deleted_at: Option<i64>,
}

pub fn book_to_db(p: &PushedBook, uid: &str, synced_at: &str) -> DbBook {
    let now = rfc3339::now_unix_ms();
    let created = p.created_at.unwrap_or(now);
    let updated = p.updated_at.unwrap_or(now);
    DbBook {
        user_id: uid.into(),
        book_hash: p.hash.clone(),
        meta_hash: p.meta_hash.clone(),
        format: p.format.clone(),
        title: sanitize(&p.title),
        source_title: sanitize(&p.source_title),
        author: sanitize(&p.author),
        group_id: p.group_id.clone(),
        group_name: sanitize(&p.group_name),
        tags: p.tags.clone(),
        progress: rawOrNil_field(p.progress.as_ref()),
        reading_status: p.reading_status.clone(),
        reading_status_updated_at: rfc3339::epoch_ms_to_rfc3339_opt(p.reading_status_updated_at),
        cover_hash: p.cover_hash.clone(),
        cover_updated_at: rfc3339::epoch_ms_to_rfc3339_opt(p.cover_updated_at),
        metadata: stringified(p.metadata.as_ref()),
        metadata_updated_at: rfc3339::epoch_ms_to_rfc3339_opt(p.metadata_updated_at),
        created_at: rfc3339::epoch_ms_to_rfc3339(created).unwrap_or_default(),
        updated_at: rfc3339::epoch_ms_to_rfc3339(updated).unwrap_or_default(),
        deleted_at: rfc3339::epoch_ms_to_rfc3339_opt(p.deleted_at),
        uploaded_at: rfc3339::epoch_ms_to_rfc3339_opt(p.uploaded_at),
        pinned_at: rfc3339::epoch_ms_to_rfc3339_opt(p.pinned_at),
        synced_at: synced_at.into(),
    }
}

pub fn config_to_db(p: &PushedConfig, uid: &str) -> DbBookConfig {
    DbBookConfig {
        user_id: uid.into(),
        book_hash: p.book_hash.clone(),
        meta_hash: p.meta_hash.clone(),
        progress: stringified(p.progress.as_ref()),
        location: p.location.clone(),
        xpointer: p.xpointer.clone(),
        rsvp_position: stringified(p.rsvp_position.as_ref()),
        search_config: stringified(p.search_config.as_ref()),
        view_settings: stringified(p.view_settings.as_ref()),
        created_at: p.updated_at,
        updated_at: p.updated_at,
        deleted_at: None,
    }
}

pub fn note_to_db(p: &PushedNote, uid: &str) -> DbBookNote {
    DbBookNote {
        user_id: uid.into(),
        book_hash: p.book_hash.clone(),
        meta_hash: p.meta_hash.clone(),
        id: p.id.clone(),
        r#type: p.r#type.clone(),
        cfi: p.cfi.clone(),
        xpointer0: p.xpointer0.clone(),
        xpointer1: p.xpointer1.clone(),
        page: p.page,
        text: sanitize(&p.text),
        style: p.style.clone(),
        color: p.color.clone(),
        note: p.note.clone(),
        global: p.global,
        created_at: p.created_at,
        updated_at: p.updated_at,
        deleted_at: p.deleted_at,
    }
}
