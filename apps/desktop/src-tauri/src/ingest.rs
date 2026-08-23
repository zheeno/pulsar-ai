use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;

use crate::cache::{CachedPrice, PriceCache};
use crate::calendar::TradingCalendar;
use crate::db::Database;
use crate::ngx::{LatestQuote, NgxPulseClient, NgxStock};

pub struct IngestionService;

impl IngestionService {
    pub async fn ingest_stocks(
        db: &Database,
        client: &NgxPulseClient,
        cache: &PriceCache,
        calendar: &TradingCalendar,
        force: bool,
    ) -> Result<usize> {
        if !force
            && crate::runtime_util::enforce_market_hours()
            && !calendar.is_market_open()
            && !Self::is_post_close(calendar)
        {
            return Ok(0);
        }
        let stocks = client.get_stocks().await?;
        let trade_date = calendar.today_wat();
        db.with_conn(|conn| {
            let mut count = 0;
            for stock in &stocks {
                Self::upsert_stock(conn, cache, stock, &trade_date)?;
                count += 1;
            }
            tracing::info!(target: "ngx_pulse", count, "upserted pulse stocks");
            Ok(count)
        })
    }

    pub async fn ingest_market(
        db: &Database,
        client: &NgxPulseClient,
        calendar: &TradingCalendar,
        force: bool,
    ) -> Result<()> {
        if !force && crate::runtime_util::enforce_market_hours() && !calendar.is_trading_day(None) {
            return Ok(());
        }
        let market = match client.get_market().await {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        let trade_date = calendar.today_wat();
        db.with_conn(|conn| {
            if let Some(asi) = market.asi {
                conn.execute(
                    "INSERT INTO index_history (index_code, trade_date, value, points)
                     VALUES ('ASI', ?1, ?2, ?3)
                     ON CONFLICT(index_code, trade_date) DO UPDATE SET value = excluded.value",
                    rusqlite::params![trade_date, asi.value, asi.change_percent.unwrap_or(0.0)],
                )?;
            }
            Ok(())
        })
    }

    pub async fn ingest_indices(
        db: &Database,
        client: &NgxPulseClient,
        calendar: &TradingCalendar,
        force: bool,
    ) -> Result<()> {
        if !force && crate::runtime_util::enforce_market_hours() && !calendar.is_trading_day(None) {
            return Ok(());
        }
        let indices = match client.get_indices().await {
            Ok(i) => i,
            Err(_) => return Ok(()),
        };
        let trade_date = calendar.today_wat();
        db.with_conn(|conn| {
            for idx in &indices {
                conn.execute(
                    "INSERT INTO index_history (index_code, trade_date, value, points, week_change, month_change, year_change)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(index_code, trade_date) DO UPDATE SET
                        value = excluded.value,
                        points = excluded.points,
                        week_change = excluded.week_change,
                        month_change = excluded.month_change,
                        year_change = excluded.year_change",
                    rusqlite::params![
                        idx.code,
                        trade_date,
                        idx.value,
                        idx.points,
                        idx.week_change,
                        idx.month_change,
                        idx.year_change
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub async fn backfill_symbols(
        conn: &Connection,
        client: &NgxPulseClient,
        calendar: &TradingCalendar,
        limit: usize,
    ) -> Result<usize> {
        if calendar.is_trading_day(None) {
            return Ok(0);
        }
        let mut stmt = conn.prepare(
            "SELECT i.symbol FROM instruments i
             LEFT JOIN backfill_state b ON i.symbol = b.symbol
             WHERE i.is_active = 1
             ORDER BY b.last_run_at IS NULL DESC, b.last_run_at ASC
             LIMIT ?1",
        )?;
        let symbols: Vec<String> = stmt
            .query_map([limit as i64], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut count = 0;
        let to = Utc::now().format("%Y-%m-%d").to_string();
        let from = (Utc::now() - chrono::Duration::days(365)).format("%Y-%m-%d").to_string();

        for symbol in symbols {
            let history = match client.get_symbol_price(&symbol, Some(&from), Some(&to)).await {
                Ok(h) => h,
                Err(_) => continue,
            };
            for h in &history {
                conn.execute(
                    "INSERT INTO price_history (symbol, trade_date, price, volume)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(symbol, trade_date) DO UPDATE SET price = excluded.price",
                    rusqlite::params![symbol, h.date, h.price, h.volume.unwrap_or(0)],
                )?;
            }
            let earliest = history.first().map(|h| h.date.as_str());
            conn.execute(
                "INSERT INTO backfill_state (symbol, earliest_date_fetched, last_run_at)
                 VALUES (?1, ?2, datetime('now'))
                 ON CONFLICT(symbol) DO UPDATE SET last_run_at = datetime('now')",
                rusqlite::params![symbol, earliest],
            )?;
            count += 1;
        }
        Ok(count)
    }

    fn upsert_stock(conn: &Connection, cache: &PriceCache, stock: &NgxStock, trade_date: &str) -> Result<()> {
        conn.execute(
            "INSERT INTO instruments (symbol, name, sector, is_active)
             VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(symbol) DO UPDATE SET name = COALESCE(excluded.name, instruments.name)",
            rusqlite::params![
                stock.symbol,
                stock.name.as_deref().unwrap_or(&stock.symbol),
                stock.sector.as_deref().unwrap_or("Unknown")
            ],
        )?;
        conn.execute(
            "INSERT INTO price_history (symbol, trade_date, price, change_percent, volume, market_cap, pe_ratio, ingested_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
             ON CONFLICT(symbol, trade_date) DO UPDATE SET
               price = excluded.price, change_percent = excluded.change_percent,
               volume = excluded.volume, ingested_at = datetime('now')",
            rusqlite::params![
                stock.symbol,
                trade_date,
                stock.price,
                stock.change_percent.unwrap_or(0.0),
                stock.volume.unwrap_or(0),
                stock.market_cap,
                stock.pe_ratio
            ],
        )?;
        cache.set_price(&CachedPrice {
            symbol: stock.symbol.clone(),
            price: stock.price,
            trade_date: trade_date.into(),
            updated_at: Utc::now().to_rfc3339(),
        });
        Ok(())
    }

    pub fn upsert_last_quote(conn: &Connection, cache: &PriceCache, quote: &LatestQuote) -> Result<()> {
        conn.execute(
            "INSERT INTO instruments (symbol, name, sector, is_active)
             VALUES (?1, ?1, 'Unknown', 1)
             ON CONFLICT(symbol) DO NOTHING",
            [&quote.symbol],
        )?;
        let change_pct = quote.prev_close.filter(|p| *p > 0.0).map(|p| (quote.last - p) / p * 100.0);
        conn.execute(
            "INSERT INTO price_history (symbol, trade_date, price, change_percent, ingested_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(symbol, trade_date) DO UPDATE SET
               price = excluded.price, change_percent = excluded.change_percent, ingested_at = datetime('now')",
            rusqlite::params![
                quote.symbol,
                quote.trade_date,
                quote.last,
                change_pct.unwrap_or(0.0)
            ],
        )?;
        cache.set_price(&CachedPrice {
            symbol: quote.symbol.clone(),
            price: quote.last,
            trade_date: quote.trade_date.clone(),
            updated_at: Utc::now().to_rfc3339(),
        });
        Ok(())
    }

    fn is_post_close(calendar: &TradingCalendar) -> bool {
        calendar.is_post_close_window()
    }
}
