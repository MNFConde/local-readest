use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

/// Wire contract claims: `sub` is the data-isolation key, `plan: "pro"` is
/// what the client treats as "all features unlocked". `email` is extra
/// (display-only for the client's `jwtToUser`).
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub plan: String,
    pub exp: usize,
    pub iat: usize,
}

/// Short-lived on purpose: the client silently re-logs-in with the persisted
/// login code via AuthContext.refresh when `exp` passes (see sync-protocol §1).
const TOKEN_TTL_SECS: usize = 60 * 60;

pub fn issue(secret: &str, sub: &str, email: Option<&str>, plan: &str) -> anyhow::Result<String> {
    let now = now_secs();
    let claims = Claims {
        sub: sub.to_string(),
        email: email.map(str::to_string),
        plan: plan.to_string(),
        iat: now,
        exp: now + TOKEN_TTL_SECS,
    };
    Ok(encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?)
}

/// Verify signature + expiry; returns the `sub` (user id).
pub fn verify(secret: &str, token: &str) -> anyhow::Result<Claims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?;
    Ok(data.claims)
}

fn now_secs() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as usize)
        .unwrap_or(0)
}
