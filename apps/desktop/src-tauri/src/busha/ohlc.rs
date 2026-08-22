use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use super::BushaClient;
use crate::db::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BushaOhlcPeriod {
    OneDay,
    OneMonth,
    OneYear,
    AllTime,
}

impl BushaOhlcPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OneDay => "1d",
            Self::OneMonth => "1m",
            Self::OneYear => "1y",
            Self::AllTime => "all",
        }
    }

    pub fn as_query(self) -> Option<&'static str> {
        match self {
            Self::OneDay => Some("1d"),
            Self::OneMonth => Some("1m"),
            Self::OneYear => Some("1y"),
            Self::AllTime => None,
        }
    }

    pub fn default_cache_secs(self) -> i64 {
        match self {
            Self::OneDay => 300,
            Self::OneMonth => 3600,
            Self::OneYear | Self::AllTime => 86400,
        }
    }

    pub fn from_arg(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1d" | "day" => Some(Self::OneDay),
            "1m" | "month" => Some(Self::OneMonth),
            "1y" | "year" => Some(Self::OneYear),
            "all" | "alltime" | "all-time" => Some(Self::AllTime),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BushaOhlcPoint {
    pub ts: DateTime<Utc>,
    pub price: f64,
}

#[derive(Debug, Clone)]
pub struct BushaOhlcSnapshot {
    pub pair_id: String,
    pub base: String,
    pub price: f64,
    pub change_pct: f64,
    pub high: f64,
    pub low: f64,
    pub market_cap: f64,
    pub points: Vec<BushaOhlcPoint>,
}

#[derive(Debug, Default)]
pub struct OhlcIngestReport {
    pub fetched: usize,
    pub skipped_cache: usize,
    pub failed: Vec<(String, String)>,
}

pub fn resolve_pair_id(conn: &Connection, base: &str) -> String {
    let sym = base.trim().to_uppercase();
    conn.query_row(
        "SELECT pair_id FROM busha_pairs WHERE symbol = ?1",
        [&sym],
        |row| row.get::<_, String>(0),
    )
    .unwrap_or_else(|_| format!("{sym}NGN"))
}

impl BushaClient {
    pub async fn ohlc_for_base(&self, base: &str, period: BushaOhlcPeriod) -> Result<BushaOhlcSnapshot> {
        let sym = base.trim().to_uppercase();
        if sym.is_empty() {
            return Err(anyhow!("symbol is required"));
        }
        let pair_id = format!("{sym}NGN");
        let path = match period.as_query() {
            Some(p) => format!("/v1/currencies/ohlc/{pair_id}?period={p}"),
            None => format!("/v1/currencies/ohlc/{pair_id}"),
        };
        let v = self.get_json(&path).await?;
        parse_ohlc_response(&v, &sym)
    }
}

pub fn parse_ohlc_response(root: &Value, base: &str) -> Result<BushaOhlcSnapshot> {
    let status = root.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if !status.eq_ignore_ascii_case("success") {
        return Err(anyhow!(
            "Busha OHLC {}",
            root.get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("request failed")
        ));
    }
    let data = root
        .get("data")
        .ok_or_else(|| anyhow!("OHLC missing data"))?;
    let pair_id = data
        .get("symbol")
        .and_then(|v| v.as_str())
        .unwrap_or(base)
        .to_string();
    let price = parse_num_field(data.get("price"));
    let change_pct = parse_num_field(data.get("change"));
    let high = parse_num_field(data.get("high"));
    let low = parse_num_field(data.get("low"));
    let market_cap = parse_num_field(data.get("market_cap"));
    let arr = data
        .get("price_data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut points = Vec::with_capacity(arr.len());
    for item in arr {
        let ts_raw = item.get("time").and_then(|v| v.as_str()).unwrap_or("");
        let price = parse_num_field(item.get("price"));
        if ts_raw.is_empty() || !price.is_finite() || price <= 0.0 {
            continue;
        }
        let ts = DateTime::parse_from_rfc3339(ts_raw)
            .map(|dt| dt.with_timezone(&Utc))
            .with_context(|| format!("invalid OHLC timestamp {ts_raw}"))?;
        points.push(BushaOhlcPoint { ts, price });
    }
    points.sort_by_key(|p| p.ts);
    Ok(BushaOhlcSnapshot {
        pair_id,
        base: base.trim().to_uppercase(),
        price,
        change_pct,
        high,
        low,
        market_cap,
        points,
    })
}

fn parse_num_field(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn ohlc_meta_fresh(
    conn: &Connection,
    symbol: &str,
    period: BushaOhlcPeriod,
    max_age_secs: i64,
) -> Result<bool> {
    let sym = symbol.trim().to_uppercase();
    let fetched_at: Option<String> = conn
        .query_row(
            "SELECT fetched_at FROM busha_ohlc_meta WHERE symbol = ?1 AND period = ?2",
            rusqlite::params![sym, period.as_str()],
            |row| row.get(0),
        )
        .ok();
    let Some(fetched_at) = fetched_at else {
        return Ok(false);
    };
    let Ok(parsed) = DateTime::parse_from_rfc3339(&fetched_at) else {
        return Ok(false);
    };
    let age = Utc::now().signed_duration_since(parsed.with_timezone(&Utc));
    Ok(age.num_seconds() < max_age_secs)
}

pub fn persist_ohlc_snapshot(
    conn: &Connection,
    snap: &BushaOhlcSnapshot,
    period: BushaOhlcPeriod,
) -> Result<()> {
    let sym = snap.base.trim().to_uppercase();
    conn.execute(
        "INSERT INTO instruments (symbol, name, sector, is_active) VALUES (?1, ?1, 'CRYPTO', 1)
         ON CONFLICT(symbol) DO UPDATE SET sector = 'CRYPTO', is_active = 1",
        [&sym],
    )?;
    conn.execute(
        "INSERT INTO busha_ohlc_meta (symbol, period, snapshot_price, change_pct, high, low, market_cap, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(symbol, period) DO UPDATE SET
           snapshot_price = excluded.snapshot_price,
           change_pct = excluded.change_pct,
           high = excluded.high,
           low = excluded.low,
           market_cap = excluded.market_cap,
           fetched_at = excluded.fetched_at",
        rusqlite::params![
            sym,
            period.as_str(),
            snap.price,
            snap.change_pct,
            snap.high,
            snap.low,
            snap.market_cap,
            Utc::now().to_rfc3339(),
        ],
    )?;
    for pt in &snap.points {
        conn.execute(
            "INSERT INTO busha_ohlc_points (symbol, period, ts, price, ingested_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(symbol, period, ts) DO UPDATE SET price = excluded.price, ingested_at = datetime('now')",
            rusqlite::params![sym, period.as_str(), pt.ts.to_rfc3339(), pt.price],
        )?;
    }
    Ok(())
}

pub fn ohlc_points(
    conn: &Connection,
    symbol: &str,
    period: BushaOhlcPeriod,
    limit: i64,
) -> Result<Vec<(String, f64)>> {
    let sym = symbol.trim().to_uppercase();
    let mut stmt = conn.prepare(
        "SELECT ts, price FROM busha_ohlc_points
         WHERE symbol = ?1 AND period = ?2
         ORDER BY ts ASC LIMIT ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![sym, period.as_str(), limit], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn ohlc_meta(
    conn: &Connection,
    symbol: &str,
    period: BushaOhlcPeriod,
) -> Result<Option<(f64, f64, f64, f64, f64)>> {
    let sym = symbol.trim().to_uppercase();
    conn.query_row(
        "SELECT snapshot_price, change_pct, high, low, market_cap FROM busha_ohlc_meta
         WHERE symbol = ?1 AND period = ?2",
        rusqlite::params![sym, period.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )
    .optional()
    .map_err(Into::into)
}

pub fn has_ohlc_meta(conn: &Connection, symbol: &str, period: BushaOhlcPeriod) -> bool {
    let sym = symbol.trim().to_uppercase();
    conn.query_row(
        "SELECT 1 FROM busha_ohlc_meta WHERE symbol = ?1 AND period = ?2 LIMIT 1",
        rusqlite::params![sym, period.as_str()],
        |_| Ok(true),
    )
    .unwrap_or(false)
}

/// Merge Busha pair + OHLC meta into a quote object for symbol detail UI.
pub fn crypto_detail_quote(
    conn: &Connection,
    symbol: &str,
    fallback_price: Option<f64>,
) -> serde_json::Value {
    use serde_json::json;
    let sym = symbol.trim().to_uppercase();
    let mut price = fallback_price.filter(|p| p.is_finite() && *p > 0.0);
    let mut change: Option<f64> = None;
    let mut high: Option<f64> = None;
    let mut low: Option<f64> = None;
    let mut market_cap: Option<f64> = None;

    if let Ok(Some((snap, chg, h, l, mc))) =
        ohlc_meta(conn, &sym, BushaOhlcPeriod::OneDay)
    {
        if snap.is_finite() && snap > 0.0 {
            price = Some(snap);
        }
        if chg.is_finite() {
            change = Some(chg);
        }
        if h.is_finite() && h > 0.0 {
            high = Some(h);
        }
        if l.is_finite() && l > 0.0 {
            low = Some(l);
        }
        if mc.is_finite() && mc > 0.0 {
            market_cap = Some(mc);
        }
    }

    if price.is_none() {
        if let Ok(px) = conn.query_row(
            "SELECT buy_price FROM busha_pairs WHERE symbol = ?1",
            [&sym],
            |row| row.get::<_, f64>(0),
        ) {
            if px.is_finite() && px > 0.0 {
                price = Some(px);
            }
        }
    }

    if change.is_none() {
        change = conn
            .query_row(
                "SELECT change_percent FROM price_history WHERE symbol = ?1 ORDER BY trade_date DESC LIMIT 1",
                [&sym],
                |row| row.get::<_, Option<f64>>(0),
            )
            .ok()
            .flatten()
            .filter(|c| c.is_finite());
    }

    json!({
        "symbol": sym,
        "price": price,
        "changePercent": change,
        "high": high,
        "low": low,
        "marketCap": market_cap,
        "volume": serde_json::Value::Null,
        "peRatio": serde_json::Value::Null,
        "source": "busha",
    })
}

pub fn rollup_ohlc_to_daily_price_history(
    conn: &Connection,
    symbol: &str,
    period: BushaOhlcPeriod,
) -> Result<usize> {
    let sym = symbol.trim().to_uppercase();
    let points = ohlc_points(conn, &sym, period, 10_000)?;
    if points.is_empty() {
        return Ok(0);
    }
    let mut by_date: HashMap<NaiveDate, f64> = HashMap::new();
    for (ts, price) in points {
        let date = DateTime::parse_from_rfc3339(&ts)
            .map(|d| d.date_naive())
            .or_else(|_| NaiveDate::parse_from_str(&ts[..10.min(ts.len())], "%Y-%m-%d"))
            .unwrap_or_else(|_| Utc::now().date_naive());
        by_date.insert(date, price);
    }
    let mut dates: Vec<_> = by_date.keys().copied().collect();
    dates.sort();
    let mut written = 0usize;
    for (i, date) in dates.iter().enumerate() {
        let price = by_date[date];
        let change_pct = if i > 0 {
            let prev = by_date[&dates[i - 1]];
            if prev.abs() > f64::EPSILON {
                ((price - prev) / prev) * 100.0
            } else {
                0.0
            }
        } else {
            0.0
        };
        conn.execute(
            "INSERT INTO price_history (symbol, trade_date, price, change_percent, volume, ingested_at)
             VALUES (?1, ?2, ?3, ?4, 0, datetime('now'))
             ON CONFLICT(symbol, trade_date) DO UPDATE SET
               price = excluded.price,
               change_percent = excluded.change_percent,
               ingested_at = datetime('now')",
            rusqlite::params![sym, date.to_string(), price, change_pct],
        )?;
        written += 1;
    }
    Ok(written)
}

pub fn is_crypto_symbol(conn: &Connection, symbol: &str) -> bool {
    let sym = symbol.trim().to_uppercase();
    conn.query_row(
        "SELECT 1 FROM instruments WHERE symbol = ?1 AND sector = 'CRYPTO' LIMIT 1",
        [&sym],
        |_| Ok(true),
    )
    .unwrap_or(false)
        || conn
            .query_row(
                "SELECT 1 FROM busha_pairs WHERE symbol = ?1 LIMIT 1",
                [&sym],
                |_| Ok(true),
            )
            .unwrap_or(false)
}

pub fn symbols_needing_ohlc(
    conn: &Connection,
    held: &[String],
    universe_limit: usize,
) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for s in held {
        let u = s.trim().to_uppercase();
        if !u.is_empty() && seen.insert(u.clone()) {
            out.push(u);
        }
    }
    let mut stmt = conn.prepare(
        "SELECT i.symbol FROM instruments i
         LEFT JOIN busha_pairs bp ON bp.symbol = i.symbol
         WHERE i.is_active = 1 AND i.sector = 'CRYPTO'
         ORDER BY bp.symbol IS NOT NULL DESC, i.symbol ASC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([universe_limit as i64], |row| row.get::<_, String>(0))?;
    for row in rows.filter_map(|r| r.ok()) {
        let u = row.trim().to_uppercase();
        if !u.is_empty() && seen.insert(u.clone()) {
            out.push(u);
        }
    }
    Ok(out)
}

pub async fn ingest_ohlc_for_symbols(
    client: &BushaClient,
    db: &Database,
    symbols: &[String],
    period: BushaOhlcPeriod,
    max_age_secs: i64,
    always_refresh: &HashSet<String>,
) -> Result<OhlcIngestReport> {
    let mut report = OhlcIngestReport::default();
    for symbol in symbols {
        let sym = symbol.trim().to_uppercase();
        if sym.is_empty() {
            continue;
        }
        let force = always_refresh.contains(&sym);
        let fresh = db.with_conn(|conn| ohlc_meta_fresh(conn, &sym, period, max_age_secs))?;
        if fresh && !force {
            report.skipped_cache += 1;
            continue;
        }
        match client.ohlc_for_base(&sym, period).await {
            Ok(snap) => {
                if snap.points.is_empty() {
                    tracing::warn!(target: "busha", symbol = %sym, period = period.as_str(), "OHLC returned no points");
                }
                db.with_conn(|conn| {
                    persist_ohlc_snapshot(conn, &snap, period)?;
                    if matches!(period, BushaOhlcPeriod::OneMonth | BushaOhlcPeriod::OneYear | BushaOhlcPeriod::AllTime) {
                        let _ = rollup_ohlc_to_daily_price_history(conn, &sym, period);
                    }
                    Ok(())
                })?;
                report.fetched += 1;
            }
            Err(e) => {
                tracing::warn!(target: "busha", symbol = %sym, period = period.as_str(), error = %e, "OHLC fetch failed");
                report.failed.push((sym, e.to_string()));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    }
    Ok(report)
}

pub fn latest_ohlc_price(conn: &Connection, symbol: &str, period: BushaOhlcPeriod) -> Option<f64> {
    let sym = symbol.trim().to_uppercase();
    conn.query_row(
        "SELECT price FROM busha_ohlc_points
         WHERE symbol = ?1 AND period = ?2
         ORDER BY ts DESC LIMIT 1",
        rusqlite::params![sym, period.as_str()],
        |row| row.get(0),
    )
    .ok()
    .filter(|p: &f64| p.is_finite() && *p > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    const OHLC_FIXTURE: &str = r#"{
      "status": "success",
      "message": "OHLC fetched successfully",
      "data": {
        "symbol": "ARKMNGN",
        "change": "3.73",
        "high": "161.93",
        "low": "140.5",
        "market_cap": "0",
        "price": "147.49",
        "price_data": [
          { "time": "2026-08-21T10:40:00Z", "price": "142.12" },
          { "time": "2026-08-21T10:45:00Z", "price": "142.51" },
          { "time": "2026-08-22T10:40:00Z", "price": "147.53" }
        ]
      }
    }"#;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/001_initial.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/013_busha_pairs.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/014_busha_ohlc.sql"))
            .unwrap();
        conn
    }

    #[test]
    fn parse_ohlc_response_fixture() {
        let v: Value = serde_json::from_str(OHLC_FIXTURE).unwrap();
        let snap = parse_ohlc_response(&v, "ARKM").unwrap();
        assert_eq!(snap.base, "ARKM");
        assert_eq!(snap.pair_id, "ARKMNGN");
        assert!((snap.price - 147.49).abs() < 0.01);
        assert_eq!(snap.points.len(), 3);
        assert!(snap.points[0].ts < snap.points[2].ts);
    }

    #[test]
    fn persist_and_read_ohlc() {
        let conn = test_conn();
        let v: Value = serde_json::from_str(OHLC_FIXTURE).unwrap();
        let snap = parse_ohlc_response(&v, "ARKM").unwrap();
        persist_ohlc_snapshot(&conn, &snap, BushaOhlcPeriod::OneDay).unwrap();
        let pts = ohlc_points(&conn, "ARKM", BushaOhlcPeriod::OneDay, 100).unwrap();
        assert_eq!(pts.len(), 3);
        assert!(ohlc_meta_fresh(&conn, "ARKM", BushaOhlcPeriod::OneDay, 300).unwrap());
    }

    #[test]
    fn rollup_writes_price_history() {
        let conn = test_conn();
        let v: Value = serde_json::from_str(OHLC_FIXTURE).unwrap();
        let snap = parse_ohlc_response(&v, "ARKM").unwrap();
        persist_ohlc_snapshot(&conn, &snap, BushaOhlcPeriod::OneMonth).unwrap();
        let n = rollup_ohlc_to_daily_price_history(&conn, "ARKM", BushaOhlcPeriod::OneMonth).unwrap();
        assert_eq!(n, 2);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM price_history WHERE symbol = 'ARKM'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn ohlc_meta_stale_after_ttl() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO busha_ohlc_meta (symbol, period, snapshot_price, change_pct, high, low, market_cap, fetched_at)
             VALUES ('BTC', '1d', 100, 1, 110, 90, 0, '2020-01-01T00:00:00+00:00')",
            [],
        )
        .unwrap();
        assert!(!ohlc_meta_fresh(&conn, "BTC", BushaOhlcPeriod::OneDay, 300).unwrap());
    }
}
