use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::Connection;

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    pub path: PathBuf,
}

pub fn sqlite_filename(dev: bool) -> &'static str {
    if dev {
        "pulsar-dev.db"
    } else {
        "pulsar.db"
    }
}

impl Database {
    pub fn open(app_data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(app_data_dir).context("create app data dir")?;
        harden_dir_permissions(app_data_dir);
        let db_path = app_data_dir.join(sqlite_filename(crate::runtime_util::is_dev()));
        tracing::info!(
            target: "db",
            path = %db_path.display(),
            app_env = crate::runtime_util::app_env(),
            "opening sqlite database"
        );
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
        conn.execute_batch(include_str!("../../migrations/006_wealth_positions.sql"))
            .context("run wealth_positions migration")?;
        conn.execute_batch(include_str!("../../migrations/007_bamboo.sql"))
            .context("run bamboo cache migration")?;
        conn.execute_batch(include_str!("../../migrations/008_agent_memories.sql"))
            .context("run agent_memories migration")?;
        Self::add_column_if_missing(
            conn,
            "strategy_param_sets",
            "cycle_budget_pct",
            "REAL NOT NULL DEFAULT 0.20",
        )?;
        Self::add_column_if_missing(conn, "broker_orders", "external_order_ref", "TEXT")?;
        Self::add_column_if_missing(conn, "order_intents", "external_order_ref", "TEXT")?;
        conn.execute_batch(include_str!("../../migrations/010_autonomy_p0.sql"))
            .context("run signal_outcomes migration")?;
        Self::add_column_if_missing(
            conn,
            "strategy_param_sets",
            "time_stop_hours",
            "REAL NOT NULL DEFAULT 24.0",
        )?;
        Self::add_column_if_missing(
            conn,
            "strategy_param_sets",
            "partial_tp_fraction",
            "REAL NOT NULL DEFAULT 1.0",
        )?;
        Self::add_column_if_missing(conn, "cycle_audits", "detail", "TEXT")?;
        Self::add_column_if_missing(conn, "cycle_audits", "blocked_histogram", "TEXT")?;
        Self::add_column_if_missing(conn, "cycle_audits", "cash", "REAL")?;
        Self::add_column_if_missing(conn, "cycle_audits", "executed_ids", "TEXT")?;
        conn.execute_batch(include_str!("../../migrations/011_coach_sessions.sql"))
            .context("run coach sessions migration")?;
        conn.execute_batch(include_str!("../../migrations/012_busha.sql"))
            .context("run busha cache migration")?;
        conn.execute_batch(include_str!("../../migrations/013_busha_pairs.sql"))
            .context("run busha pairs migration")?;
        Self::add_column_if_missing(conn, "broker_orders", "venue", "TEXT NOT NULL DEFAULT 'wealth'")?;
        conn.execute("UPDATE strategy_param_sets SET allowed_symbols = NULL", [])
            .context("clear allowed_symbols")?;
        Ok(())
    }

    fn add_column_if_missing(conn: &Connection, table: &str, column: &str, ty: &str) -> Result<()> {
        let has: bool = conn
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|name| name == column);
        if !has {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"),
                [],
            )
            .with_context(|| format!("add {table}.{column}"))?;
        }
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

#[cfg(test)]
mod tests {
    use super::sqlite_filename;

    #[test]
    fn sqlite_filename_splits_dev_and_production() {
        assert_eq!(sqlite_filename(false), "pulsar.db");
        assert_eq!(sqlite_filename(true), "pulsar-dev.db");
    }
}
