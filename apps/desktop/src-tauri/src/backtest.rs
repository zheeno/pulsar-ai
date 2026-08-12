use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::AgentBridge;
use crate::execution::{FillSimulator, ParamSet, RiskPolicyService, SignalInput};
use crate::settings::AppSettings;

const PROMPT_VERSION: &str = "v1.0.0";

pub struct BacktestService;

impl BacktestService {
    pub fn start_run(
        conn: &Connection,
        strategy_param_set_id: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<String> {
        let run_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO backtest_runs (id, strategy_param_set_id, start_date, end_date, status)
             VALUES (?1, ?2, ?3, ?4, 'running')",
            rusqlite::params![run_id, strategy_param_set_id, start_date, end_date],
        )?;
        Ok(run_id)
    }

    pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<Value>> {
        conn.query_row(
            "SELECT id, strategy_param_set_id, start_date, end_date, status, results, created_at, completed_at FROM backtest_runs WHERE id = ?1",
            [run_id],
            |row| {
                let results: Option<String> = row.get(5)?;
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "strategy_param_set_id": row.get::<_, String>(1)?,
                    "start_date": row.get::<_, String>(2)?,
                    "end_date": row.get::<_, String>(3)?,
                    "status": row.get::<_, String>(4)?,
                    "results": results.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                    "created_at": row.get::<_, String>(6)?,
                    "completed_at": row.get::<_, Option<String>>(7)?,
                }))
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub async fn run_async(
        conn: &Connection,
        agent: &AgentBridge,
        settings: &AppSettings,
        run_id: &str,
        param_set_id: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<()> {
        let result = Self::execute_backtest(conn, agent, settings, param_set_id, start_date, end_date).await;
        match result {
            Ok(results) => {
                conn.execute(
                    "UPDATE backtest_runs SET status = 'completed', results = ?1, completed_at = datetime('now') WHERE id = ?2",
                    rusqlite::params![serde_json::to_string(&results)?, run_id],
                )?;
            }
            Err(e) => {
                conn.execute(
                    "UPDATE backtest_runs SET status = 'failed', results = ?1, completed_at = datetime('now') WHERE id = ?2",
                    rusqlite::params![json!({ "error": e.to_string() }).to_string(), run_id],
                )?;
            }
        }
        Ok(())
    }

    async fn execute_backtest(
        conn: &Connection,
        agent: &AgentBridge,
        settings: &AppSettings,
        param_set_id: &str,
        start_date: &str,
        end_date: &str,
    ) -> Result<Value> {
        let param_set = load_param_set(conn, param_set_id)?;
        let symbols = resolve_symbols(conn, &param_set)?;

        let mut cash = 10_000_000.0;
        let mut positions: std::collections::HashMap<String, (f64, f64)> = std::collections::HashMap::new();
        let mut equity_curve = vec![];
        let mut trades = 0i64;
        let mut wins = 0i64;
        let fill_sim = FillSimulator::from_settings(settings);

        let mut stmt = conn.prepare(
            "SELECT DISTINCT trade_date FROM price_history WHERE trade_date BETWEEN ?1 AND ?2 ORDER BY trade_date",
        )?;
        let dates: Vec<String> = stmt
            .query_map(rusqlite::params![start_date, end_date], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        for date in dates {
            let mut prices = std::collections::HashMap::new();
            for symbol in &symbols {
                let price: Option<f64> = conn
                    .query_row(
                        "SELECT price FROM price_history WHERE symbol = ?1 AND trade_date <= ?2 ORDER BY trade_date DESC LIMIT 1",
                        rusqlite::params![symbol, date],
                        |row| row.get(0),
                    )
                    .ok();
                if let Some(p) = price {
                    prices.insert(symbol.clone(), p);
                }
            }

            for symbol in &symbols {
                if !prices.contains_key(symbol) {
                    continue;
                }
                let technical = get_technical_at_date(conn, symbol, &date)?;
                let Some(technical) = technical else { continue };

                let context = json!({ "symbol": symbol, "technical": technical, "date": date });
                let output = if let Some(cached) = get_cached_llm(conn, symbol, &date)? {
                    cached
                } else {
                    let result = agent.symbol_signal(settings, context).await?;
                    result.get("output").cloned().unwrap_or(result)
                };

                let action = output.get("action").and_then(|v| v.as_str()).unwrap_or("HOLD");
                if action == "HOLD" {
                    continue;
                }
                let confidence = output.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);

                let pos_list: Vec<(String, f64, f64)> = positions
                    .iter()
                    .map(|(s, (q, c))| (s.clone(), *q, *c))
                    .collect();
                let market_value: f64 = pos_list
                    .iter()
                    .map(|(s, q, c)| q * prices.get(s).copied().unwrap_or(*c))
                    .sum();
                let total_equity = cash + market_value;

                let input = SignalInput {
                    id: "backtest".into(),
                    symbol: symbol.clone(),
                    action: action.into(),
                    confidence,
                };
                let (result, quantity) = RiskPolicyService::evaluate(
                    &input,
                    &param_set,
                    cash,
                    &pos_list,
                    &prices,
                    trades,
                    0.0,
                    fill_sim.fee_pct,
                );

                if result != "APPROVED" || quantity <= 0.0 {
                    continue;
                }

                let price = prices[symbol];
                if action == "BUY" {
                    let (fill_price, _) = fill_sim.simulate_fill("BUY", price);
                    let fee = fill_sim.calculate_fee(fill_price * quantity);
                    let cost = fill_price * quantity + fee;
                    if cash < cost {
                        continue;
                    }
                    cash -= cost;
                    let entry = positions.entry(symbol.clone()).or_insert((0.0, fill_price));
                    let new_qty = entry.0 + quantity;
                    entry.1 = (entry.1 * entry.0 + fill_price * quantity) / new_qty;
                    entry.0 = new_qty;
                    trades += 1;
                } else {
                    let entry = positions.get_mut(symbol).context("no position")?;
                    if entry.0 <= 0.0 {
                        continue;
                    }
                    let (fill_price, _) = fill_sim.simulate_fill("SELL", price);
                    let fee = fill_sim.calculate_fee(fill_price * quantity);
                    let pnl = (fill_price - entry.1) * quantity - fee;
                    if pnl > 0.0 {
                        wins += 1;
                    }
                    cash += fill_price * quantity - fee;
                    entry.0 -= quantity;
                    if entry.0 <= 0.0 {
                        positions.remove(symbol);
                    }
                    trades += 1;
                }
            }

            let market_value: f64 = positions
                .iter()
                .map(|(s, (q, c))| q * prices.get(s).copied().unwrap_or(*c))
                .sum();
            equity_curve.push(json!({ "date": date, "equity": cash + market_value }));
        }

        let final_equity = equity_curve.last().and_then(|e| e.get("equity").and_then(|v| v.as_f64())).unwrap_or(cash);
        Ok(json!({
            "final_equity": final_equity,
            "total_trades": trades,
            "win_rate": if trades > 0 { wins as f64 / trades as f64 } else { 0.0 },
            "equity_curve": equity_curve,
        }))
    }
}

use anyhow::Context;
use rusqlite::OptionalExtension;

fn load_param_set(conn: &Connection, id: &str) -> Result<ParamSet> {
    conn.query_row(
        "SELECT id, max_position_pct, max_daily_trades, stop_loss_pct, min_confidence_to_trade, max_daily_drawdown_pct, position_size_pct FROM strategy_param_sets WHERE id = ?1",
        [id],
        |row| {
            Ok(ParamSet {
                id: row.get(0)?,
                max_position_pct: row.get(1)?,
                max_daily_trades: row.get(2)?,
                stop_loss_pct: row.get(3)?,
                min_confidence_to_trade: row.get(4)?,
                max_daily_drawdown_pct: row.get(5)?,
                position_size_pct: row.get(6)?,
            })
        },
    )
    .map_err(Into::into)
}

fn resolve_symbols(conn: &Connection, _param_set: &ParamSet) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT symbol FROM instruments WHERE is_active = 1")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn get_technical_at_date(conn: &Connection, symbol: &str, date: &str) -> Result<Option<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT trade_date, price, volume FROM price_history WHERE symbol = ?1 AND trade_date <= ?2 ORDER BY trade_date DESC LIMIT 250",
    )?;
    let rows: Vec<(String, f64, i64)> = stmt
        .query_map(rusqlite::params![symbol, date], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();

    if rows.len() < 60 {
        return Ok(None);
    }

    let rows: Vec<_> = rows.into_iter().rev().collect();
    let prices: Vec<f64> = rows.iter().map(|r| r.1).collect();
    let current_price = *prices.last().unwrap();

    let sma50 = crate::indicators::sma_export(&prices, 50);
    let rsi14 = crate::indicators::rsi_export(&prices, 14);

    Ok(Some(json!({
        "sma50": sma50,
        "rsi14": rsi14,
        "currentPrice": current_price,
        "momentum": if prices.len() > 20 {
            Some(((current_price - prices[prices.len() - 21]) / prices[prices.len() - 21]) * 100.0)
        } else { None::<f64> },
    })))
}

fn get_cached_llm(conn: &Connection, symbol: &str, date: &str) -> Result<Option<Value>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT sll.raw_response FROM signals s
             JOIN signal_llm_logs sll ON sll.signal_id = s.id
             WHERE s.symbol = ?1 AND date(s.generated_at) = ?2 AND s.prompt_version = ?3 LIMIT 1",
            rusqlite::params![symbol, date, PROMPT_VERSION],
            |row| row.get(0),
        )
        .ok();
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
}
