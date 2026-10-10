//! Authenticated proxies: DeepL/Yandex translation, Edge TTS voices, Open
//! Library metadata search — per docs/selfhosted/sync-protocol.md §5.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::db::SharedState;
use crate::error::ApiError;

fn http_client() -> reqwest::Client {
    reqwest::Client::new()
}

fn upstream_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::server(anyhow::anyhow!("upstream: {e}"))
}

/// POST /api/deepl/translate — `{text:[...], source_lang, target_lang,
/// use_cache}` → `{translations:[{text}]}`. Without DEEPL_API_KEY+ENDPOINT a
/// harmless mock echoes the input so the client never errors.
pub async fn deepl_translate(
    State(state): State<SharedState>,
    _user: AuthUser,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let payload: Value = serde_json::from_slice(&body).map_err(|_| ApiError::Validation)?;
    if !payload.is_object() {
        // The reference decodes into a struct, so non-object bodies 400.
        return Err(ApiError::Validation);
    }
    let texts: Vec<String> = payload
        .get("text")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let target_lang = payload
        .get("target_lang")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let source_lang = payload
        .get("source_lang")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let (Some(key), Some(endpoint)) = (
        state.cfg.deepl_api_key.clone(),
        state.cfg.deepl_endpoint.clone(),
    ) else {
        let translations: Vec<Value> = texts.into_iter().map(|t| json!({ "text": t })).collect();
        return Ok(Json(json!({ "translations": translations })).into_response());
    };

    let mut payload = json!({ "text": texts, "target_lang": target_lang });
    if !source_lang.is_empty() {
        payload["source_lang"] = json!(source_lang);
    }
    let resp = http_client()
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .json(&payload)
        .send()
        .await
        .map_err(upstream_error)?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let bytes = resp.bytes().await.map_err(upstream_error)?;
    Ok((status, bytes).into_response())
}

/// POST /api/yandex-translate?endpoint=session|translate&text=a&text=b&lang=..
/// — form-forwards to the free public Yandex endpoint, passing the body through.
pub async fn yandex_translate(
    _user: AuthUser,
    Query(q): Query<HashMap<String, String>>,
    raw_query: axum::extract::RawQuery,
) -> Result<Response, ApiError> {
    let endpoint = match q.get("endpoint").map(String::as_str) {
        Some("session") => "session",
        _ => "translate",
    };
    let lang = q
        .get("lang")
        .or_else(|| q.get("target_lang"))
        .cloned()
        .unwrap_or_default();

    // The upstream API wants repeated `text=` query params; forward them raw
    // (the client already URL-encodes them) instead of round-tripping forms.
    let incoming = raw_query
        .0
        .unwrap_or_default()
        .trim_start_matches('?')
        .to_string();
    let texts: Vec<String> = q
        .iter()
        .filter(|(k, _)| k.as_str() == "text")
        .map(|(_, v)| v.clone())
        .collect();
    let mut upstream_query = format!("_={endpoint}");
    for t in &texts {
        upstream_query.push_str(&format!("&text={}", urlencoded(t)));
    }
    if !lang.is_empty() {
        upstream_query.push_str(&format!("&lang={}", urlencoded(&lang)));
    }
    let _ = incoming;

    let url = format!("https://translate.yandex.net/api/v1/tr.json/translate?{upstream_query}");
    let resp = http_client()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send()
        .await
        .map_err(upstream_error)?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let bytes = resp.bytes().await.map_err(upstream_error)?;
    Ok((status, bytes).into_response())
}

fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// GET /api/tts/edge — real voice list via Microsoft's public HTTP endpoint.
/// POST (synthesis) is a documented gap: the reference drives the Edge
/// WebSocket protocol; until ported, respond 500 with the contract envelope.
pub async fn edge_tts(
    State(_state): State<SharedState>,
    _user: AuthUser,
    method: axum::http::Method,
) -> Result<Response, ApiError> {
    const TRUSTED_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
    if method == axum::http::Method::GET {
        let url = format!(
            "https://speech.platform.bing.com/consumer/speech/synthesize/readaloud/voices/list?trustedclienttoken={TRUSTED_TOKEN}"
        );
        let resp = http_client()
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
            )
            .send()
            .await
            .map_err(upstream_error)?;
        let voices: Value = resp.json().await.map_err(upstream_error)?;
        let out: Vec<Value> = voices
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| {
                        json!({
                            "name": v.get("ShortName").cloned().unwrap_or(Value::Null),
                            "display_name": v.get("FriendlyName").cloned().unwrap_or(Value::Null),
                            "locale": v.get("Locale").cloned().unwrap_or(Value::Null),
                            "gender": v.get("Gender").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Json(out).into_response());
    }
    if method == axum::http::Method::POST {
        // TODO: port the Edge TTS WebSocket synthesis protocol.
        return Err(ApiError::server(anyhow::anyhow!(
            "edge tts synthesis not ported yet"
        )));
    }
    Ok(StatusCode::METHOD_NOT_ALLOWED.into_response())
}

/// GET /api/metadata/search?q= — Open Library with the reference's result shape.
pub async fn metadata_search(
    _user: AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let Some(query) = q.get("q").filter(|s| !s.is_empty()) else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing q", "code": "VALIDATION" })),
        )
            .into_response());
    };
    let url = format!(
        "https://openlibrary.org/search.json?q={}&limit=20",
        urlencoded(query)
    );
    let resp = http_client()
        .get(&url)
        .send()
        .await
        .map_err(upstream_error)?;
    let body: Value = resp.json().await.map_err(upstream_error)?;
    let out: Vec<Value> = body
        .get("docs")
        .and_then(Value::as_array)
        .map(|docs| {
            docs.iter()
                .map(|d| {
                    let cover_i = d.get("cover_i").and_then(Value::as_i64).unwrap_or(0);
                    let cover = if cover_i != 0 {
                        format!("https://covers.openlibrary.org/b/id/{cover_i}-L.jpg")
                    } else {
                        String::new()
                    };
                    json!({
                        "title": d.get("title").cloned().unwrap_or(Value::Null),
                        "authors": d.get("author_name").cloned().unwrap_or(Value::Null),
                        "cover": cover,
                        "publish_year": d.get("first_publish_year").cloned().unwrap_or(Value::Null),
                        "identifiers": { "isbn": d.get("isbn").cloned().unwrap_or(json!([])) },
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(json!({ "results": out })).into_response())
}
