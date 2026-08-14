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
        harden_dir_permissions(app_data_dir);
        let db_path = app_data_dir.join("pulsar.db");
        let conn = Connection::open(&db_path).context("open sqlite db")?;
        harden_file_permissions(&db_path);
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
        let conn = self.conn.lock();
        Self::apply_migrations(&conn)
    }

    fn apply_migrations(conn: &Connection) -> Result<()> {
        let migration = include_str!("../../migrations/001_initial.sql");
        conn.execute_batch(migration)
            .context("run migrations")?;
        let migration2 = include_str!("../../migrations/002_broker_orders.sql");
        conn.execute_batch(migration2)
            .context("run broker_orders migration")?;
        let migration3 = include_str!("../../migrations/003_equity_curve_venue.sql");
        conn.execute_batch(migration3)
            .context("run equity_curve_venue migration")?;
        Self::migrate_equity_curve_recorded_at(conn)?;
        conn.execute_batch(include_str!("../../migrations/005_order_intents.sql"))
            .context("run order_intents migration")?;
        conn.execute("UPDATE strategy_param_sets SET allowed_symbols = NULL", [])
            .context("clear allowed_symbols")?;
        Ok(())
    }

    fn migrate_equity_curve_recorded_at(conn: &Connection) -> Result<()> {
        let has_recorded_at: bool = conn
            .prepare("PRAGMA table_info(equity_curve_points)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|name| name == "recorded_at");
        if has_recorded_at {
            return Ok(());
        }
        conn.execute_batch(include_str!("../../migrations/004_equity_curve_recorded_at.sql"))
            .context("run equity_curve recorded_at migration")
    }

    /// Drop all app tables, re-run migrations, and seed a fresh sandbox.
    pub fn wipe_and_reseed(&self, starting_capital: f64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch("PRAGMA foreign_keys = OFF;")
            .context("disable foreign keys")?;
        let tables: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        for name in tables {
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            conn.execute(&format!("DROP TABLE IF EXISTS \"{name}\""), [])
                .with_context(|| format!("drop table {name}"))?;
        }
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("enable foreign keys")?;
        Self::apply_migrations(&conn)?;
        crate::seed::SeedService::seed_if_empty(&conn, starting_capital)?;
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

fn harden_dir_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
}

fn harden_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}
