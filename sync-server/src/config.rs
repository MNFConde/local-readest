use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: String,
    pub jwt_secret: String,
    pub data_dir: PathBuf,
    pub quota_bytes: i64,
    pub deepl_api_key: Option<String>,
    pub deepl_endpoint: Option<String>,
}

/// Per-Quota-User default: 2 GiB. The ywz Go server defaulted to "unlimited"
/// (1<<60); a real quota is safer for a multi-user deployment and callers can
/// still raise it via QUOTA_BYTES.
const DEFAULT_QUOTA_BYTES: i64 = 2 * 1024 * 1024 * 1024;

impl Config {
    /// # Errors
    /// Returns an error when `JWT_SECRET` is absent: unlike the Go reference
    /// (random secret per boot, silently invalidating all tokens) we require
    /// it so restarts don't log every device out.
    pub fn from_env() -> anyhow::Result<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());

        let jwt_secret = get("JWT_SECRET")
            .filter(|s| s.len() >= 16)
            .ok_or_else(|| {
                anyhow::anyhow!("JWT_SECRET must be set (>=16 chars); tokens survive restarts only while it stays the same")
            })?;

        let data_dir = PathBuf::from(get("DATA_DIR").unwrap_or_else(|| "./data".into()));
        let quota_bytes = get("QUOTA_BYTES")
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|q| *q > 0)
            .unwrap_or(DEFAULT_QUOTA_BYTES);

        Ok(Self {
            listen_addr: get("LISTEN_ADDR").unwrap_or_else(|| ":8080".into()),
            jwt_secret,
            data_dir,
            quota_bytes,
            deepl_api_key: get("DEEPL_API_KEY"),
            deepl_endpoint: get("DEEPL_ENDPOINT"),
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("readest.db")
    }

    pub fn files_root(&self) -> PathBuf {
        self.data_dir.join("files")
    }
}
