use anyhow::Result;
use rusqlite::Connection;

use crate::busha::{is_crypto_symbol, ohlc_points, BushaOhlcPeriod};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TechnicalSnapshot {
    pub sma50: Option<f64>,
    pub sma200: Option<f64>,
    pub rsi14: Option<f64>,
    pub momentum: Option<f64>,
    pub volume_anomaly: Option<f64>,
    pub current_price: f64,
}

pub struct IndicatorService;

impl IndicatorService {
    pub fn compute(conn: &Connection, symbol: &str) -> Result<Option<TechnicalSnapshot>> {
        if is_crypto_symbol(conn, symbol) {
            return Self::compute_crypto(conn, symbol);
        }
        Self::compute_daily(conn, symbol)
    }

    fn compute_daily(conn: &Connection, symbol: &str) -> Result<Option<TechnicalSnapshot>> {
        let mut stmt = conn.prepare(
            "SELECT trade_date, price, volume FROM price_history
             WHERE symbol = ?1 ORDER BY trade_date DESC LIMIT 250",
        )?;
        let rows: Vec<(String, f64, i64)> = stmt
            .query_map([symbol], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .filter_map(|r| r.ok())
            .collect();

        if rows.len() < 60 {
            return Ok(None);
        }

        Self::compute_from_prices(rows, true, false)
    }

    fn compute_crypto(conn: &Connection, symbol: &str) -> Result<Option<TechnicalSnapshot>> {
        let points = ohlc_points(conn, symbol, BushaOhlcPeriod::OneDay, 500)?;
        if points.len() >= 15 {
            let rows: Vec<(String, f64, i64)> = points
                .into_iter()
                .map(|(ts, price)| (ts, price, 0_i64))
                .collect();
            return Self::compute_from_prices(rows, false, true);
        }

        // Fall back to rolled-up daily history when intraday OHLC is sparse.
        Self::compute_daily(conn, symbol)
    }

    fn compute_from_prices(
        rows: Vec<(String, f64, i64)>,
        include_volume: bool,
        chronological: bool,
    ) -> Result<Option<TechnicalSnapshot>> {
        let min_bars = if include_volume { 60 } else { 15 };
        if rows.len() < min_bars {
            return Ok(None);
        }

        let rows: Vec<_> = if chronological {
            rows
        } else {
            rows.into_iter().rev().collect()
        };
        let prices: Vec<f64> = rows.iter().map(|r| r.1).collect();
        let volumes: Vec<f64> = rows.iter().map(|r| r.2 as f64).collect();
        let current_price = *prices.last().unwrap();

        let sma50 = sma(&prices, 50);
        let sma200 = sma(&prices, 200);
        let rsi14 = rsi(&prices, 14);

        let momentum_window = 20;
        let momentum = if prices.len() > momentum_window {
            let prev = prices[prices.len() - momentum_window - 1];
            Some(((current_price - prev) / prev) * 100.0)
        } else {
            None
        };

        let volume_anomaly = if include_volume {
            let recent_volumes = &volumes[volumes.len().saturating_sub(20)..];
            let avg_volume = recent_volumes.iter().sum::<f64>() / recent_volumes.len() as f64;
            let current_volume = *volumes.last().unwrap();
            if avg_volume > 0.0 {
                Some(current_volume / avg_volume)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Some(TechnicalSnapshot {
            sma50,
            sma200,
            rsi14,
            momentum,
            volume_anomaly,
            current_price,
        }))
    }

    /// True when RSI14 is known and > 70. Missing history is not overbought (fail-open).
    pub fn is_overbought(conn: &Connection, symbol: &str) -> Result<bool> {
        Ok(Self::compute(conn, symbol)?
            .and_then(|t| t.rsi14)
            .map(|r| r > 70.0)
            .unwrap_or(false))
    }
}

fn sma(values: &[f64], period: usize) -> Option<f64> {
    if values.len() < period {
        return None;
    }
    let slice = &values[values.len() - period..];
    Some(slice.iter().sum::<f64>() / period as f64)
}

pub fn sma_export(values: &[f64], period: usize) -> Option<f64> {
    sma(values, period)
}

pub fn rsi_export(values: &[f64], period: usize) -> Option<f64> {
    rsi(values, period)
}

fn rsi(values: &[f64], period: usize) -> Option<f64> {
    if values.len() <= period {
        return None;
    }
    let mut gains = 0.0;
    let mut losses = 0.0;
    for i in (values.len() - period)..values.len() {
        let change = values[i] - values[i - 1];
        if change >= 0.0 {
            gains += change;
        } else {
            losses -= change;
        }
    }
    if losses == 0.0 {
        return Some(100.0);
    }
    let rs = gains / losses;
    Some(100.0 - (100.0 / (1.0 + rs)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::busha::{persist_ohlc_snapshot, parse_ohlc_response, BushaOhlcPeriod};
    use rusqlite::Connection;
    use serde_json::Value;

    const OHLC_UP: &str = r#"{
      "status":"success",
      "data":{
        "symbol":"TESTNGN","change":"1","high":"200","low":"50","market_cap":"0","price":"150",
        "price_data":[]
      }
    }"#;

    fn seed_crypto_ohlc(conn: &Connection, symbol: &str, n: usize, start: f64, step: f64) {
        let mut price_data = Vec::new();
        for i in 0..n {
            let price = start + step * i as f64;
            price_data.push(serde_json::json!({
                "time": format!("2026-08-21T10:{:02}:00Z", i % 60),
                "price": format!("{price:.2}")
            }));
        }
        let mut v: Value = serde_json::from_str(OHLC_UP).unwrap();
        v["data"]["symbol"] = serde_json::json!(format!("{symbol}NGN"));
        v["data"]["price_data"] = serde_json::Value::Array(price_data);
        let snap = parse_ohlc_response(&v, symbol).unwrap();
        persist_ohlc_snapshot(conn, &snap, BushaOhlcPeriod::OneDay).unwrap();
    }

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE instruments (symbol TEXT PRIMARY KEY, name TEXT, sector TEXT, is_active INTEGER DEFAULT 1);
             CREATE TABLE price_history (symbol TEXT, trade_date TEXT, price REAL, volume INTEGER);
             CREATE TABLE busha_pairs (symbol TEXT PRIMARY KEY, pair_id TEXT);
             CREATE TABLE busha_ohlc_points (symbol TEXT, period TEXT, ts TEXT, price REAL, ingested_at TEXT, UNIQUE(symbol, period, ts));
             CREATE TABLE busha_ohlc_meta (symbol TEXT, period TEXT, snapshot_price REAL, change_pct REAL, high REAL, low REAL, market_cap REAL, fetched_at TEXT, PRIMARY KEY(symbol, period));",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO instruments (symbol, name, sector) VALUES ('TEST', 'TEST', 'CRYPTO')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn rsi_overbought_threshold() {
        let up: Vec<f64> = (0..20).map(|i| 50.0 + i as f64 * 5.0).collect();
        assert!(rsi_export(&up, 14).unwrap() > 70.0);
        let down: Vec<f64> = (0..20).map(|i| 150.0 - i as f64 * 5.0).collect();
        assert!(rsi_export(&down, 14).unwrap() < 30.0);
    }

    #[test]
    fn missing_history_is_not_overbought() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE price_history (symbol TEXT, trade_date TEXT, price REAL, volume INTEGER);
             CREATE TABLE instruments (symbol TEXT PRIMARY KEY, name TEXT, sector TEXT);
             CREATE TABLE busha_ohlc_points (symbol TEXT, period TEXT, ts TEXT, price REAL, ingested_at TEXT);
             CREATE TABLE busha_ohlc_meta (symbol TEXT, period TEXT, snapshot_price REAL, change_pct REAL, high REAL, low REAL, market_cap REAL, fetched_at TEXT);
             CREATE TABLE busha_pairs (symbol TEXT PRIMARY KEY, pair_id TEXT);",
        )
        .unwrap();
        assert!(!IndicatorService::is_overbought(&conn, "GTCO").unwrap());
    }

    #[test]
    fn crypto_ohlc_enables_compute() {
        let conn = test_conn();
        seed_crypto_ohlc(&conn, "TEST", 20, 100.0, 5.0);
        let snap = IndicatorService::compute(&conn, "TEST").unwrap();
        assert!(snap.is_some());
        assert!(snap.unwrap().rsi14.is_some());
    }

    #[test]
    fn crypto_sparse_ohlc_returns_none() {
        let conn = test_conn();
        seed_crypto_ohlc(&conn, "TEST", 5, 100.0, 1.0);
        assert!(IndicatorService::compute(&conn, "TEST").unwrap().is_none());
    }

    #[test]
    fn crypto_overbought_on_uptrend() {
        let conn = test_conn();
        seed_crypto_ohlc(&conn, "TEST", 30, 100.0, 5.0);
        let snap = IndicatorService::compute(&conn, "TEST").unwrap().expect("technical");
        assert!(snap.rsi14.unwrap_or(0.0) > 70.0);
        assert!(IndicatorService::is_overbought(&conn, "TEST").unwrap());
    }
}
