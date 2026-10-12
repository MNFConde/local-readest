use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: String,
    pub jwt_secret: String,
    pub data_dir: PathBuf,
    pub quota_bytes: i64,
    pub max_body_bytes: usize,
    /// Online library (OPDS) visibility: false = each user only sees their
    /// own uploads in the catalog, true = every user sees every book.
    pub library_shared: bool,
    /// Admin API/page bearer token. None disables /api/admin entirely
    /// (safe default) while /admin still renders an explanatory page.
    pub admin_token: Option<String>,
    pub deepl_api_key: Option<String>,
    pub deepl_endpoint: Option<String>,
}

/// Per-Quota-User default: 2 GiB. The ywz Go server defaulted to "unlimited"
/// (1<<60); a real quota is safer for a multi-user deployment and callers can
/// still raise it via QUOTA_BYTES.
const DEFAULT_QUOTA_BYTES: i64 = 2 * 1024 * 1024 * 1024;

/// Per-request body cap. Axum's silent default is 2 MiB, which rejects every
/// real book upload on /api/storage/blob while tiny record payloads sail
/// through. Per-user accounting stays with quota_bytes; this only bounds one
/// request (and thus one blob held in memory).
const DEFAULT_MAX_BODY_BYTES: usize = 512 * 1024 * 1024;

/// Auto-provisioned secret lives inside the data dir, so it travels with the
/// data on backup/migration and rotates only when the data is removed.
const JWT_SECRET_FILE: &str = "jwt_secret.key";

/// Portable default: keep everything next to the executable so a
/// double-clicked sync-server.exe stores its data where it lives, independent
/// of whatever directory the process happened to start from.
fn default_data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("data")))
        .unwrap_or_else(|| PathBuf::from("./data"))
}

impl Config {
    /// Base config without the JWT requirement — used by CLI account
    /// management, which doesn't touch tokens.
    pub fn from_env_base() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());

        let data_dir = get("DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        let quota_bytes = get("QUOTA_BYTES")
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|q| *q > 0)
            .unwrap_or(DEFAULT_QUOTA_BYTES);
        let max_body_bytes = get("MAX_BODY_BYTES")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|m| *m > 0)
            .unwrap_or(DEFAULT_MAX_BODY_BYTES);
        let library_shared = !matches!(get("LIBRARY_SHARED").as_deref(), Some("0" | "false"));

        Self {
            listen_addr: get("LISTEN_ADDR").unwrap_or_else(|| ":8080".into()),
            jwt_secret: String::new(),
            data_dir,
            quota_bytes,
            max_body_bytes,
            library_shared,
            admin_token: get("ADMIN_TOKEN").filter(|t| t.len() >= 8),
            deepl_api_key: get("DEEPL_API_KEY"),
            deepl_endpoint: get("DEEPL_ENDPOINT"),
        }
    }

    /// # Errors
    /// An explicitly set but too-short `JWT_SECRET` is a configuration
    /// mistake and errors out; secret-file I/O failures propagate.
    pub fn from_env() -> anyhow::Result<Self> {
        let mut cfg = Self::from_env_base();
        cfg.jwt_secret = resolve_jwt_secret(&cfg.data_dir, std::env::var("JWT_SECRET").ok())?;
        Ok(cfg)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("readest.db")
    }

    pub fn files_root(&self) -> PathBuf {
        self.data_dir.join("files")
    }

    /// Content-addressed blob store backing the per-uid files/ tree via
    /// hard links (library feature): identical bytes are stored once.
    pub fn blobs_root(&self) -> PathBuf {
        self.data_dir.join("blobs")
    }
}

/// JWT secret resolution, portable-first:
/// 1. a valid `JWT_SECRET` env (>=16 chars) always wins;
/// 2. otherwise the secret is persisted to `<data_dir>/jwt_secret.key` —
///    generated on first boot, reused afterwards — so a double-clicked
///    server keeps tokens valid across restarts with zero configuration.
///    Removing the data dir rotates the secret and logs every device out.
fn resolve_jwt_secret(data_dir: &Path, env_secret: Option<String>) -> anyhow::Result<String> {
    if let Some(secret) = env_secret.filter(|v| !v.is_empty()) {
        if secret.len() >= 16 {
            return Ok(secret);
        }
        anyhow::bail!(
            "JWT_SECRET must be >=16 chars (got {})",
            secret.len()
        );
    }
    let file = data_dir.join(JWT_SECRET_FILE);
    if let Ok(existing) = std::fs::read_to_string(&file) {
        let secret = existing.trim();
        if secret.len() >= 16 {
            return Ok(secret.to_string());
        }
        // empty or truncated file: fall through and regenerate
    }
    let secret = generate_jwt_secret();
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&file, &secret)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    Ok(secret)
}

fn generate_jwt_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sync-server-cfg-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn env_secret_wins_over_persisted_file() {
        let dir = temp_data_dir("env-wins");
        std::fs::write(dir.join(JWT_SECRET_FILE), "0123456789abcdef0123456789abcdef").unwrap();
        let got = resolve_jwt_secret(&dir, Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0".into())).unwrap();
        assert_eq!(got, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn short_env_secret_is_an_error() {
        let dir = temp_data_dir("short-env");
        assert!(resolve_jwt_secret(&dir, Some("tooshort".into())).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generates_persists_and_reuses_secret() {
        let dir = temp_data_dir("gen-reuse");
        let first = resolve_jwt_secret(&dir, None).unwrap();
        assert!(first.len() >= 16);
        assert!(dir.join(JWT_SECRET_FILE).exists());
        let second = resolve_jwt_secret(&dir, None).unwrap();
        assert_eq!(first, second, "secret must survive restarts");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reuses_existing_file_secret() {
        let dir = temp_data_dir("file");
        std::fs::write(dir.join(JWT_SECRET_FILE), "0123456789abcdef0123456789abcdef\n").unwrap();
        let got = resolve_jwt_secret(&dir, None).unwrap();
        assert_eq!(got, "0123456789abcdef0123456789abcdef");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn regenerates_over_garbage_file() {
        let dir = temp_data_dir("garbage");
        std::fs::write(dir.join(JWT_SECRET_FILE), "tiny").unwrap();
        let got = resolve_jwt_secret(&dir, None).unwrap();
        assert!(got.len() >= 16);
        std::fs::remove_dir_all(&dir).ok();
    }
}
