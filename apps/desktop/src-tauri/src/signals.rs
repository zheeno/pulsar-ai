use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::AgentBridge;
use crate::indicators::IndicatorService;
use crate::memory;
use crate::secrets::{get_secret, SECRET_LLM_API_KEY};
use crate::settings::AppSettings;

const PORTFOLIO_PROMPT_VERSION: &str = "v2.4.0";
const PROMPT_VERSION: &str = "v1.0.0";
/// LLM may return this many BUY/SELL ideas per cycle. Executed BUYs still use max_daily_trades.
const LLM_SIGNAL_CAP: usize = 40;

#[derive(Debug, Clone)]
pub struct HeldLot {
    pub symbol: String,
    pub quantity: f64,
    pub avg_cost: f64,
    pub last_price: Option<f64>,
}

impl HeldLot {
    pub fn from_holdings(holdings: &[crate::broker::BrokerHolding]) -> Vec<Self> {
        holdings
            .iter()
            .filter(|h| h.quantity > 0.0)
            .map(|h| Self {
                symbol: h.symbol.clone(),
                quantity: h.quantity,
                avg_cost: h.buy_price.unwrap_or(0.0),
                last_price: if h.price > 0.0 { Some(h.price) } else { None },
            })
            .collect()
    }
}

pub struct PortfolioGeneration {
    pub signal_ids: Vec<String>,
    pub universe_size: usize,
}

pub struct SignalGenerationService;

impl SignalGenerationService {
    pub async fn generate_for_portfolio(
        db: &crate::db::Database,
        agent: &AgentBridge,
        settings: &AppSettings,
        portfolio_id: Option<&str>,
        cash_balance: Option<f64>,
        trading_venue: &str,
        live_holdings: Option<&[HeldLot]>,
    ) -> Result<PortfolioGeneration> {
        let live_holdings_owned = live_holdings.map(|h| h.to_vec());
        let settings_fee = settings.simulated_fee_pct;
        let venue = trading_venue.to_string();
        let pid = portfolio_id.map(|s| s.to_string());
        let prep = db.with_conn(|conn| -> Result<Option<_>> {
        let param_set = Self::get_active_param_set(conn, pid.as_deref())?;
        let Some(param_set) = param_set else {
            return Ok(None);
        };

        let lots = if venue != "sandbox" {
            live_holdings_owned.clone().unwrap_or_default()
        } else {
            Self::sandbox_lots(conn, pid.as_deref())?
        };
        let held_symbols: std::collections::HashSet<String> =
            lots.iter().map(|l| l.symbol.to_uppercase()).collect();

        let universe_rows = Self::build_universe(conn, &held_symbols)?;
        if universe_rows.is_empty() {
            return Ok(None);
        }

        let market_context = Self::get_market_context(conn)?;

        let mut memory_symbols = held_symbols.clone();
        for sym in Self::recent_active_symbols(conn, 30)? {
            memory_symbols.insert(sym);
            if memory_symbols.len() >= 40 {
                break;
            }
        }
        let symbol_memory = Self::build_symbol_memory(conn, pid.as_deref(), &memory_symbols)?;

        let cash = cash_balance.unwrap_or_else(|| {
            conn.query_row(
                "SELECT cash_balance FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
                [],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0)
        });

        let universe_tsv = Self::encode_universe_tsv(&universe_rows);
        let universe_size = universe_rows.len();
        let valid_symbols: std::collections::HashSet<String> = universe_rows
            .iter()
            .map(|r| r.symbol.clone())
            .collect();

        let mut signal_ids = vec![];
        let mut seen = std::collections::HashSet::new();
        let mut held_context = Vec::new();
        let mut positions_json = Vec::new();

        for lot in &lots {
            let last = lot
                .last_price
                .filter(|p| *p > 0.0)
                .or_else(|| Self::last_db_price(conn, &lot.symbol));
            let pnl_pct = if lot.avg_cost > 0.0 {
                last.map(|px| (px - lot.avg_cost) / lot.avg_cost)
            } else {
                None
            };
            let (exit_hint, rule) = match (pnl_pct, last) {
                (Some(pnl), Some(px)) if pnl <= -param_set.stop_loss_pct => (
                    "stop_loss",
                    Some((
                        "rules:stop-loss",
                        format!(
                            "Stop loss: {:+.1}% vs {:.0}% threshold (avg {:.2}, last {:.2})",
                            pnl * 100.0,
                            param_set.stop_loss_pct * 100.0,
                            lot.avg_cost,
                            px
                        ),
                    )),
                ),
                (Some(pnl), Some(px))
                    if param_set
                        .take_profit_pct
                        .map(|tp| tp > 0.0 && pnl >= tp)
                        .unwrap_or(false) =>
                {
                    let tp = param_set.take_profit_pct.unwrap_or(0.0);
                    (
                        "take_profit",
                        Some((
                            "rules:take-profit",
                            format!(
                                "Take profit: {:+.1}% vs {:.0}% threshold (avg {:.2}, last {:.2})",
                                pnl * 100.0,
                                tp * 100.0,
                                lot.avg_cost,
                                px
                            ),
                        )),
                    )
                }
                _ => ("hold", None),
            };

            positions_json.push(json!({
                "symbol": lot.symbol,
                "quantity": lot.quantity,
                "avgCost": lot.avg_cost,
            }));
            held_context.push(json!({
                "symbol": lot.symbol,
                "qty": lot.quantity,
                "avgCost": lot.avg_cost,
                "last": last,
                "pnlPct": pnl_pct,
                "exitHint": exit_hint,
            }));

            if let Some((model_name, rationale)) = rule {
                if seen.contains(&lot.symbol) {
                    continue;
                }
                seen.insert(lot.symbol.clone());
                let pick = json!({
                    "action": "SELL",
                    "confidence": 1.0,
                    "rationale": rationale,
                });
                if let Some(id) = Self::persist_signal(
                    conn,
                    &pick,
                    &lot.symbol,
                    "",
                    "",
                    model_name,
                    PORTFOLIO_PROMPT_VERSION,
                    false,
                )? {
                    signal_ids.push(id);
                }
            }
        }

        let llm_cap = universe_size.min(LLM_SIGNAL_CAP) as i64;

        let context = json!({
            "universeTsv": universe_tsv,
            "universeSize": universe_size,
            "positions": positions_json,
            "held": held_context,
            "strategy": {
                "stopLossPct": param_set.stop_loss_pct,
                "takeProfitPct": param_set.take_profit_pct,
                "maxDailyTrades": param_set.max_daily_trades,
            },
            "marketContext": market_context,
            "maxActions": llm_cap,
            "symbolMemory": symbol_memory,
            "cashBalance": cash,
            "brokerageBalance": if venue != "sandbox" { Some(cash) } else { None::<f64> },
            "tradingVenue": venue,
            "estimatedFeePct": settings_fee,
        });
        Ok(Some((context, valid_symbols, held_symbols, seen, signal_ids, universe_size)))
        })?;
        let Some((mut context, valid_symbols, held_symbols, mut seen, mut signal_ids, universe_size)) = prep else {
            return Ok(PortfolioGeneration {
                signal_ids: vec![],
                universe_size: 0,
            });
        };

        let api_key = get_secret(SECRET_LLM_API_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        let held_q: String = {
            let mut v: Vec<_> = held_symbols.iter().cloned().collect();
            v.sort();
            v.join(" ")
        };
        let tsv = context
            .get("universeTsv")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let movers: String = tsv.lines().take(20).collect::<Vec<_>>().join(" ");
        let mem_query = format!("holdings {held_q} {movers}");
        let retrieved = memory::search_memories(db, settings, &api_key, &mem_query, None, Some(8))
            .await
            .unwrap_or_default();
        if let Some(obj) = context.as_object_mut() {
            obj.insert("retrievedMemories".into(), json!(retrieved));
        }
        let _ = memory::backfill_missing_embeddings(db, settings, &api_key).await;

        let result = agent.portfolio_signals(settings, context, db).await?;
        let signals = result
            .get("output")
            .and_then(|o| o.get("signals"))
            .or_else(|| result.get("signals"))
            .cloned()
            .unwrap_or(json!([]));

        let prompt = result.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let raw_response = result.get("rawResponse").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let model_name = result.get("modelName").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();

        db.with_conn(|conn| {
        if let Some(arr) = signals.as_array() {
            let mut buys = 0usize;
            let mut sells = 0usize;
            for pick in arr.iter().take(crate::execution::MAX_SIGNAL_TOTAL) {
                let symbol = pick.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
                if symbol.is_empty() || seen.contains(symbol) || !valid_symbols.contains(symbol) {
                    continue;
                }
                if !crate::ngx::is_valid_ticker(symbol) {
                    continue;
                }
                let action = pick.get("action").and_then(|v| v.as_str()).unwrap_or("");
                if action == "BUY" {
                    if buys >= crate::execution::MAX_SIGNAL_BUYS { continue; }
                    buys += 1;
                } else if action == "SELL" {
                    if !llm_sell_is_held(symbol, &held_symbols) {
                        continue;
                    }
                    if sells >= crate::execution::MAX_SIGNAL_SELLS { continue; }
                    sells += 1;
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
                    settings.retain_raw_llm_logs,
                )?;
                if let Some(id) = signal_id {
                    signal_ids.push(id);
                }
            }
        }
        Ok(())
        })?;

        Ok(PortfolioGeneration {
            signal_ids,
            universe_size,
        })
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

        let mut mem_set = std::collections::HashSet::new();
        mem_set.insert(symbol.to_string());
        let symbol_memory = Self::build_symbol_memory(conn, None, &mem_set)?;

        let context = json!({
            "symbol": symbol,
            "technical": technical,
            "symbolMemory": symbol_memory.get(symbol).cloned().unwrap_or(json!({})),
        });
        let result = agent.symbol_signal(settings, context).await?;
        let output = result.get("output").cloned().unwrap_or(result.clone());
        let prompt = result.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let raw_response = result.get("rawResponse").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let model_name = result.get("modelName").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();

        Self::persist_signal(conn, &output, symbol, &prompt, &raw_response, &model_name, PROMPT_VERSION, settings.retain_raw_llm_logs)
    }

    fn persist_signal(
        conn: &Connection,
        pick: &Value,
        symbol: &str,
        prompt: &str,
        raw_response: &str,
        model_name: &str,
        prompt_version: &str,
        retain_raw: bool,
    ) -> Result<Option<String>> {
        let action = pick.get("action").and_then(|v| v.as_str()).unwrap_or("HOLD");
        let confidence = pick.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rationale = pick.get("rationale").and_then(|v| v.as_str()).unwrap_or("");

        if symbol.is_empty() || !crate::ngx::is_valid_ticker(symbol) {
            return Ok(None);
        }

        let signal_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result)
             VALUES (?1, ?2, ?3, ?4, ?5, '{}', ?6, ?7, 'BLOCKED_OTHER')",
            rusqlite::params![signal_id, symbol, action, confidence, rationale, model_name, prompt_version],
        )?;

        let log_id = Uuid::new_v4().to_string();
        let stored_prompt = if retain_raw {
            redact_audit(prompt, 4000)
        } else {
            redact_audit(prompt, 400)
        };
        let stored_response = if retain_raw {
            redact_audit(raw_response, 4000)
        } else {
            redact_audit(raw_response, 400)
        };
        conn.execute(
            "INSERT INTO signal_llm_logs (id, signal_id, prompt, raw_response) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![log_id, signal_id, stored_prompt, stored_response],
        )?;

        Ok(Some(signal_id))
    }

    fn get_active_param_set(conn: &Connection, portfolio_id: Option<&str>) -> Result<Option<ParamSetRow>> {
        let map_row = |row: &rusqlite::Row| {
            Ok(ParamSetRow {
                id: row.get(0)?,
                max_daily_trades: row.get(1)?,
                stop_loss_pct: row.get(2)?,
                take_profit_pct: row.get(3)?,
            })
        };
        if let Some(pid) = portfolio_id {
            let row: Option<ParamSetRow> = conn
                .query_row(
                    "SELECT s.id, s.max_daily_trades, s.stop_loss_pct, s.take_profit_pct
                     FROM strategy_param_sets s
                     JOIN sandbox_portfolios p ON p.strategy_param_set_id = s.id
                     WHERE p.id = ?1 AND s.is_active = 1 LIMIT 1",
                    [pid],
                    map_row,
                )
                .ok();
            if row.is_some() {
                return Ok(row);
            }
        }

        conn.query_row(
            "SELECT id, max_daily_trades, stop_loss_pct, take_profit_pct FROM strategy_param_sets WHERE is_active = 1 LIMIT 1",
            [],
            map_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn build_universe(
        conn: &Connection,
        held_symbols: &std::collections::HashSet<String>,
    ) -> Result<Vec<UniverseRow>> {
        let sql = "SELECT i.symbol, i.sector, ph.price, ph.change_percent, ph.volume
             FROM instruments i
             LEFT JOIN price_history ph ON ph.symbol = i.symbol AND ph.trade_date = (
               SELECT MAX(trade_date) FROM price_history WHERE symbol = i.symbol
             )
             WHERE i.is_active = 1";

        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(UniverseRow {
                symbol: row.get::<_, String>(0)?,
                sector: row.get::<_, Option<String>>(1)?,
                price: row.get::<_, Option<f64>>(2)?,
                change_percent: row.get::<_, Option<f64>>(3)?,
                volume: row.get::<_, Option<i64>>(4)?,
            })
        })?;
        let mut out: Vec<UniverseRow> = rows
            .filter_map(|r| r.ok())
            .filter(|r| r.price.is_some())
            .collect();
        out.sort_by(|a, b| {
            let ah = held_symbols.contains(&a.symbol.to_uppercase());
            let bh = held_symbols.contains(&b.symbol.to_uppercase());
            bh.cmp(&ah).then_with(|| {
                let ac = a.change_percent.unwrap_or(0.0).abs();
                let bc = b.change_percent.unwrap_or(0.0).abs();
                bc.partial_cmp(&ac)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.volume.unwrap_or(0).cmp(&a.volume.unwrap_or(0)))
            })
        });
        Ok(out)
    }

    fn encode_universe_tsv(rows: &[UniverseRow]) -> String {
        let mut out = String::from("SYM\tpx\tpct\tvol\tsec\n");
        for row in rows {
            let px = row.price.unwrap_or(0.0);
            let pct = row.change_percent.unwrap_or(0.0);
            let vol = row.volume.unwrap_or(0);
            let sec = sector_code(row.sector.as_deref());
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                row.symbol,
                compact_num(px),
                compact_num(pct),
                compact_vol(vol),
                sec
            ));
        }
        out
    }

    fn sandbox_lots(conn: &Connection, portfolio_id: Option<&str>) -> Result<Vec<HeldLot>> {
        let pid = if let Some(id) = portfolio_id {
            id.to_string()
        } else {
            conn.query_row(
                "SELECT id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
                [],
                |row| row.get(0),
            )?
        };

        let mut stmt = conn.prepare(
            "SELECT symbol, quantity, avg_cost FROM sandbox_positions WHERE portfolio_id = ?1 AND quantity > 0",
        )?;
        let rows = stmt.query_map([&pid], |row| {
            Ok(HeldLot {
                symbol: row.get(0)?,
                quantity: row.get(1)?,
                avg_cost: row.get(2)?,
                last_price: None,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    fn last_db_price(conn: &Connection, symbol: &str) -> Option<f64> {
        conn.query_row(
            "SELECT price FROM price_history WHERE symbol = ?1 ORDER BY trade_date DESC LIMIT 1",
            [symbol],
            |row| row.get(0),
        )
        .ok()
        .filter(|p: &f64| *p > 0.0)
    }

    fn recent_active_symbols(conn: &Connection, days: i64) -> Result<Vec<String>> {
        let cutoff = format!("-{days} days");
        let mut set = std::collections::HashSet::new();
        let mut stmt = conn.prepare(
            "SELECT symbol FROM signals WHERE generated_at >= datetime('now', ?1)
             UNION
             SELECT symbol FROM sandbox_trades WHERE executed_at >= datetime('now', ?1)
             UNION
             SELECT symbol FROM broker_orders WHERE created_at >= datetime('now', ?1)
             LIMIT 40",
        )?;
        for row in stmt.query_map([&cutoff], |row| row.get::<_, String>(0))? {
            set.insert(row?);
        }
        Ok(set.into_iter().collect())
    }

    fn build_symbol_memory(
        conn: &Connection,
        portfolio_id: Option<&str>,
        symbols: &std::collections::HashSet<String>,
    ) -> Result<serde_json::Map<String, Value>> {
        let mut memory = serde_json::Map::new();
        let pid = portfolio_id.map(|s| s.to_string()).or_else(|| {
            conn.query_row(
                "SELECT id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok()
        });

        for symbol in symbols {
            let mut recent_signals = Vec::new();
            let mut stmt = conn.prepare(
                "SELECT generated_at, action, confidence, rationale, executed, risk_policy_result
                 FROM signals WHERE symbol = ?1 ORDER BY generated_at DESC LIMIT 5",
            )?;
            for row in stmt.query_map([symbol], |row| {
                let rationale = row.get::<_, String>(3)?;
                Ok(json!({
                    "generatedAt": row.get::<_, String>(0)?,
                    "action": row.get::<_, String>(1)?,
                    "confidence": row.get::<_, f64>(2)?,
                    "rationale": truncate_chars(&rationale, 240),
                    "executed": row.get::<_, i64>(4)? == 1,
                    "riskPolicyResult": row.get::<_, String>(5)?,
                }))
            })? {
                recent_signals.push(row?);
            }

            let mut recent_trades = Vec::new();
            let mut stmt = conn.prepare(
                "SELECT side, quantity, fill_price, simulated_fee, executed_at
                 FROM sandbox_trades WHERE symbol = ?1 ORDER BY executed_at DESC LIMIT 5",
            )?;
            for row in stmt.query_map([symbol], |row| {
                Ok(json!({
                    "side": row.get::<_, String>(0)?,
                    "quantity": row.get::<_, f64>(1)?,
                    "fillPrice": row.get::<_, f64>(2)?,
                    "fee": row.get::<_, f64>(3)?,
                    "executedAt": row.get::<_, String>(4)?,
                    "venue": "sandbox",
                }))
            })? {
                recent_trades.push(row?);
            }
            let mut stmt = conn.prepare(
                "SELECT side, quantity, fill_price, fee, created_at, status
                 FROM broker_orders WHERE symbol = ?1 ORDER BY created_at DESC LIMIT 5",
            )?;
            for row in stmt.query_map([symbol], |row| {
                Ok(json!({
                    "side": row.get::<_, String>(0)?,
                    "quantity": row.get::<_, f64>(1)?,
                    "fillPrice": row.get::<_, Option<f64>>(2)?,
                    "fee": row.get::<_, Option<f64>>(3)?,
                    "executedAt": row.get::<_, String>(4)?,
                    "status": row.get::<_, String>(5)?,
                    "venue": "wealth",
                }))
            })? {
                recent_trades.push(row?);
            }
            recent_trades.sort_by(|a, b| {
                let da = a.get("executedAt").and_then(|v| v.as_str()).unwrap_or("");
                let db = b.get("executedAt").and_then(|v| v.as_str()).unwrap_or("");
                db.cmp(da)
            });
            recent_trades.truncate(5);

            let position = if let Some(ref pid) = pid {
                conn.query_row(
                    "SELECT quantity, avg_cost FROM sandbox_positions WHERE portfolio_id = ?1 AND symbol = ?2",
                    rusqlite::params![pid, symbol],
                    |row| {
                        Ok(json!({
                            "quantity": row.get::<_, f64>(0)?,
                            "avgCost": row.get::<_, f64>(1)?,
                        }))
                    },
                )
                .optional()?
            } else {
                None
            };

            if recent_signals.is_empty() && recent_trades.is_empty() && position.is_none() {
                continue;
            }

            let mut entry = serde_json::Map::new();
            if !recent_signals.is_empty() {
                entry.insert("recentSignals".into(), json!(recent_signals));
            }
            if !recent_trades.is_empty() {
                entry.insert("recentTrades".into(), json!(recent_trades));
            }
            if let Some(pos) = position {
                entry.insert("position".into(), pos);
            }
            memory.insert(symbol.clone(), Value::Object(entry));
        }
        Ok(memory)
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

struct UniverseRow {
    symbol: String,
    sector: Option<String>,
    price: Option<f64>,
    change_percent: Option<f64>,
    volume: Option<i64>,
}

struct ParamSetRow {
    id: String,
    max_daily_trades: i64,
    stop_loss_pct: f64,
    take_profit_pct: Option<f64>,
}

fn redact_audit(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect();
    truncate_chars(&cleaned, max)
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let taken: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
    }
}

fn compact_num(n: f64) -> String {
    if n.abs() >= 100.0 {
        format!("{n:.1}")
    } else {
        format!("{n:.2}")
    }
    .trim_end_matches('0')
    .trim_end_matches('.')
    .to_string()
}

fn compact_vol(v: i64) -> String {
    if v >= 1_000_000 {
        format!("{:.1}e6", v as f64 / 1_000_000.0)
    } else if v >= 10_000 {
        format!("{:.0}e3", v as f64 / 1_000.0)
    } else {
        v.to_string()
    }
}

fn sector_code(sector: Option<&str>) -> String {
    let Some(raw) = sector.filter(|s| !s.is_empty()) else {
        return "?".into();
    };
    let l = raw.to_ascii_lowercase();
    let code = if l.contains("financial") || l.contains("bank") || l.contains("insurance") {
        "FIN"
    } else if l.contains("consumer") || l.contains("food") || l.contains("brew") {
        "CG"
    } else if l.contains("oil") || l.contains("gas") || l.contains("energy") {
        "OG"
    } else if l.contains("ict") || l.contains("telecom") || l.contains("tech") {
        "ICT"
    } else if l.contains("industrial") || l.contains("cement") || l.contains("construct") {
        "IND"
    } else if l.contains("agric") || l.contains("palm") {
        "AGR"
    } else if l.contains("health") || l.contains("pharma") {
        "HLTH"
    } else if l.contains("service") {
        "SVC"
    } else if l.contains("estate") || l.contains("property") {
        "RE"
    } else if l.contains("natural") || l.contains("mining") {
        "RES"
    } else {
        let mut c: String = raw
            .chars()
            .filter(|ch| ch.is_ascii_alphabetic())
            .take(3)
            .collect::<String>()
            .to_ascii_uppercase();
        if c.is_empty() {
            c = "?".into();
        }
        return c;
    };
    code.into()
}

pub async fn run_cycle(
    db: &crate::db::Database,
    agent: &AgentBridge,
    settings: &AppSettings,
    cache: &crate::cache::PriceCache,
    client: &crate::ngx::NgxPulseClient,
    calendar: &crate::calendar::TradingCalendar,
    broker: Option<&crate::broker::BrokerSession>,
    execute: bool,
    allow_bulk_liquidation: bool,
    cycle_id: Option<&str>,
) -> Result<serde_json::Value> {
    let mut warnings: Vec<String> = Vec::new();
    let ingested = match crate::ingest::IngestionService::ingest_stocks(db, client, cache, calendar, true)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            warnings.push(format!("Pulse stock ingest failed: {e}"));
            0
        }
    };
    if let Err(e) = crate::ingest::IngestionService::ingest_market(db, client, calendar, true).await {
        warnings.push(format!("Pulse market ingest failed: {e}"));
    }
    let trading_mode = if let Some(session) = broker {
        session.resolve_trading_mode(settings).await
    } else {
        crate::wealth::TradingMode::Sandbox
    };

    let mut live_snap: Option<crate::wealth::WealthPortfolioSnapshot> = None;
    let (cash_for_agent, trading_venue, live_holdings) =
        if trading_mode == crate::wealth::TradingMode::Live {
            if let Some(session) = broker {
                if execute {
                    let _ = crate::execution::ExecutionService::reconcile_if_possible(db, session).await;
                }
                let book = match session.refresh_book(db).await {
                    Ok(b) => Some(b),
                    Err(e) => {
                        warnings.push(format!("Could not refresh {} holdings: {e}", session.display_name()));
                        session.load_book(db).ok().flatten()
                    }
                };
                let cash = book.as_ref().map(|b| b.brokerage_balance);
                let holdings = book
                    .as_ref()
                    .map(|b| HeldLot::from_holdings(&b.holdings))
                    .unwrap_or_default();
                let venue = session.id().as_str();
                live_snap = book.map(|b| crate::wealth::WealthPortfolioSnapshot {
                    balance: b.brokerage_balance,
                    profit: b.profit,
                    stock_value: b.stock_value,
                    holdings: b.holdings,
                    stocks_present: true,
                });
                (cash, venue, holdings)
            } else {
                (None, "sandbox", Vec::new())
            }
        } else {
            (None, "sandbox", Vec::new())
        };

    let generated = SignalGenerationService::generate_for_portfolio(
        db,
        agent,
        settings,
        None,
        cash_for_agent,
        trading_venue,
        if trading_venue != "sandbox" {
            Some(live_holdings.as_slice())
        } else {
            None
        },
    )
    .await?;
    let signal_ids = generated.signal_ids;

    if generated.universe_size > 0 && generated.universe_size <= 20 {
        warnings.push(format!(
            "Universe is only {} names (seed size). Pulse ingest may be incomplete.",
            generated.universe_size
        ));
    }

    let live_market_open = if trading_mode == crate::wealth::TradingMode::Live {
        match broker {
            Some(session) => match session.market_is_open().await {
                Ok(open) => open,
                Err(e) => {
                    warnings.push(format!("Could not check {} market status: {e}", session.display_name()));
                    false
                }
            },
            None => false,
        }
    } else {
        true
    };

    let do_execute = match trading_mode {
        crate::wealth::TradingMode::Sandbox => true,
        crate::wealth::TradingMode::Live => execute && settings.live_trading_enabled,
    };

    let (executed, exec_warnings) = crate::execution::ExecutionService::process_signals(
        db,
        cache,
        settings,
        &signal_ids,
        broker,
        trading_mode,
        live_market_open,
        do_execute,
        allow_bulk_liquidation,
        cycle_id,
    )
    .await?;
    warnings.extend(exec_warnings);

    if trading_mode == crate::wealth::TradingMode::Sandbox {
        db.with_conn(|conn| crate::portfolio::DailySnapshotService::create_snapshot(conn, cache, None))?;
    } else if execute {
        if let Some(session) = broker {
            if let Ok(book) = session.refresh_book(db).await {
                let _ = db.with_conn(|conn| {
                    crate::portfolio::EquityCurveService::insert_point(
                        conn,
                        session.id().as_str(),
                        book.brokerage_balance + book.market_value(),
                        book.brokerage_balance,
                        book.market_value(),
                    )
                });
            } else if let Some(snap) = live_snap {
                let cash = snap.balance;
                let market_value = snap.holdings.iter().map(|h| h.current_value).sum();
                let _ = db.with_conn(|conn| {
                    crate::portfolio::EquityCurveService::insert_point(
                        conn,
                        session.id().as_str(),
                        cash + market_value,
                        cash,
                        market_value,
                    )
                });
            }
        }
    }

    let _ = db.with_conn(|conn| {
        conn.execute(
            "DELETE FROM cycle_audits WHERE expires_at < datetime('now')",
            [],
        )?;
        conn.execute(
            "INSERT INTO cycle_audits (id, cycle_id, summary, expires_at)
             VALUES (?1, ?2, ?3, datetime('now', '+14 days'))",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                cycle_id.unwrap_or(""),
                format!(
                    "signals={} executed={} venue={} universe={}",
                    signal_ids.len(),
                    executed,
                    trading_mode.as_str(),
                    generated.universe_size
                )
            ],
        )?;
        Ok(())
    });

    Ok(json!({
        "signals": signal_ids.len(),
        "executed": executed,
        "signalIds": signal_ids,
        "tradingMode": trading_mode.as_str(),
        "warnings": warnings,
        "universeSize": generated.universe_size,
        "instrumentsIngested": ingested,
    }))
}

fn llm_sell_is_held(symbol: &str, held: &std::collections::HashSet<String>) -> bool {
    held.contains(&symbol.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn held(symbols: &[&str]) -> HashSet<String> {
        symbols.iter().map(|s| s.to_uppercase()).collect()
    }

    #[test]
    fn llm_sell_kept_when_held() {
        assert!(llm_sell_is_held("GTCO", &held(&["GTCO", "DANGCEM"])));
    }

    #[test]
    fn llm_sell_dropped_when_unheld() {
        assert!(!llm_sell_is_held("SEPLAT", &held(&["GTCO"])));
        assert!(!llm_sell_is_held("GTCO", &HashSet::new()));
    }

    #[test]
    fn llm_sell_held_ignores_case() {
        assert!(llm_sell_is_held("gtco", &held(&["GTCO"])));
        assert!(llm_sell_is_held("Gtco", &held(&["gtco"])));
    }

    #[test]
    fn llm_buy_unaffected_by_held_check() {
        // BUY persistence does not call llm_sell_is_held; this documents that
        // an unheld ticker still fails the SELL helper only.
        assert!(!llm_sell_is_held("AIRTELAFRI", &held(&["GTCO"])));
    }
}
