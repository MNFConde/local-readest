use std::sync::Mutex;

use rusqlite::Connection;

use crate::config::Config;

/// Application state shared by all handlers.
///
/// A single SQLite connection behind a mutex: home-scale traffic is tiny and
/// the queries are sub-millisecond, so serializing them is fine and sidesteps
/// WAL/busy handling entirely.
/// Latest host-metrics snapshot POSTed by the supervisor (ROADMAP 待办⑦).
/// In-memory only: it is a live feed, losing it on restart is fine.
pub struct HostSnapshotEntry {
    pub received_at_unix: u64,
    pub body: serde_json::Value,
}

pub struct AppState {
    pub db: Mutex<Connection>,
    pub cfg: Config,
    /// Brute-force lock state for POST /api/auth (in-memory by design: a
    /// restart forgiving lockouts is acceptable for a self-hosted server).
    pub login_limiter: Mutex<crate::ratelimit::LoginLimiter>,
    /// Newest supervisor metrics snapshot, if the supervisor ever delivered
    /// one (None = "not connected" for the admin page's server monitor).
    pub host_snapshot: Mutex<Option<HostSnapshotEntry>>,
}

pub type SharedState = std::sync::Arc<AppState>;

/// Schema mirrors the ywz Go server's SQLite layout exactly (the wire contract
/// lives in the JSON payloads; column names matter only server-side, but keep
/// them identical anyway so operators can inspect both with the same queries).
const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS users (
        id TEXT PRIMARY KEY,
        email TEXT UNIQUE NOT NULL,
        code_sha256 TEXT NOT NULL,
        code_argon2 TEXT NOT NULL,
        plan TEXT NOT NULL DEFAULT 'pro',
        disabled INTEGER NOT NULL DEFAULT 0,
        role TEXT NOT NULL DEFAULT 'user',
        created_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS books (
        id TEXT, user_id TEXT, book_hash TEXT, meta_hash TEXT,
        updated_at INTEGER, deleted_at INTEGER, synced_at TEXT, data TEXT,
        PRIMARY KEY (user_id, book_hash)
    );
    CREATE TABLE IF NOT EXISTS notes (
        user_id TEXT, note_id TEXT, updated_at INTEGER, deleted_at INTEGER, data TEXT,
        PRIMARY KEY (user_id, note_id)
    );
    CREATE TABLE IF NOT EXISTS configs (
        user_id TEXT, config_id TEXT, updated_at INTEGER, data TEXT,
        PRIMARY KEY (user_id, config_id)
    );
    CREATE TABLE IF NOT EXISTS stat_books (
        user_id TEXT, book_hash TEXT, title TEXT, authors TEXT, updated_at_ms INTEGER, deleted_at INTEGER,
        PRIMARY KEY (user_id, book_hash)
    );
    CREATE TABLE IF NOT EXISTS stat_pages (
        user_id TEXT, book_hash TEXT, page INTEGER, start_time INTEGER, duration INTEGER, total_pages INTEGER, updated_at_ms INTEGER, deleted_at INTEGER,
        PRIMARY KEY (user_id, book_hash, page, start_time)
    );
    CREATE TABLE IF NOT EXISTS replicas (
        user_id TEXT, kind TEXT, replica_id TEXT, fields_jsonb TEXT, manifest_jsonb TEXT, deleted_at_ts TEXT, reincarnation TEXT, updated_at_ts TEXT, schema_version INTEGER,
        PRIMARY KEY (user_id, kind, replica_id)
    );
    CREATE TABLE IF NOT EXISTS replica_keys (
        user_id TEXT, salt_id TEXT, alg TEXT, salt TEXT, created_at TEXT,
        PRIMARY KEY (user_id, salt_id)
    );
    -- library feature: one authoritative bibliography row per distinct book
    -- content (hash), plus the work-grouping layer above it. Reading state
    -- stays per-user in books/notes; these tables only describe the books.
    CREATE TABLE IF NOT EXISTS catalog_books (
        hash TEXT PRIMARY KEY,
        title TEXT NOT NULL DEFAULT '',
        author TEXT NOT NULL DEFAULT '',
        language TEXT NOT NULL DEFAULT '',
        work_id TEXT,
        source_hash TEXT,
        added_at TEXT NOT NULL,
        added_by TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS works (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL DEFAULT '',
        author TEXT NOT NULL DEFAULT ''
    );
"#;

/// Open (creating if needed) the metadata database and bootstrap the schema.
pub fn open_db(cfg: &Config) -> anyhow::Result<Connection> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let conn = Connection::open(cfg.db_path())?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    conn.execute_batch(SCHEMA)?;
    run_migrations(&conn)?;
    Ok(conn)
}

/// Open an in-memory database with the full schema; used by contract tests.
pub fn open_db_in_memory() -> anyhow::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    run_migrations(&conn)?;
    Ok(conn)
}

/// ALTERs for tables that already exist on disk from before a column was
/// introduced — `CREATE TABLE IF NOT EXISTS` never touches those. Keep in
/// sync with SCHEMA.
fn run_migrations(conn: &Connection) -> anyhow::Result<()> {
    if !table_has_column(conn, "users", "role")? {
        conn.execute_batch("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user'")?;
    }
    Ok(())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> anyhow::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|name| name.ok().as_deref() == Some(column));
    Ok(found)
}
