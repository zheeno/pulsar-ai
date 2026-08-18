use anyhow::Result;
use rusqlite::Connection;

const DAILY_LIMIT: i64 = 100;
const MINUTE_LIMIT: i64 = 10;

pub struct RateLimiter;

impl RateLimiter {
    pub fn can_make_request(conn: &Connection) -> Result<bool> {
        let daily = Self::get_count(conn, &Self::daily_key())?;
        let minute = Self::get_count(conn, &Self::minute_key())?;
        Ok(daily < DAILY_LIMIT && minute < MINUTE_LIMIT)
    }

    pub fn record_request(conn: &Connection, endpoint: &str, auth_mode: &str) -> Result<()> {
        if auth_mode == "session" {
            conn.execute(
                "INSERT INTO ngx_pulse_usage_log (endpoint) VALUES (?1)",
                [endpoint],
            )?;
            return Ok(());
        }

        Self::increment(conn, &Self::daily_key(), 86400)?;
        Self::increment(conn, &Self::minute_key(), 120)?;
        conn.execute(
            "INSERT INTO ngx_pulse_usage_log (endpoint) VALUES (?1)",
            [endpoint],
        )?;
        Ok(())
    }

    pub fn usage_stats(conn: &Connection) -> Result<(i64, i64, i64)> {
        let daily = Self::get_count(conn, &Self::daily_key())?;
        Ok((daily, DAILY_LIMIT, DAILY_LIMIT - daily))
    }

    #[cfg(test)]
    pub fn reset(conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM rate_limit_counters", [])?;
        Ok(())
    }

    fn daily_key() -> String {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        format!("ngx_pulse:daily:{today}")
    }

    fn minute_key() -> String {
        let now = chrono::Utc::now().format("%Y-%m-%d:%H:%M").to_string();
        format!("ngx_pulse:minute:{now}")
    }

    fn get_count(conn: &Connection, key: &str) -> Result<i64> {
        let count: Option<i64> = conn
            .query_row(
                "SELECT count FROM rate_limit_counters WHERE key = ?1 AND (expires_at IS NULL OR expires_at > datetime('now'))",
                [key],
                |row| row.get(0),
            )
            .ok();
        Ok(count.unwrap_or(0))
    }

    fn increment(conn: &Connection, key: &str, ttl_secs: i64) -> Result<()> {
        let expires = chrono::Utc::now() + chrono::Duration::seconds(ttl_secs);
        conn.execute(
            "INSERT INTO rate_limit_counters (key, count, expires_at) VALUES (?1, 1, ?2)
             ON CONFLICT(key) DO UPDATE SET count = count + 1, expires_at = excluded.expires_at",
            rusqlite::params![key, expires.to_rfc3339()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn rate_limit_api_key_mode() {
        let dir = std::env::temp_dir().join(format!("pulsar-rl-{}", uuid::Uuid::new_v4()));
        let db = Database::open(&dir).expect("test db");
        db.with_conn(|conn| {
            RateLimiter::reset(conn)?;
            assert!(RateLimiter::can_make_request(conn)?);
            RateLimiter::record_request(conn, "stocks", "api_key")?;
            let (daily, _, _) = RateLimiter::usage_stats(conn)?;
            assert_eq!(daily, 1);
            Ok(())
        })
        .expect("rate limit test");
    }
}
