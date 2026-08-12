use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::Connection;

pub struct Database {
    conn: Arc<Mutex<Connection>>,
    pub path: PathBuf,
}

impl Database {
    pub fn open(app_data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(app_data_dir).context("create app data dir")?;
        let db_path = app_data_dir.join("pulsar.db");
        let conn = Connection::open(&db_path).context("open sqlite db")?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
            .context("set pragmas")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: db_path,
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let migration = include_str!("../../migrations/001_initial.sql");
        let conn = self.conn.lock();
        conn.execute_batch(migration)
            .context("run migrations")?;
        // Discontinue strategy symbol allowlists — universe is all active instruments.
        conn.execute("UPDATE strategy_param_sets SET allowed_symbols = NULL", [])
            .context("clear allowed_symbols")?;
        Ok(())
    }

    pub fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock();
        f(&conn)
    }
}
