use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::AgentBridge;
use crate::execution::ExecutionService;
use crate::indicators::IndicatorService;
use crate::settings::AppSettings;

const PORTFOLIO_PROMPT_VERSION: &str = "v2.0.0";
const PROMPT_VERSION: &str = "v1.0.0";

pub struct SignalGenerationService;

impl SignalGenerationService {
    pub async fn generate_for_portfolio(
        conn: &Connection,
        agent: &AgentBridge,
        settings: &AppSettings,
        portfolio_id: Option<&str>,
    ) -> Result<Vec<String>> {
        let param_set = Self::get_active_param_set(conn, portfolio_id)?;
        let Some(param_set) = param_set else {
            return Ok(vec![]);
        };

        let universe = Self::build_universe(conn, &param_set)?;
        if universe.is_empty() {
            return Ok(vec![]);
        }

        let positions = Self::get_positions(conn, portfolio_id)?;
        let market_context = Self::get_market_context(conn)?;
        let max_picks = param_set.max_daily_trades;

        let context = json!({
            "universe": universe,
            "positions": positions,
            "marketContext": market_context,
            "maxPicks": max_picks,
        });

        let result = agent.portfolio_signals(settings, context).await?;
        let signals = result
            .get("output")
            .and_then(|o| o.get("signals"))
            .or_else(|| result.get("signals"))
            .cloned()
            .unwrap_or(json!([]));

        let prompt = result.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let raw_response = result.get("rawResponse").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let model_name = result.get("modelName").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();

        let valid_symbols: std::collections::HashSet<String> = universe
            .iter()
            .filter_map(|u| u.get("symbol").and_then(|s| s.as_str()).map(String::from))
            .collect();

        let mut signal_ids = vec![];
        let mut seen = std::collections::HashSet::new();

        if let Some(arr) = signals.as_array() {
            for pick in arr {
                let symbol = pick.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
                if symbol.is_empty() || seen.contains(symbol) || !valid_symbols.contains(symbol) {
                    continue;
                }
                seen.insert(symbol.to_string());

                let signal_id = Self::persist_signal(
                    conn,
                    pick,
                    symbol,
                    &prompt,
                    &raw_response,
                    &model_name,
                    PORTFOLIO_PROMPT_VERSION,
                )?;
                if let Some(id) = signal_id {
                    signal_ids.push(id);
                }
            }
        }

        Ok(signal_ids)
    }

    pub async fn generate_for_symbol(
        conn: &Connection,
        agent: &AgentBridge,
        settings: &AppSettings,
        symbol: &str,
    ) -> Result<Option<String>> {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM instruments WHERE symbol = ?1 AND is_active = 1",
                [symbol],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !exists {
            return Ok(None);
        }

        let technical = IndicatorService::compute(conn, symbol)?;
        let Some(technical) = technical else {
            return Ok(None);
        };

        let context = json!({ "symbol": symbol, "technical": technical });
        let result = agent.symbol_signal(settings, context).await?;
        let output = result.get("output").cloned().unwrap_or(result.clone());
        let prompt = result.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let raw_response = result.get("rawResponse").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let model_name = result.get("modelName").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();

        Self::persist_signal(conn, &output, symbol, &prompt, &raw_response, &model_name, PROMPT_VERSION)
    }

    fn persist_signal(
        conn: &Connection,
        pick: &Value,
        symbol: &str,
        prompt: &str,
        raw_response: &str,
        model_name: &str,
        prompt_version: &str,
    ) -> Result<Option<String>> {
        let action = pick.get("action").and_then(|v| v.as_str()).unwrap_or("HOLD");
        let confidence = pick.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rationale = pick.get("rationale").and_then(|v| v.as_str()).unwrap_or("");

        if symbol.is_empty() {
            return Ok(None);
        }

        let signal_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result)
             VALUES (?1, ?2, ?3, ?4, ?5, '{}', ?6, ?7, 'BLOCKED_OTHER')",
            rusqlite::params![signal_id, symbol, action, confidence, rationale, model_name, prompt_version],
        )?;

        let log_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO signal_llm_logs (id, signal_id, prompt, raw_response) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![log_id, signal_id, prompt, raw_response],
        )?;

        Ok(Some(signal_id))
    }

    fn get_active_param_set(conn: &Connection, portfolio_id: Option<&str>) -> Result<Option<ParamSetRow>> {
        if let Some(pid) = portfolio_id {
            let row: Option<ParamSetRow> = conn
                .query_row(
                    "SELECT s.id, s.max_daily_trades FROM strategy_param_sets s
                     JOIN sandbox_portfolios p ON p.strategy_param_set_id = s.id
                     WHERE p.id = ?1 AND s.is_active = 1 LIMIT 1",
                    [pid],
                    |row| {
                        Ok(ParamSetRow {
                            id: row.get(0)?,
                            max_daily_trades: row.get(1)?,
                        })
                    },
                )
                .ok();
            if row.is_some() {
                return Ok(row);
            }
        }

        conn.query_row(
            "SELECT id, max_daily_trades FROM strategy_param_sets WHERE is_active = 1 LIMIT 1",
            [],
            |row| {
                Ok(ParamSetRow {
                    id: row.get(0)?,
                    max_daily_trades: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    fn build_universe(conn: &Connection, _param_set: &ParamSetRow) -> Result<Vec<Value>> {
        let sql = "SELECT i.symbol, i.name, i.sector, ph.price, ph.change_percent, ph.volume
             FROM instruments i
             LEFT JOIN price_history ph ON ph.symbol = i.symbol AND ph.trade_date = (
               SELECT MAX(trade_date) FROM price_history WHERE symbol = i.symbol
             )
             WHERE i.is_active = 1";

        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(json!({
                "symbol": row.get::<_, String>(0)?,
                "name": row.get::<_, Option<String>>(1)?,
                "sector": row.get::<_, Option<String>>(2)?,
                "price": row.get::<_, Option<f64>>(3)?,
                "change_percent": row.get::<_, Option<f64>>(4)?,
                "volume": row.get::<_, Option<i64>>(5)?,
            }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    fn get_positions(conn: &Connection, portfolio_id: Option<&str>) -> Result<Vec<Value>> {
        let pid = if let Some(id) = portfolio_id {
            id.to_string()
        } else {
            conn.query_row(
                "SELECT id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
                [],
                |row| row.get(0),
            )?
        };

        let mut stmt = conn.prepare("SELECT symbol, quantity FROM sandbox_positions WHERE portfolio_id = ?1")?;
        let rows = stmt.query_map([&pid], |row| {
            Ok(json!({ "symbol": row.get::<_, String>(0)?, "quantity": row.get::<_, f64>(1)? }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    fn get_market_context(conn: &Connection) -> Result<Option<Value>> {
        conn.query_row(
            "SELECT index_code, value, week_change FROM index_history WHERE index_code = 'ASI' ORDER BY trade_date DESC LIMIT 1",
            [],
            |row| {
                Ok(json!({
                    "index_code": row.get::<_, String>(0)?,
                    "value": row.get::<_, f64>(1)?,
                    "week_change": row.get::<_, Option<f64>>(2)?,
                }))
            },
        )
        .optional()
        .map_err(Into::into)
    }
}

struct ParamSetRow {
    id: String,
    max_daily_trades: i64,
}

use rusqlite::OptionalExtension;

pub async fn run_cycle(
    conn: &Connection,
    agent: &AgentBridge,
    settings: &AppSettings,
    cache: &crate::cache::PriceCache,
    client: &crate::ngx::NgxPulseClient,
    calendar: &crate::calendar::TradingCalendar,
    wealth: Option<&crate::wealth::WealthClient>,
) -> Result<serde_json::Value> {
    let _ = crate::ingest::IngestionService::ingest_stocks(conn, client, cache, calendar, true).await;
    let _ = crate::ingest::IngestionService::ingest_market(conn, client, calendar, true).await;

    let signal_ids = SignalGenerationService::generate_for_portfolio(conn, agent, settings, None).await?;

    let mut warnings: Vec<String> = Vec::new();
    let trading_mode = if let Some(w) = wealth {
        w.resolve_trading_mode(settings).await
    } else {
        crate::wealth::TradingMode::Sandbox
    };

    let live_market_open = if trading_mode == crate::wealth::TradingMode::Live {
        match wealth {
            Some(w) => match w.market_is_open().await {
                Ok(open) => open,
                Err(e) => {
                    warnings.push(format!("Could not check Wealth market status: {e}"));
                    false
                }
            },
            None => false,
        }
    } else {
        true
    };

    let (executed, exec_warnings) = ExecutionService::process_signals(
        conn,
        cache,
        settings,
        &signal_ids,
        wealth,
        trading_mode,
        live_market_open,
    )
    .await?;
    warnings.extend(exec_warnings);

    if trading_mode == crate::wealth::TradingMode::Sandbox {
        crate::portfolio::DailySnapshotService::create_snapshot(conn, cache, None)?;
    }

    Ok(json!({
        "signals": signal_ids.len(),
        "executed": executed,
        "tradingMode": trading_mode.as_str(),
        "warnings": warnings,
    }))
}
