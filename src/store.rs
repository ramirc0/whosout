//! The SQLite file that caches the channel and holds the derived day table.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::board::Day;
use crate::slack::Message;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS messages (
    ts TEXT PRIMARY KEY,
    user TEXT NOT NULL,
    text TEXT NOT NULL,
    thread_ts TEXT,
    latest_reply TEXT
);
CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS syncs (at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS absence_days (
    day TEXT NOT NULL,
    user TEXT NOT NULL,
    person TEXT NOT NULL,
    kind TEXT NOT NULL,
    part TEXT NOT NULL,
    message_ts TEXT NOT NULL,
    PRIMARY KEY (user, day)
);
";

/// The database under the data directory.
pub struct Store {
    db: Connection,
}

impl Store {
    /// Opens `whosout.db` in `dir`, creating the directory and the tables if they are missing.
    ///
    /// # Errors
    ///
    /// Fails when the directory or the database cannot be created or opened.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join("whosout.db");
        let db = Connection::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
        db.execute_batch(SCHEMA)?;
        Ok(Self { db })
    }

    /// Returns every cached message, oldest first.
    ///
    /// # Errors
    ///
    /// Fails when the query fails.
    pub fn messages(&self) -> Result<Vec<Message>> {
        let mut query = self.db.prepare("SELECT ts, user, text, thread_ts, latest_reply FROM messages ORDER BY ts")?;
        let rows = query.query_map([], |row| {
            Ok(Message {
                ts: row.get(0)?,
                user: row.get(1)?,
                text: row.get(2)?,
                thread_ts: row.get(3)?,
                latest_reply: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Returns the display name of each cached user id.
    ///
    /// # Errors
    ///
    /// Fails when the query fails.
    pub fn names(&self) -> Result<HashMap<String, String>> {
        let mut query = self.db.prepare("SELECT id, name FROM users")?;
        let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Returns when the last sync finished, if one ever did.
    ///
    /// # Errors
    ///
    /// Fails when the query fails.
    pub fn synced(&self) -> Result<Option<String>> {
        Ok(self.db.query_row("SELECT max(at) FROM syncs", [], |row| row.get(0))?)
    }

    /// Replaces the whole Slack cache in one transaction and records the sync time.
    ///
    /// # Errors
    ///
    /// Fails when a statement fails. The old cache then stays in place.
    pub fn replace_cache(&mut self, messages: &[Message], users: &[(String, String)]) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute_batch("DELETE FROM messages; DELETE FROM users; DELETE FROM syncs;")?;
        for message in messages {
            tx.execute(
                "INSERT OR REPLACE INTO messages VALUES (?1, ?2, ?3, ?4, ?5)",
                params![message.ts, message.user, message.text, message.thread_ts, message.latest_reply],
            )?;
        }
        for (id, name) in users {
            tx.execute("INSERT INTO users VALUES (?1, ?2)", params![id, name])?;
        }
        tx.execute("INSERT INTO syncs VALUES (?1)", [jiff::Timestamp::now().to_string()])?;
        Ok(tx.commit()?)
    }

    /// Replaces the derived table of one row per person per day away.
    ///
    /// # Errors
    ///
    /// Fails when a statement fails. The old table then stays in place.
    pub fn replace_days(&mut self, days: &[Day]) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM absence_days", [])?;
        for day in days {
            let kind = serde_json::to_value(day.kind)?;
            let part = serde_json::to_value(day.part)?;
            tx.execute(
                "INSERT INTO absence_days VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    day.date.to_string(),
                    day.user,
                    day.person,
                    kind.as_str(),
                    part.as_str(),
                    day.message_ts
                ],
            )?;
        }
        Ok(tx.commit()?)
    }
}
