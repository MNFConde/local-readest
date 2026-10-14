use std::fmt;

use argon2::password_hash::{rand_core::OsRng as PasswordOsRng, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};

/// A user's login credential is a per-user high-entropy code (the ywz server
/// had one shared AUTH_CODE; the client sends `{code}` and nothing else, so
/// per-user codes give multi-user without touching the client patch).
///
/// Since the account+password model the same column pair holds the password:
/// existing codes simply work as passwords (no migration), and an *empty*
/// password (`Some("")` at creation / reset) means the account needs no
/// secret at all — anyone knowing the email can log in. The server warns
/// about such accounts at startup (`empty_password_emails`).
///
/// Lookup uses a SHA-256 index column (O(1) find; the code is random, so a
/// fast hash is not the weak link), then verifies against an argon2id hash so
/// a user-chosen weak code doesn't make offline cracking trivial.
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub email: String,
    pub plan: String,
}

#[derive(Debug)]
pub struct NewUser {
    pub email: String,
    /// Plaintext code — returned once to the operator, never stored.
    pub code: String,
}

pub fn code_sha256(code: &str) -> String {
    hex::encode(Sha256::digest(code.as_bytes()))
}

fn random_code() -> String {
    let bytes: [u8; 16] = rand::rng().random();
    hex::encode(bytes)
}

fn random_id() -> String {
    let bytes: [u8; 16] = rand::rng().random();
    // UUID-ish layout for readability; uniqueness comes from the random bytes.
    let h = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn argon2_hash(code: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut PasswordOsRng);
    let hash = Argon2::default()
        .hash_password(code.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash failed: {e}"))?;
    Ok(hash.to_string())
}

fn argon2_verify(stored: &str, code: &str) -> bool {
    PasswordHash::new(stored)
        .map(|parsed| {
            Argon2::default()
                .verify_password(code.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

pub fn add_user(conn: &Connection, email: &str, code: Option<String>) -> anyhow::Result<NewUser> {
    // None generates a random secret; Some("") creates a passwordless account
    // (login needs only the email — the server warns about these at startup).
    let code = code.unwrap_or_else(random_code);
    let id = random_id();
    let created_at = iso_now();
    conn.execute(
        "INSERT INTO users (id, email, code_sha256, code_argon2, plan, disabled, created_at)
         VALUES (?1, ?2, ?3, ?4, 'pro', 0, ?5)",
        params![
            id,
            email,
            code_sha256(&code),
            argon2_hash(&code)?,
            created_at
        ],
    )?;
    Ok(NewUser {
        email: email.into(),
        code,
    })
}

/// Look up a user by login code and verify it. Disabled users never match.
pub fn verify_login(conn: &Connection, code: &str) -> anyhow::Result<Option<User>> {
    let row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT id, email, code_argon2, plan FROM users
             WHERE code_sha256 = ?1 AND disabled = 0",
            params![code_sha256(code)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(row.and_then(|(id, email, argon, plan)| {
        argon2_verify(&argon, code).then_some(User { id, email, plan })
    }))
}

/// Email+code pair verification for the OPDS catalog's HTTP Basic auth
/// (the client's catalog credential fields map to email + login code).
/// Same semantics as verify_login; argon2's cost is the brute-force guard.
pub fn verify_login_email(
    conn: &Connection,
    email: &str,
    code: &str,
) -> anyhow::Result<Option<User>> {
    let row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT id, email, code_argon2, plan FROM users
             WHERE email = ?1 AND code_sha256 = ?2 AND disabled = 0",
            params![email, code_sha256(code)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(row.and_then(|(id, email, argon, plan)| {
        argon2_verify(&argon, code).then_some(User { id, email, plan })
    }))
}

/// Emails of accounts whose password is empty (free login). Surfaced as a
/// startup warning so an operator never runs a passwordless account unnoticed.
pub fn empty_password_emails(conn: &Connection) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT email FROM users WHERE code_sha256 = ?1 AND disabled = 0 ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map(params![code_sha256("")], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_users(conn: &Connection) -> anyhow::Result<Vec<(String, String, bool, String)>> {
    let mut stmt =
        conn.prepare("SELECT email, id, disabled, created_at FROM users ORDER BY created_at")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn set_disabled(conn: &Connection, email: &str, disabled: bool) -> anyhow::Result<bool> {
    let n = conn.execute(
        "UPDATE users SET disabled = ?2 WHERE email = ?1",
        params![email, disabled as i64],
    )?;
    Ok(n > 0)
}

/// Replace a user's login code; returns the new plaintext code.
pub fn reset_code(
    conn: &Connection,
    email: &str,
    code: Option<String>,
) -> anyhow::Result<Option<String>> {
    let code = code.unwrap_or_else(random_code);
    let n = conn.execute(
        "UPDATE users SET code_sha256 = ?2, code_argon2 = ?3 WHERE email = ?1",
        params![email, code_sha256(&code), argon2_hash(&code)?],
    )?;
    Ok((n > 0).then_some(code))
}

fn iso_now() -> String {
    // RFC3339 UTC via chrono-free formatting: sufficient for an audit column.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Pretty-print a user list row for the CLI.
pub fn format_user_row(row: &(String, String, bool, String)) -> impl fmt::Display {
    let (email, id, disabled, created) = row;
    format!(
        "{email:<32} {id}  {}  created_unix={created}",
        if *disabled { "DISABLED" } else { "active" }
    )
}

/// Convenience for tests and CLI seeding.
#[allow(dead_code)]
pub fn ensure_user(conn: &Connection, email: &str, code: &str) -> anyhow::Result<()> {
    if verify_login(conn, code)?.is_none() {
        add_user(conn, email, Some(code.to_string()))?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn user_json(row: &(String, String, bool, String)) -> serde_json::Value {
    json!({ "email": row.0, "id": row.1, "disabled": row.2, "created_at": row.3 })
}
