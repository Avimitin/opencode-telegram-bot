use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

pub trait SessionStore {
    /// Link a bot message to a session.
    fn link_message(&self, chat_id: i64, msg_id: i64, session_id: &str) -> Result<()>;

    /// Look up which session a bot message belongs to.
    fn get_by_message(&self, chat_id: i64, msg_id: i64) -> Result<Option<String>>;

    /// Get the most recently linked session for a chat.
    fn get_latest_session(&self, chat_id: i64) -> Result<Option<String>>;
}

pub struct SqliteSessionStore {
    conn: Connection,
}

impl SqliteSessionStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let conn = Connection::open(path).context("open session database")?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS msg_session (
                 id       INTEGER PRIMARY KEY AUTOINCREMENT,
                 chat_id  INTEGER NOT NULL,
                 msg_id   INTEGER NOT NULL,
                 session_id TEXT NOT NULL,
                 UNIQUE (chat_id, msg_id)
             );
             CREATE INDEX IF NOT EXISTS idx_chat_id
                 ON msg_session (chat_id);",
        )
        .context("initialize session database schema")?;

        Ok(SqliteSessionStore { conn })
    }
}

impl SessionStore for SqliteSessionStore {
    fn link_message(&self, chat_id: i64, msg_id: i64, session_id: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO msg_session (chat_id, msg_id, session_id) VALUES (?1, ?2, ?3)",
                params![chat_id, msg_id, session_id],
            )
            .context("link_message")?;
        Ok(())
    }

    fn get_by_message(&self, chat_id: i64, msg_id: i64) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT session_id FROM msg_session WHERE chat_id = ?1 AND msg_id = ?2")
            .context("get_by_message prepare")?;
        let result = stmt
            .query_row(params![chat_id, msg_id], |row| row.get(0))
            .optional()
            .context("get_by_message query")?;
        Ok(result)
    }

    fn get_latest_session(&self, chat_id: i64) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT session_id FROM msg_session WHERE chat_id = ?1 ORDER BY id DESC LIMIT 1",
            )
            .context("get_latest_session prepare")?;
        let result = stmt
            .query_row(params![chat_id], |row| row.get(0))
            .optional()
            .context("get_latest_session query")?;
        Ok(result)
    }
}

// Re-export the optional extension for ergonomic query handling
use rusqlite::OptionalExtension;
