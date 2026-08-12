use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

        let rows: Vec<_> = rows.into_iter().rev().collect();
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

        let recent_volumes = &volumes[volumes.len().saturating_sub(20)..];
        let avg_volume = recent_volumes.iter().sum::<f64>() / recent_volumes.len() as f64;
        let current_volume = *volumes.last().unwrap();
        let volume_anomaly = if avg_volume > 0.0 {
            Some(current_volume / avg_volume)
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
