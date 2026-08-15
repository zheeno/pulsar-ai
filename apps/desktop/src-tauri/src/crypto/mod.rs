use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::Deserialize;

use crate::cache::{CachedPrice, PriceCache};
use crate::db::Database;
use crate::http_client::http_client;
use crate::seed::CRYPTO_UNIVERSE;

const BINANCE_TICKER_URL: &str = "https://api.binance.com/api/v3/ticker/24hr";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceTicker {
    symbol: String,
    last_price: String,
    price_change_percent: String,
    volume: String,
}

pub async fn fetch_usdt_tickers() -> Result<Vec<(String, f64, f64, f64)>> {
    crate::net_policy::validate_public_feed_url(BINANCE_TICKER_URL)?;
    let symbols: Vec<String> = CRYPTO_UNIVERSE.iter().map(|(s, _)| (*s).to_string()).collect();
    let encoded = serde_json::to_string(&symbols)?;
    let http = http_client()?;
    let res = http
        .get(BINANCE_TICKER_URL)
        .query(&[("symbols", encoded.as_str())])
        .header("Accept", "application/json")
        .send()
        .await
        .context("binance ticker request")?;
    if !res.status().is_success() {
        anyhow::bail!("binance ticker HTTP {}", res.status());
    }
    let rows: Vec<BinanceTicker> = res.json().await.context("parse binance ticker")?;
    let mut out = Vec::new();
    for row in rows {
        let px: f64 = row.last_price.parse().unwrap_or(0.0);
        if px <= 0.0 {
            continue;
        }
        let chg: f64 = row.price_change_percent.parse().unwrap_or(0.0);
        let vol: f64 = row.volume.parse().unwrap_or(0.0);
        out.push((row.symbol, px, chg, vol));
    }
    if out.is_empty() {
        return Err(anyhow!("binance returned no usable tickers"));
    }
    Ok(out)
}

pub async fn ingest_crypto(db: &Database, cache: &PriceCache) -> Result<usize> {
    let tickers = fetch_usdt_tickers().await?;
    let trade_date = Utc::now().format("%Y-%m-%d").to_string();
    db.with_conn(|conn| {
        crate::seed::SeedService::ensure_crypto_sandbox(conn)?;
        let mut n = 0usize;
        for (symbol, price, change_pct, volume) in &tickers {
            let name = CRYPTO_UNIVERSE
                .iter()
                .find(|(s, _)| *s == symbol.as_str())
                .map(|(_, n)| *n)
                .unwrap_or(symbol.as_str());
            conn.execute(
                "INSERT INTO instruments (symbol, name, sector, is_active, module)
                 VALUES (?1, ?2, 'Crypto', 1, 'crypto')
                 ON CONFLICT(symbol) DO UPDATE SET name = excluded.name, module = 'crypto'",
                rusqlite::params![symbol, name],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price, change_percent, volume, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
                 ON CONFLICT(symbol, trade_date) DO UPDATE SET
                   price = excluded.price, change_percent = excluded.change_percent,
                   volume = excluded.volume, ingested_at = datetime('now')",
                rusqlite::params![symbol, trade_date, price, change_pct, *volume as i64],
            )?;
            cache.set_price(&CachedPrice {
                symbol: symbol.clone(),
                price: *price,
                trade_date: trade_date.clone(),
                updated_at: Utc::now().to_rfc3339(),
            });
            n += 1;
        }
        Ok(n)
    })
}
