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
            Ok(claims) => Ok(AuthUser {
                id: claims.sub,
                email: claims.email,
                plan: claims.plan,
            }),
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

/// POST /api/auth — body `{"code": "..."}`, header `X-Device-Id`.
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
    let code = parsed
        .ok()
        .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_string));
    let Some(code) = code else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "bad request" })),
        )
            .into_response();
    };

    let verified = crate::routes::with_db(&state, |conn| {
        users::verify_login(conn, &code).map_err(crate::error::ApiError::server)
    });
    match verified {
        Ok(Some(user)) => {
            state
                .login_limiter
                .lock()
                .expect("login limiter mutex poisoned")
                .record_success(&identity);
            match jwt::issue(
                &state.cfg.jwt_secret,
                &user.id,
                Some(&user.email),
                &user.plan,
            ) {
                Ok(token) => (
                    StatusCode::OK,
                    Json(json!({ "access_token": token, "token_type": "bearer" })),
                )
                    .into_response(),
                Err(_) => internal_error_response(),
            }
        }
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
