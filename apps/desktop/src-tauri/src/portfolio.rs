use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::cache::PriceCache;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioSummary {
    pub portfolio: serde_json::Value,
    pub positions: Vec<serde_json::Value>,
    pub total_equity: f64,
    pub market_value: f64,
    pub pnl_today: f64,
}

pub struct PortfolioService;

impl PortfolioService {
    pub fn get_portfolio(conn: &Connection, cache: &PriceCache, id: &str) -> Result<Option<PortfolioSummary>> {
        let portfolio: Option<(String, String, f64, f64, String, String)> = conn
            .query_row(
                "SELECT id, name, starting_capital, cash_balance, created_at, strategy_param_set_id FROM sandbox_portfolios WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .ok();

        let Some(p) = portfolio else {
            return Ok(None);
        };

        let mut stmt = conn.prepare(
            "SELECT id, symbol, quantity, avg_cost FROM sandbox_positions WHERE portfolio_id = ?1",
        )?;
        let positions_raw: Vec<(String, String, f64, f64)> = stmt
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let mut market_value = 0.0;
        let mut enriched = vec![];
        for (pid, symbol, quantity, avg_cost) in positions_raw {
            let price = Self::get_price(conn, cache, &symbol);
            let value = quantity * price;
            market_value += value;
            enriched.push(serde_json::json!({
                "id": pid,
                "symbol": symbol,
                "quantity": quantity,
                "avg_cost": avg_cost,
                "current_price": price,
                "market_value": value,
            }));
        }

        let total_equity = p.3 + market_value;
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let pnl_today: f64 = conn
            .query_row(
                "SELECT pnl_daily FROM daily_performance_snapshot WHERE portfolio_id = ?1 AND snapshot_date = ?2",
                rusqlite::params![id, today],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        Ok(Some(PortfolioSummary {
            portfolio: serde_json::json!({
                "id": p.0,
                "name": p.1,
                "starting_capital": p.2,
                "cash_balance": p.3,
                "created_at": p.4,
                "strategy_param_set_id": p.5,
            }),
            positions: enriched,
            total_equity,
            market_value,
            pnl_today,
        }))
    }

    pub fn get_default_portfolio_id(conn: &Connection) -> Result<Option<String>> {
        conn.query_row(
            "SELECT id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_performance(conn: &Connection, id: &str) -> Result<Vec<serde_json::Value>> {
        let mut stmt = conn.prepare(
            "SELECT snapshot_date, total_equity, pnl_daily, pnl_cumulative, benchmark_asi_change_pct, drawdown_pct
             FROM daily_performance_snapshot WHERE portfolio_id = ?1 ORDER BY snapshot_date",
        )?;
        let rows = stmt.query_map([id], |row| {
            Ok(serde_json::json!({
                "snapshot_date": row.get::<_, String>(0)?,
                "total_equity": row.get::<_, f64>(1)?,
                "pnl_daily": row.get::<_, f64>(2)?,
                "pnl_cumulative": row.get::<_, f64>(3)?,
                "benchmark_asi_change_pct": row.get::<_, f64>(4)?,
                "drawdown_pct": row.get::<_, f64>(5)?,
            }))
        })?;
        rows.filter_map(|r| r.ok()).collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn get_price(conn: &Connection, cache: &PriceCache, symbol: &str) -> f64 {
        if let Some(cached) = cache.get_price(symbol) {
            return cached.price;
        }
        conn.query_row(
            "SELECT price FROM price_history WHERE symbol = ?1 ORDER BY trade_date DESC LIMIT 1",
            [symbol],
            |row| row.get(0),
        )
        .unwrap_or(0.0)
    }
}

use rusqlite::OptionalExtension;

pub struct DailySnapshotService;

impl DailySnapshotService {
    pub fn create_snapshot(conn: &Connection, cache: &PriceCache, portfolio_id: Option<&str>) -> Result<()> {
        let portfolios: Vec<(String, f64, f64)> = if let Some(id) = portfolio_id {
            vec![conn.query_row(
                "SELECT id, cash_balance, starting_capital FROM sandbox_portfolios WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?]
        } else {
            let mut stmt = conn.prepare("SELECT id, cash_balance, starting_capital FROM sandbox_portfolios")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .filter_map(|r| r.ok())
                .collect()
        };

        for (pid, cash_balance, starting_capital) in portfolios {
            Self::snapshot_one(conn, cache, &pid, cash_balance, starting_capital)?;
        }
        Ok(())
    }

    fn snapshot_one(
        conn: &Connection,
        cache: &PriceCache,
        portfolio_id: &str,
        cash_balance: f64,
        starting_capital: f64,
    ) -> Result<()> {
        let mut stmt = conn.prepare("SELECT symbol, quantity FROM sandbox_positions WHERE portfolio_id = ?1")?;
        let positions: Vec<(String, f64)> = stmt
            .query_map([portfolio_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let mut market_value = 0.0;
        for (symbol, qty) in &positions {
            let price = PortfolioService::get_price(conn, cache, symbol);
            market_value += qty * price;
        }

        let total_equity = cash_balance + market_value;
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let prev_equity: f64 = conn
            .query_row(
                "SELECT total_equity FROM daily_performance_snapshot WHERE portfolio_id = ?1 ORDER BY snapshot_date DESC LIMIT 1",
                [portfolio_id],
                |row| row.get(0),
            )
            .unwrap_or(starting_capital);

        let pnl_daily = total_equity - prev_equity;
        let pnl_cumulative = total_equity - starting_capital;

        let peak: f64 = conn
            .query_row(
                "SELECT MAX(total_equity) FROM daily_performance_snapshot WHERE portfolio_id = ?1",
                [portfolio_id],
                |row| row.get(0),
            )
            .unwrap_or(starting_capital)
            .max(total_equity);

        let drawdown_pct = if peak > 0.0 { (peak - total_equity) / peak } else { 0.0 };

        let benchmark_change: f64 = conn
            .query_row(
                "SELECT value FROM index_history WHERE index_code = 'ASI' ORDER BY trade_date DESC LIMIT 2",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        conn.execute(
            "INSERT INTO daily_performance_snapshot (portfolio_id, snapshot_date, total_equity, pnl_daily, pnl_cumulative, benchmark_asi_change_pct, drawdown_pct)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(portfolio_id, snapshot_date) DO UPDATE SET
               total_equity = excluded.total_equity, pnl_daily = excluded.pnl_daily,
               pnl_cumulative = excluded.pnl_cumulative, drawdown_pct = excluded.drawdown_pct",
            rusqlite::params![portfolio_id, today, total_equity, pnl_daily, pnl_cumulative, benchmark_change, drawdown_pct],
        )?;
        Ok(())
    }
}
