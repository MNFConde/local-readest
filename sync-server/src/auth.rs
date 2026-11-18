use std::net::SocketAddr;

use axum::extract::FromRequestParts;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::db::SharedState;
use crate::jwt;
use crate::ratelimit::RequestIdentity;
use crate::users;

/// Extractor for protected routes: verifies the Bearer JWT and yields the
/// user id (`sub`) as the data-isolation key. Rejections carry the exact
/// wire bodies from sync-protocol §1.
pub struct AuthUser {
    pub id: String,
    pub email: Option<String>,
    pub plan: String,
}

impl FromRequestParts<SharedState> for AuthUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let Some(token) = raw.strip_prefix("Bearer ") else {
            return Err(unauthorized("missing token"));
        };
        match jwt::verify(&state.cfg.jwt_secret, token) {
            Ok(claims) => {
                // Activity stamp for the admin users list: throttled inside
                // the UPDATE and best-effort — never blocks the request.
                let _ = crate::routes::with_db(state, |conn| {
                    users::touch_last_seen(conn, &claims.sub, users::now_unix())
                        .map_err(crate::error::ApiError::server)
                });
                Ok(AuthUser {
                    id: claims.sub,
                    email: claims.email,
                    plan: claims.plan,
                })
            }
            Err(_) => Err(unauthorized("invalid token")),
        }
    }
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": message, "code": "AUTH" })),
    )
        .into_response()
}

/// Client IP from the `ConnectInfo` extension; absent in tests (oneshot has
/// no connection info) and always present under `into_make_service_with_connect_info`.
pub struct ClientIp(pub Option<SocketAddr>);

impl FromRequestParts<SharedState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
        ))
    }
}

/// POST /api/auth — body `{"email": "...", "password": "..."}` (the account +
/// password model); header `X-Device-Id`.
///
/// Back-compat: `{"code": "..."}` from pre-password clients is accepted as an
/// email-less password (the code *is* the password — same column pair), so old
/// clients keep logging in without migration.
///
/// ⑪-P1 device identity: when `ALLOW_AUTO_PROVISION` is on, a login whose
/// email is a well-formed UUID and whose secret is >=16 chars creates its own
/// `kind='device'` account on first use (idempotent afterwards). Anything
/// else — non-UUID emails, short secrets, the flag being off — keeps failing
/// with the plain 401 and never creates a row.
///
/// Responses are written verbatim from the contract (the login endpoint's
/// 400/401 bodies have no `code` field — keep it that way).
pub async fn login(
    State(state): State<SharedState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let device_id = headers
        .get("X-Device-Id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let identity = RequestIdentity { ip, device_id };

    let locked = {
        let mut limiter = state
            .login_limiter
            .lock()
            .expect("login limiter mutex poisoned");
        limiter.allow(&identity).err()
    };
    if let Some(wait) = locked {
        return locked_response(wait);
    }

    let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&body);
    let creds = parsed.ok();
    let email = creds
        .as_ref()
        .and_then(|v| v.get("email"))
        .and_then(|e| e.as_str())
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string);
    // `code` is the legacy spelling of `password` (pre-password clients).
    let secret = creds
        .as_ref()
        .and_then(|v| v.get("password").or_else(|| v.get("code")))
        .and_then(|c| c.as_str())
        .map(str::to_string);
    // ⑪-P1 candidate data for device-identity first login. Kept out of the
    // dispatch below; only consulted in the Ok(None) arm and only when
    // ALLOW_AUTO_PROVISION is on.
    let provision = match (&email, &secret) {
        (Some(email), Some(secret)) if is_uuid_email(email) && secret.len() >= 16 => {
            Some((email.clone(), secret.clone()))
        }
        _ => None,
    };
    let verified = match (email, secret) {
        (Some(email), Some(secret)) => crate::routes::with_db(&state, |conn| {
            users::verify_login_email(conn, &email, &secret).map_err(crate::error::ApiError::server)
        }),
        // Legacy {code} / {password} without an email: the secret alone
        // identifies the user.
        (None, Some(secret)) => crate::routes::with_db(&state, |conn| {
            users::verify_login(conn, &secret).map_err(crate::error::ApiError::server)
        }),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "bad request" })),
            )
                .into_response()
        }
    };
    match verified {
        Ok(Some(user)) => login_granted(&state, &identity, user),
        Ok(None) => {
            // First-time device identity (⑪-P1): with the flag on, a
            // UUID-shaped email with a sufficiently long secret provisions
            // its own account and logs in like any other user. Everything
            // else keeps failing as a plain 401 — including a UUID login
            // with a wrong key against an existing account (the INSERT then
            // collides and re-verification rejects it).
            let provisioned = match provision {
                Some((ref email, ref secret)) if state.cfg.allow_auto_provision => {
                    crate::routes::with_db(&state, |conn| {
                        users::provision_device_user(conn, email, secret)
                            .map_err(crate::error::ApiError::server)
                    })
                }
                _ => Ok(None),
            };
            match provisioned {
                Ok(Some(user)) => login_granted(&state, &identity, user),
                Ok(None) => {
                    state
                        .login_limiter
                        .lock()
                        .expect("login limiter mutex poisoned")
                        .record_failure(&identity);
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "error": "invalid code" })),
                    )
                        .into_response()
                }
                Err(_) => internal_error_response(),
            }
        }
        Err(_) => internal_error_response(),
    }
}

/// Shared tail of every successful login: clear the limiter, stamp activity,
/// issue the JWT. Kept in one place so the auto-provision path (⑪-P1) is
/// behaviorally identical to a plain login.
fn login_granted(state: &SharedState, identity: &RequestIdentity, user: users::User) -> Response {
    state
        .login_limiter
        .lock()
        .expect("login limiter mutex poisoned")
        .record_success(identity);
    // Activity stamp for the admin users list; best-effort.
    let _ = crate::routes::with_db(state, |conn| {
        users::touch_last_seen(conn, &user.id, users::now_unix())
            .map_err(crate::error::ApiError::server)
    });
    match jwt::issue(
        &state.cfg.jwt_secret,
        &user.id,
        Some(&user.email),
        &user.plan,
        Some(&user.role),
    ) {
        Ok(token) => (
            StatusCode::OK,
            Json(json!({ "access_token": token, "token_type": "bearer" })),
        )
            .into_response(),
        Err(_) => internal_error_response(),
    }
}

/// Strict UUID shape (8-4-4-4-12 hex, either case) — what clients generate
/// with `crypto.randomUUID()`. No real person's email matches, so turning on
/// ALLOW_AUTO_PROVISION can never silently become open registration.
fn is_uuid_email(email: &str) -> bool {
    let is_hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit());
    let parts: Vec<&str> = email.split('-').collect();
    parts.len() == 5
        && is_hex(parts[0])
        && parts[0].len() == 8
        && is_hex(parts[1])
        && parts[1].len() == 4
        && is_hex(parts[2])
        && parts[2].len() == 4
        && is_hex(parts[3])
        && parts[3].len() == 4
        && is_hex(parts[4])
        && parts[4].len() == 12
}

fn locked_response(wait: std::time::Duration) -> Response {
    let secs = wait.as_secs().max(1).to_string();
    let mut resp = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "error": "too many failures, device locked",
            "code": "LOCKED"
        })),
    )
        .into_response();
    if let Ok(v) = secs.parse() {
        resp.headers_mut().insert("Retry-After", v);
    }
    resp
}

fn internal_error_response() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal" })),
    )
        .into_response()
}
