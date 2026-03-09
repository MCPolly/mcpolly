use r2d2::{ManageConnection, Pool};
use rusqlite::{params, Connection};

pub type DbPool = Pool<SqliteConnectionManager>;

pub struct SqliteConnectionManager {
    path: String,
}

impl SqliteConnectionManager {
    pub fn file(path: &str) -> Self {
        Self {
            path: path.to_string(),
        }
    }
}

impl ManageConnection for SqliteConnectionManager {
    type Connection = Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Connection, rusqlite::Error> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )?;
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch("SELECT 1")
    }

    fn has_broken(&self, _conn: &mut Connection) -> bool {
        false
    }
}

pub fn init_db(database_url: &str) -> DbPool {
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *const i8,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> i32,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )));
    }

    let manager = SqliteConnectionManager::file(database_url);

    let pool = Pool::builder()
        .max_size(16)
        .build(manager)
        .expect("Failed to create connection pool");

    {
        let conn = pool.get().expect("Failed to get connection for migrations");
        run_migrations(&conn);
    }

    pool
}

fn run_migrations(conn: &Connection) {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS api_keys (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL DEFAULT '',
            key_hash TEXT NOT NULL UNIQUE,
            key_prefix TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT,
            revoked INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            description TEXT NOT NULL DEFAULT '',
            metadata_json TEXT,
            current_state TEXT NOT NULL DEFAULT 'offline',
            last_message TEXT,
            last_error_message TEXT,
            registered_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_update_at TEXT
        );

        CREATE TABLE IF NOT EXISTS status_updates (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL REFERENCES agents(id),
            state TEXT NOT NULL,
            message TEXT NOT NULL DEFAULT '',
            metadata_json TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_status_updates_agent_id ON status_updates(agent_id);
        CREATE INDEX IF NOT EXISTS idx_status_updates_created_at ON status_updates(created_at);

        CREATE TABLE IF NOT EXISTS errors (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL REFERENCES agents(id),
            message TEXT NOT NULL,
            severity TEXT NOT NULL DEFAULT 'error',
            stack_trace TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_errors_agent_id ON errors(agent_id);
        CREATE INDEX IF NOT EXISTS idx_errors_created_at ON errors(created_at);

        CREATE TABLE IF NOT EXISTS alert_rules (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            condition TEXT NOT NULL,
            agent_id TEXT,
            webhook_url TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            silence_minutes INTEGER NOT NULL DEFAULT 5,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS alert_history (
            id TEXT PRIMARY KEY,
            alert_rule_id TEXT NOT NULL REFERENCES alert_rules(id),
            agent_id TEXT,
            agent_name TEXT,
            condition TEXT NOT NULL,
            message TEXT NOT NULL DEFAULT '',
            delivery_status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_alert_history_created_at ON alert_history(created_at);

        CREATE TABLE IF NOT EXISTS stop_requests (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL REFERENCES agents(id),
            requested_by TEXT NOT NULL DEFAULT 'operator',
            reason TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            resolved_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_stop_requests_agent_id ON stop_requests(agent_id);
        CREATE INDEX IF NOT EXISTS idx_stop_requests_status ON stop_requests(status);

        CREATE TABLE IF NOT EXISTS embedding_documents (
            id TEXT PRIMARY KEY,
            source_type TEXT NOT NULL,
            source_name TEXT NOT NULL,
            chunk_index INTEGER NOT NULL,
            heading TEXT NOT NULL DEFAULT '',
            content TEXT NOT NULL,
            agent_id TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_embedding_docs_source ON embedding_documents(source_type, source_name);
        CREATE INDEX IF NOT EXISTS idx_embedding_docs_agent ON embedding_documents(agent_id);
        "
    ).expect("Failed to run migrations");

    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_embeddings USING vec0(
            document_id TEXT PRIMARY KEY,
            embedding float[384]
        )",
        [],
    )
    .expect("Failed to create vec_embeddings virtual table");

    tracing::info!("Database migrations completed successfully");
}

/// Seed a default API key if no keys exist. Returns the raw key if one was created.
pub fn seed_default_key_if_empty(conn: &Connection) -> Option<String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM api_keys WHERE revoked = 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if count == 0 {
        let raw_key = crate::auth::generate_raw_key();
        let key_hash = crate::auth::hash_key(&raw_key);
        let key_prefix = &raw_key[raw_key.len().saturating_sub(4)..];
        let id = uuid::Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO api_keys (id, label, key_hash, key_prefix) VALUES (?1, ?2, ?3, ?4)",
            params![id, "default", key_hash, key_prefix],
        )
        .unwrap();

        tracing::info!("No API keys found. Created default key.");
        tracing::info!("=== DEFAULT API KEY (save this, it won't be shown again) ===");
        tracing::info!("  {}", raw_key);
        tracing::info!("=============================================================");

        Some(raw_key)
    } else {
        None
    }
}
