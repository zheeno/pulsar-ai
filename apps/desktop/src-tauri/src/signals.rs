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
/// Hard ceiling on LLM BUY+SELL ideas per cycle (further capped by trade capacity).
const LLM_SIGNAL_CAP: usize = 40;
/// Soft target for BUY count when capacity allows (prompt + post-LLM backfill).
const MIN_BUY_TARGET: usize = 8;
/// Max accepted BUYs per sector code in pass A (pass B may relax to fill min target).
const MAX_BUYS_PER_SECTOR: usize = 3;
/// Block BUY re-entry after an executed SELL within this window.
const BUY_REENTRY_COOLDOWN_HOURS: i64 = 6;

#[derive(Debug, Clone)]
pub struct HeldLot {
    pub symbol: String,
    pub quantity: f64,
    pub avg_cost: f64,
    pub last_price: Option<f64>,
    pub opened_at: Option<String>,
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
                opened_at: None,
            })
            .collect()
    }

    pub fn hours_held(&self) -> Option<f64> {
        let raw = self.opened_at.as_deref()?;
        let parsed = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f"))
            .or_else(|_| chrono::DateTime::parse_from_rfc3339(raw).map(|d| d.naive_utc()))
            .ok()?;
        let now = chrono::Utc::now().naive_utc();
        let secs = (now - parsed).num_seconds() as f64;
        if secs.is_finite() && secs > 0.0 {
            Some(secs / 3600.0)
        } else {
            None
        }
    }
}

pub struct PortfolioGeneration {
    pub signal_ids: Vec<String>,
    pub universe_size: usize,
    pub warnings: Vec<String>,
    pub buys_disabled: bool,
}

#[derive(Debug, Clone)]
struct TradeCapacity {
    remaining_buy_slots: i64,
    buy_allowed: bool,
    sell_allowed: bool,
    max_buy_signals: usize,
    max_sell_signals: usize,
    max_actions: usize,
    buys_disabled: bool,
    min_confidence: f64,
    skip_llm: bool,
    skip_reason: Option<String>,
    capital_reason: Option<&'static str>,
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

        let mut lots = if venue != "sandbox" {
            live_holdings_owned.clone().unwrap_or_default()
        } else {
            Self::sandbox_lots(conn, pid.as_deref())?
        };
        if venue != "sandbox" {
            Self::attach_live_opened_at(conn, &mut lots);
        }
        let held_symbols: std::collections::HashSet<String> =
            lots.iter().map(|l| l.symbol.to_uppercase()).collect();

        let recent_active = Self::recent_active_symbols(conn, 30)?;
        let recent_set: std::collections::HashSet<String> =
            recent_active.iter().map(|s| s.to_uppercase()).collect();
        let recently_sold = Self::recently_sold_symbols(conn, BUY_REENTRY_COOLDOWN_HOURS)?;
        let queued_buy_symbols = Self::queued_unexecuted_buy_symbols(conn)?;
        let pending_unexecuted = Self::pending_unexecuted_for_prompt(conn)?;

        let mut universe_rows = Self::build_universe(conn, &held_symbols, &recent_set)?;
        universe_rows.retain(|row| {
            keep_unheld_for_llm(
                held_symbols.contains(&row.symbol.to_uppercase()),
                IndicatorService::is_overbought(conn, &row.symbol).unwrap_or(false),
            )
        });
        if universe_rows.is_empty() {
            return Ok(None);
        }

        let market_context = Self::get_market_context(conn)?;

        let mut memory_symbols = held_symbols.clone();
        for sym in &recent_active {
            memory_symbols.insert(sym.clone());
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
        let spendable = if venue == "sandbox" {
            cash
        } else {
            let pending = crate::intents::pending_buy_notional_on(conn, Some(&venue)).unwrap_or(0.0);
            (cash - pending).max(0.0)
        };

        let universe_tsv = Self::encode_universe_tsv(&universe_rows);
        let universe_size = universe_rows.len();
        let valid_symbols: std::collections::HashSet<String> = universe_rows
            .iter()
            .map(|r| r.symbol.clone())
            .collect();
        let symbol_sectors: std::collections::HashMap<String, String> = universe_rows
            .iter()
            .map(|r| {
                (
                    r.symbol.to_uppercase(),
                    sector_code(r.sector.as_deref()),
                )
            })
            .collect();
        let held_sector_counts = held_sector_counts_json(&lots, &symbol_sectors);

        let mut signal_ids = vec![];
        let mut seen = std::collections::HashSet::new();
        let mut held_context = Vec::new();
        let mut positions_json = Vec::new();

        for lot in &lots {
            let last = lot
                .last_price
                .filter(|p| *p > 0.0)
                .or_else(|| Self::last_db_price(conn, &lot.symbol));
            let assessment = crate::risk_exits::assess_position_exit_full(
                &lot.symbol,
                lot.avg_cost,
                last,
                crate::risk_exits::ExitParams {
                    stop_loss_pct: param_set.stop_loss_pct,
                    take_profit_pct: param_set.take_profit_pct,
                    time_stop_hours: param_set.time_stop_hours,
                    partial_tp_fraction: param_set.partial_tp_fraction,
                },
                lot.hours_held(),
            );

            positions_json.push(json!({
                "symbol": lot.symbol,
                "quantity": lot.quantity,
                "avgCost": lot.avg_cost,
            }));
            held_context.push(json!({
                "symbol": lot.symbol,
                "qty": lot.quantity,
                "avgCost": lot.avg_cost,
                "last": assessment.last,
                "pnlPct": assessment.pnl_pct,
                "exitHint": assessment.exit_hint,
            }));

            if let Some(exit) = assessment.exit {
                if seen.contains(&lot.symbol) {
                    continue;
                }
                seen.insert(lot.symbol.clone());
                if let Some(id) = Self::persist_rule_sell(
                    conn,
                    &exit.symbol,
                    exit.kind.model_name(),
                    &exit.rationale,
                    exit.confidence,
                    exit.sell_fraction,
                )? {
                    signal_ids.push(id);
                }
            }
        }

        let portfolio_db_id = if venue != "sandbox" {
            None
        } else if let Some(id) = pid.as_deref() {
            Some(id.to_string())
        } else {
            conn.query_row(
                "SELECT id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
        };

        let positions_for_risk: Vec<(String, f64, f64)> = lots
            .iter()
            .map(|l| (l.symbol.clone(), l.quantity, l.avg_cost))
            .collect();
        let market_value: f64 = lots
            .iter()
            .map(|l| {
                let px = l
                    .last_price
                    .filter(|p| *p > 0.0)
                    .or_else(|| Self::last_db_price(conn, &l.symbol))
                    .unwrap_or(l.avg_cost);
                l.quantity * px
            })
            .sum();
        let total_equity = cash + market_value;
        let daily_drawdown = if venue != "sandbox" {
            crate::execution::live_drawdown_pct(conn, &venue, total_equity).unwrap_or(0.0)
        } else if let Some(ref id) = portfolio_db_id {
            conn.query_row(
                "SELECT drawdown_pct FROM daily_performance_snapshot WHERE portfolio_id = ?1 ORDER BY snapshot_date DESC LIMIT 1",
                [id],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0)
        } else {
            0.0
        };
        let risk_params = param_set.to_param_set();

        let min_n = crate::execution::min_order_notional_for_venue(&venue);
        let remaining_budget = crate::execution::remaining_buy_budget(
            spendable,
            param_set.cycle_budget_pct,
            min_n,
        );
        let capacity = compute_trade_capacity(
            &param_set,
            &risk_params,
            spendable,
            total_equity,
            daily_drawdown,
            settings_fee,
            &universe_rows,
            &positions_for_risk,
            &held_symbols,
            &seen,
            min_n,
            remaining_budget,
            settings.halt_new_buys,
        );

        let min_buy_signals = MIN_BUY_TARGET.min(capacity.max_buy_signals);
        let mut recently_sold_list: Vec<String> = recently_sold.iter().cloned().collect();
        recently_sold_list.sort();
        let context = json!({
            "universeTsv": universe_tsv,
            "universeSize": universe_size,
            "positions": positions_json,
            "held": held_context,
            "strategy": {
                "stopLossPct": param_set.stop_loss_pct,
                "takeProfitPct": param_set.take_profit_pct,
                "minConfidenceToTrade": param_set.min_confidence_to_trade,
                "maxPositionPct": param_set.max_position_pct,
                "cycleBudgetPct": param_set.cycle_budget_pct,
                "maxDailyDrawdownPct": param_set.max_daily_drawdown_pct,
            },
            "executionConstraints": {
                "remainingBuySlots": capacity.remaining_buy_slots,
                "maxBuySignals": capacity.max_buy_signals,
                "maxSellSignals": capacity.max_sell_signals,
                "buysDisabled": capacity.buys_disabled,
                "minConfidenceToTrade": capacity.min_confidence,
                "cashBalance": cash,
                "estimatedFeePct": settings_fee,
                "cycleBudgetPct": param_set.cycle_budget_pct,
                "maxPositionPct": param_set.max_position_pct,
                "minOrderNotional": crate::execution::min_order_notional_for_venue(&venue),
            },
            "diversification": {
                "minBuySignals": min_buy_signals,
                "maxBuysPerSector": MAX_BUYS_PER_SECTOR,
                "heldSectorCounts": held_sector_counts,
                "buyReentryCooldownHours": BUY_REENTRY_COOLDOWN_HOURS,
            },
            "recentlySold": recently_sold_list,
            "pendingUnexecuted": pending_unexecuted,
            "marketContext": market_context,
            "maxActions": capacity.max_actions as i64,
            "symbolMemory": symbol_memory,
            "cashBalance": cash,
            "brokerageBalance": if venue != "sandbox" { Some(cash) } else { None::<f64> },
            "tradingVenue": venue,
            "estimatedFeePct": settings_fee,
        });
        Ok(Some((
            context,
            valid_symbols,
            held_symbols,
            seen,
            signal_ids,
            universe_size,
            capacity,
            symbol_sectors,
            min_buy_signals,
            recently_sold,
            queued_buy_symbols,
        )))
        })?;
        let Some((
            mut context,
            valid_symbols,
            held_symbols,
            mut seen,
            mut signal_ids,
            universe_size,
            capacity,
            symbol_sectors,
            min_buy_signals,
            recently_sold,
            queued_buy_symbols,
        )) = prep
        else {
            return Ok(PortfolioGeneration {
                signal_ids: vec![],
                universe_size: 0,
                warnings: vec!["No priced instruments or active strategy.".into()],
                buys_disabled: true,
            });
        };

        if capacity.skip_llm {
            let mut warnings = Vec::new();
            if let Some(reason) = capacity.capital_reason {
                tracing::info!(target: "signals", reason, "capital-constrained");
                warnings.push(format!("capital-constrained: {reason}"));
            }
            if let Some(reason) = capacity.skip_reason {
                warnings.push(reason);
            }
            return Ok(PortfolioGeneration {
                signal_ids,
                universe_size,
                warnings,
                buys_disabled: capacity.buys_disabled,
            });
        }

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

        let mut gen_warnings: Vec<String> = Vec::new();
        if let Some(reason) = capacity.capital_reason {
            tracing::info!(target: "signals", reason, "capital-constrained");
            gen_warnings.push(format!("capital-constrained: {reason}"));
        } else if capacity.buys_disabled {
            gen_warnings.push(
                "BUY capacity exhausted — LLM was constrained to discretionary SELLs only.".into(),
            );
        } else {
            tracing::info!(
                target: "signals",
                remaining_buy_slots = capacity.remaining_buy_slots,
                max_buy_signals = capacity.max_buy_signals,
                "trade capacity allows BUYs"
            );
        }
        db.with_conn(|conn| {
        if let Some(arr) = signals.as_array() {
            if arr.is_empty() {
                gen_warnings.push(format!(
                    "LLM returned an empty signals array (buy slots remaining: {}).",
                    capacity.remaining_buy_slots
                ));
            }
            let mut dropped_confidence = 0usize;
            let mut dropped_symbol = 0usize;
            let mut dropped_reentry = 0usize;
            let mut dropped_queued = 0usize;
            let mut drop_symbol_samples: Vec<String> = Vec::new();
            let buy_cap = capacity.max_buy_signals.min(crate::execution::MAX_SIGNAL_BUYS);
            let sell_cap = capacity.max_sell_signals.min(crate::execution::MAX_SIGNAL_SELLS);

            let mut buy_candidates: Vec<(Value, String, String)> = Vec::new();
            let mut sell_picks: Vec<(Value, String)> = Vec::new();

            for pick in arr.iter().take(crate::execution::MAX_SIGNAL_TOTAL) {
                let raw_symbol = pick.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
                let symbol = raw_symbol.trim().to_uppercase();
                if symbol.is_empty() {
                    continue;
                }
                if seen.contains(&symbol) {
                    continue;
                }
                if !valid_symbols.contains(&symbol) {
                    dropped_symbol += 1;
                    if drop_symbol_samples.len() < 5 {
                        drop_symbol_samples.push(raw_symbol.to_string());
                    }
                    continue;
                }
                if !crate::ngx::is_valid_ticker(&symbol) {
                    dropped_symbol += 1;
                    continue;
                }
                let action = pick.get("action").and_then(|v| v.as_str()).unwrap_or("");
                let confidence = pick.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if action == "BUY" {
                    if capacity.buys_disabled || buy_cap == 0 {
                        continue;
                    }
                    if confidence < capacity.min_confidence {
                        dropped_confidence += 1;
                        continue;
                    }
                    if buy_blocked_by_reentry_cooldown(&symbol, &recently_sold) {
                        dropped_reentry += 1;
                        continue;
                    }
                    if queued_buy_symbols.contains(&symbol) {
                        dropped_queued += 1;
                        continue;
                    }
                    let sec = symbol_sectors
                        .get(&symbol)
                        .cloned()
                        .unwrap_or_else(|| "?".into());
                    buy_candidates.push((pick.clone(), symbol, sec));
                } else if action == "SELL" {
                    if !llm_sell_is_held(&symbol, &held_symbols) {
                        continue;
                    }
                    sell_picks.push((pick.clone(), symbol));
                }
            }

            let mut sells = 0usize;
            for (pick, symbol) in sell_picks {
                if sells >= sell_cap || seen.contains(&symbol) {
                    continue;
                }
                seen.insert(symbol.clone());
                sells += 1;
                let signal_id = Self::persist_signal(
                    conn,
                    &pick,
                    &symbol,
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

            let candidate_keys: Vec<(String, String)> = buy_candidates
                .iter()
                .map(|(_, sym, sec)| (sym.clone(), sec.clone()))
                .collect();
            let (accept_idx, dropped_sector) = plan_buy_accept_indices(
                &candidate_keys,
                buy_cap,
                min_buy_signals,
                MAX_BUYS_PER_SECTOR,
            );
            for i in accept_idx {
                let (pick, symbol, _) = &buy_candidates[i];
                if seen.contains(symbol) {
                    continue;
                }
                seen.insert(symbol.clone());
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

            if dropped_confidence > 0 {
                gen_warnings.push(format!(
                    "Dropped {dropped_confidence} BUY signal(s) below min confidence ({:.0}%).",
                    capacity.min_confidence * 100.0
                ));
            }
            if dropped_reentry > 0 {
                gen_warnings.push(format!(
                    "Dropped {dropped_reentry} BUY(s) in SELL re-entry cooldown ({BUY_REENTRY_COOLDOWN_HOURS}h)."
                ));
            }
            if dropped_queued > 0 {
                gen_warnings.push(format!(
                    "Dropped {dropped_queued} BUY(s) already queued from earlier cycles (will retry those orders)."
                ));
            }
            if dropped_sector > 0 {
                gen_warnings.push(format!(
                    "Dropped {dropped_sector} BUY(s) for sector diversification cap ({MAX_BUYS_PER_SECTOR}/sector)."
                ));
            }
            if dropped_symbol > 0 {
                let sample = if drop_symbol_samples.is_empty() {
                    String::new()
                } else {
                    format!(" (e.g. {})", drop_symbol_samples.join(", "))
                };
                gen_warnings.push(format!(
                    "Dropped {dropped_symbol} LLM signal(s) whose tickers were not in the universe{sample}."
                ));
            }
        } else {
            gen_warnings.push("LLM response had no signals array.".into());
        }
        Ok(())
        })?;

        Ok(PortfolioGeneration {
            signal_ids,
            universe_size,
            warnings: gen_warnings,
            buys_disabled: capacity.buys_disabled,
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

    /// Persist a Coach-proposed trade (execution still goes through process_signals).
    pub(crate) fn persist_coach_trade(
        conn: &Connection,
        symbol: &str,
        action: &str,
        rationale: &str,
        quantity: f64,
    ) -> Result<Option<String>> {
        let pick = json!({
            "action": action,
            "confidence": 0.9,
            "rationale": rationale,
        });
        let id = Self::persist_signal(
            conn,
            &pick,
            symbol,
            "coach",
            rationale,
            "coach:propose",
            PORTFOLIO_PROMPT_VERSION,
            false,
        )?;
        if let Some(ref sid) = id {
            let snap = json!({ "coachQty": quantity.max(0.0) });
            conn.execute(
                "UPDATE signals SET technical_snapshot = ?1 WHERE id = ?2",
                rusqlite::params![snap.to_string(), sid],
            )?;
        }
        Ok(id)
    }

    /// Persist a rules-engine SELL (stop-loss / take-profit) without an LLM log payload.
    pub(crate) fn persist_rule_sell(
        conn: &Connection,
        symbol: &str,
        model_name: &str,
        rationale: &str,
        confidence: f64,
        sell_fraction: f64,
    ) -> Result<Option<String>> {
        let pick = json!({
            "action": "SELL",
            "confidence": confidence,
            "rationale": rationale,
        });
        let id = Self::persist_signal(
            conn,
            &pick,
            symbol,
            "",
            "",
            model_name,
            PORTFOLIO_PROMPT_VERSION,
            false,
        )?;
        if let Some(ref sid) = id {
            let snap = json!({ "sellFraction": sell_fraction.clamp(0.1, 1.0) });
            conn.execute(
                "UPDATE signals SET technical_snapshot = ?1 WHERE id = ?2",
                rusqlite::params![snap.to_string(), sid],
            )?;
        }
        Ok(id)
    }

    pub(crate) fn get_active_param_set(
        conn: &Connection,
        portfolio_id: Option<&str>,
    ) -> Result<Option<ParamSetRow>> {
        let map_row = |row: &rusqlite::Row| {
            Ok(ParamSetRow {
                id: row.get(0)?,
                max_daily_trades: row.get(1)?,
                stop_loss_pct: row.get(2)?,
                take_profit_pct: row.get(3)?,
                min_confidence_to_trade: row.get(4)?,
                max_position_pct: row.get(5)?,
                position_size_pct: row.get(6)?,
                cycle_budget_pct: row.get(7)?,
                max_daily_drawdown_pct: row.get(8)?,
                time_stop_hours: row.get::<_, Option<f64>>(9)?.unwrap_or(24.0),
                partial_tp_fraction: row.get::<_, Option<f64>>(10)?.unwrap_or(1.0),
            })
        };
        let cols = "s.id, s.max_daily_trades, s.stop_loss_pct, s.take_profit_pct,
                    s.min_confidence_to_trade, s.max_position_pct, s.position_size_pct,
                    s.cycle_budget_pct, s.max_daily_drawdown_pct,
                    s.time_stop_hours, s.partial_tp_fraction";
        if let Some(pid) = portfolio_id {
            let row: Option<ParamSetRow> = conn
                .query_row(
                    &format!(
                        "SELECT {cols}
                         FROM strategy_param_sets s
                         JOIN sandbox_portfolios p ON p.strategy_param_set_id = s.id
                         WHERE p.id = ?1 AND s.is_active = 1 LIMIT 1"
                    ),
                    [pid],
                    map_row,
                )
                .ok();
            if row.is_some() {
                return Ok(row);
            }
        }

        conn.query_row(
            &format!(
                "SELECT id, max_daily_trades, stop_loss_pct, take_profit_pct,
                        min_confidence_to_trade, max_position_pct, position_size_pct,
                        cycle_budget_pct, max_daily_drawdown_pct,
                        time_stop_hours, partial_tp_fraction
                 FROM strategy_param_sets WHERE is_active = 1 LIMIT 1"
            ),
            [],
            map_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn build_universe(
        conn: &Connection,
        held_symbols: &std::collections::HashSet<String>,
        recent_symbols: &std::collections::HashSet<String>,
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
        let out: Vec<UniverseRow> = rows
            .filter_map(|r| r.ok())
            .filter(|r| r.price.is_some())
            .collect();
        Ok(shape_universe_for_llm(out, held_symbols, recent_symbols))
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

    pub(crate) fn sandbox_lots(conn: &Connection, portfolio_id: Option<&str>) -> Result<Vec<HeldLot>> {
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
            "SELECT p.symbol, p.quantity, p.avg_cost,
                    (SELECT MIN(t.executed_at) FROM sandbox_trades t
                      WHERE t.portfolio_id = p.portfolio_id AND t.symbol = p.symbol AND t.side = 'BUY')
             FROM sandbox_positions p WHERE p.portfolio_id = ?1 AND p.quantity > 0",
        )?;
        let rows = stmt.query_map([&pid], |row| {
            Ok(HeldLot {
                symbol: row.get(0)?,
                quantity: row.get(1)?,
                avg_cost: row.get(2)?,
                last_price: None,
                opened_at: row.get(3)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub(crate) fn attach_live_opened_at(conn: &Connection, lots: &mut [HeldLot]) {
        for lot in lots {
            if lot.opened_at.is_some() {
                continue;
            }
            lot.opened_at = conn
                .query_row(
                    "SELECT MIN(created_at) FROM broker_orders
                     WHERE UPPER(symbol) = UPPER(?1) AND UPPER(side) = 'BUY'
                       AND LOWER(status) IN ('executed', 'filled')",
                    [&lot.symbol],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten();
        }
    }

    pub(crate) fn last_db_price(conn: &Connection, symbol: &str) -> Option<f64> {
        conn.query_row(
            "SELECT price FROM price_history WHERE symbol = ?1 ORDER BY trade_date DESC LIMIT 1",
            [symbol],
            |row| row.get(0),
        )
        .ok()
        .filter(|p: &f64| *p > 0.0)
    }

    fn recently_sold_symbols(
        conn: &Connection,
        hours: i64,
    ) -> Result<std::collections::HashSet<String>> {
        let cutoff = format!("-{hours} hours");
        let mut set = std::collections::HashSet::new();
        let mut sandbox = conn.prepare(
            "SELECT DISTINCT UPPER(symbol) FROM sandbox_trades
             WHERE side = 'SELL' AND executed_at >= datetime('now', ?1)",
        )?;
        for row in sandbox.query_map([&cutoff], |row| row.get::<_, String>(0))? {
            set.insert(row?);
        }
        let mut broker = conn.prepare(
            "SELECT DISTINCT UPPER(symbol) FROM broker_orders
             WHERE UPPER(side) = 'SELL'
               AND created_at >= datetime('now', ?1)
               AND LOWER(status) NOT IN ('rejected', 'cancelled', 'canceled')",
        )?;
        for row in broker.query_map([&cutoff], |row| row.get::<_, String>(0))? {
            set.insert(row?);
        }
        Ok(set)
    }

    fn queued_unexecuted_buy_symbols(
        conn: &Connection,
    ) -> Result<std::collections::HashSet<String>> {
        let mut set = std::collections::HashSet::new();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT UPPER(symbol) FROM signals
             WHERE executed = 0 AND UPPER(action) = 'BUY'
               AND generated_at >= datetime('now', '-24 hours')",
        )?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
            set.insert(row?);
        }
        Ok(set)
    }

    fn pending_unexecuted_for_prompt(conn: &Connection) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT UPPER(symbol), action, COALESCE(risk_policy_result, ''), generated_at
             FROM signals
             WHERE executed = 0 AND UPPER(action) IN ('BUY', 'SELL')
               AND generated_at >= datetime('now', '-24 hours')
             ORDER BY generated_at DESC
             LIMIT 20",
        )?;
        for row in stmt.query_map([], |row| {
            Ok(json!({
                "symbol": row.get::<_, String>(0)?,
                "action": row.get::<_, String>(1)?,
                "status": row.get::<_, String>(2)?,
                "generatedAt": row.get::<_, String>(3)?,
            }))
        })? {
            out.push(row?);
        }
        Ok(out)
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

pub(crate) struct ParamSetRow {
    pub id: String,
    pub max_daily_trades: i64,
    pub stop_loss_pct: f64,
    pub take_profit_pct: Option<f64>,
    pub min_confidence_to_trade: f64,
    pub max_position_pct: f64,
    pub position_size_pct: f64,
    pub cycle_budget_pct: f64,
    pub max_daily_drawdown_pct: f64,
    pub time_stop_hours: f64,
    pub partial_tp_fraction: f64,
}

impl ParamSetRow {
    fn to_param_set(&self) -> crate::execution::ParamSet {
        crate::execution::ParamSet {
            id: self.id.clone(),
            max_position_pct: self.max_position_pct,
            max_daily_trades: self.max_daily_trades,
            stop_loss_pct: self.stop_loss_pct,
            min_confidence_to_trade: self.min_confidence_to_trade,
            max_daily_drawdown_pct: self.max_daily_drawdown_pct,
            position_size_pct: self.position_size_pct,
            cycle_budget_pct: self.cycle_budget_pct,
        }
    }
}

fn compute_trade_capacity(
    param_set: &ParamSetRow,
    _risk_params: &crate::execution::ParamSet,
    cash: f64,
    _total_equity: f64,
    daily_drawdown: f64,
    fee_pct: f64,
    universe: &[UniverseRow],
    _positions: &[(String, f64, f64)],
    held_symbols: &std::collections::HashSet<String>,
    seen: &std::collections::HashSet<String>,
    min_order_notional: f64,
    remaining_budget: f64,
    halt_new_buys: bool,
) -> TradeCapacity {
    let drawdown_ok = daily_drawdown < param_set.max_daily_drawdown_pct;
    let capital_reason =
        crate::execution::buy_ineligible_reason(cash, remaining_budget, min_order_notional);
    let cycle_budget = remaining_budget;
    let can_afford = if capital_reason.is_some() || cycle_budget <= 0.0 {
        false
    } else {
        universe.iter().any(|row| {
            let price = row.price.unwrap_or(0.0);
            if price <= 0.0 {
                return false;
            }
            if min_order_notional > 0.0 {
                cash + 1e-9 >= min_order_notional
                    && cycle_budget + 1e-9 >= min_order_notional
                    && crate::execution::RiskPolicyService::can_afford_one_share(cash, price, fee_pct)
            } else {
                crate::execution::RiskPolicyService::can_afford_one_share(
                    cycle_budget,
                    price,
                    fee_pct,
                )
            }
        })
    };
    let buy_allowed =
        drawdown_ok && can_afford && cash > 0.0 && capital_reason.is_none() && !halt_new_buys;
    let sell_allowed = held_symbols.iter().any(|s| !seen.contains(s));
    let max_buy_signals = if buy_allowed {
        crate::execution::max_buys_for_min_notional(
            cycle_budget,
            min_order_notional,
            crate::execution::MAX_SIGNAL_BUYS.min(LLM_SIGNAL_CAP),
        )
    } else {
        0
    };
    let remaining_buy_slots = max_buy_signals as i64;
    let max_sell_signals = if sell_allowed {
        held_symbols
            .iter()
            .filter(|s| !seen.contains(*s))
            .count()
            .min(LLM_SIGNAL_CAP)
            .min(crate::execution::MAX_SIGNAL_SELLS)
    } else {
        0
    };
    let max_actions = (max_buy_signals + max_sell_signals).min(LLM_SIGNAL_CAP).max(0);
    let buys_disabled = !buy_allowed;
    let skip_llm = !buy_allowed && !sell_allowed;
    let skip_reason = if skip_llm {
        let mut reasons = Vec::new();
        if !drawdown_ok {
            reasons.push("daily drawdown limit");
        } else if let Some(reason) = capital_reason {
            reasons.push(reason);
        } else if !can_afford {
            if min_order_notional > 0.0 {
                reasons.push("cash below broker minimum order");
            } else {
                reasons.push("cycle cash budget cannot fund a whole share");
            }
        }
        if !sell_allowed {
            reasons.push("no discretionary SELLs available");
        }
        Some(format!(
            "Skipped LLM: no buy/sell capacity ({})",
            reasons.join(", ")
        ))
    } else {
        None
    };

    TradeCapacity {
        remaining_buy_slots,
        buy_allowed,
        sell_allowed,
        max_buy_signals,
        max_sell_signals,
        max_actions,
        buys_disabled,
        min_confidence: param_set.min_confidence_to_trade,
        skip_llm,
        skip_reason,
        capital_reason,
    }
}

fn keep_unheld_for_llm(held: bool, overbought: bool) -> bool {
    held || !overbought
}

pub fn list_recent_cycle_audits(conn: &Connection, limit: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT cycle_id, summary, blocked_histogram, cash, executed_ids, created_at, detail
         FROM cycle_audits ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit.max(1)], |row| {
        Ok(json!({
            "cycleId": row.get::<_, Option<String>>(0)?,
            "summary": row.get::<_, String>(1)?,
            "blockedHistogram": row.get::<_, Option<String>>(2)?,
            "cash": row.get::<_, Option<f64>>(3)?,
            "executedIds": row.get::<_, Option<String>>(4)?,
            "createdAt": row.get::<_, String>(5)?,
            "detail": row.get::<_, Option<String>>(6)?,
        }))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub(crate) fn persist_cycle_audit(
    conn: &Connection,
    cycle_id: Option<&str>,
    generated_count: usize,
    executed: i64,
    trading_mode: &str,
    universe_size: usize,
    warning: Option<&str>,
    signal_ids: &[String],
    cash: Option<f64>,
) -> Result<()> {
    conn.execute(
        "DELETE FROM cycle_audits WHERE expires_at < datetime('now')",
        [],
    )?;
    let mut hist: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    let mut executed_ids: Vec<String> = Vec::new();
    for id in signal_ids {
        let result: String = conn
            .query_row(
                "SELECT COALESCE(risk_policy_result, 'UNSET') FROM signals WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "UNSET".into());
        *hist.entry(result).or_insert(0) += 1;
        let done: i64 = conn
            .query_row(
                "SELECT COALESCE(executed, 0) FROM signals WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if done == 1 {
            executed_ids.push(id.clone());
        }
    }
    let histogram = serde_json::to_string(&hist).unwrap_or_else(|_| "{}".into());
    let executed_json = serde_json::to_string(&executed_ids).unwrap_or_else(|_| "[]".into());
    let summary = format!(
        "signals={} executed={} venue={} universe={}{}",
        generated_count,
        executed,
        trading_mode,
        universe_size,
        warning.map(|w| format!(" warn={w}")).unwrap_or_default()
    );
    let detail = json!({
        "blockedHistogram": hist,
        "cash": cash,
        "executedIds": executed_ids,
        "signalIds": signal_ids,
        "generated": generated_count,
        "executed": executed,
        "venue": trading_mode,
        "universeSize": universe_size,
    })
    .to_string();
    conn.execute(
        "INSERT INTO cycle_audits (id, cycle_id, summary, expires_at, detail, blocked_histogram, cash, executed_ids)
         VALUES (?1, ?2, ?3, datetime('now', '+90 days'), ?4, ?5, ?6, ?7)",
        rusqlite::params![
            Uuid::new_v4().to_string(),
            cycle_id.unwrap_or(""),
            summary,
            detail,
            histogram,
            cash,
            executed_json,
        ],
    )?;
    Ok(())
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

/// Two-pass BUY index selection: sector cap first, then backfill to `min_buys`.
fn plan_buy_accept_indices(
    candidates: &[(String, String)],
    buy_cap: usize,
    min_buys: usize,
    max_per_sector: usize,
) -> (Vec<usize>, usize) {
    let mut accepted: Vec<usize> = Vec::new();
    let mut deferred: Vec<usize> = Vec::new();
    let mut sector_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (i, (_sym, sec)) in candidates.iter().enumerate() {
        if accepted.len() >= buy_cap {
            break;
        }
        let count = sector_counts.get(sec).copied().unwrap_or(0);
        if count >= max_per_sector {
            deferred.push(i);
            continue;
        }
        *sector_counts.entry(sec.clone()).or_insert(0) += 1;
        accepted.push(i);
    }

    let mut dropped_sector = 0usize;
    for i in deferred {
        if accepted.len() >= buy_cap || accepted.len() >= min_buys {
            dropped_sector += 1;
            continue;
        }
        let sec = &candidates[i].1;
        *sector_counts.entry(sec.clone()).or_insert(0) += 1;
        accepted.push(i);
    }
    (accepted, dropped_sector)
}

fn held_sector_counts_json(
    lots: &[HeldLot],
    symbol_sectors: &std::collections::HashMap<String, String>,
) -> Value {
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for lot in lots {
        let sec = symbol_sectors
            .get(&lot.symbol.to_uppercase())
            .cloned()
            .unwrap_or_else(|| "?".into());
        *counts.entry(sec).or_insert(0) += 1;
    }
    json!(counts)
}

fn mover_rank_key(row: &UniverseRow) -> (f64, i64) {
    (
        row.change_percent.unwrap_or(0.0).abs(),
        row.volume.unwrap_or(0),
    )
}

/// Held first, then sector round-robin of movers (recent tickers soft-downranked within sector).
fn shape_universe_for_llm(
    rows: Vec<UniverseRow>,
    held_symbols: &std::collections::HashSet<String>,
    recent_symbols: &std::collections::HashSet<String>,
) -> Vec<UniverseRow> {
    let mut held_rows = Vec::new();
    let mut rest = Vec::new();
    for row in rows {
        if held_symbols.contains(&row.symbol.to_uppercase()) {
            held_rows.push(row);
        } else {
            rest.push(row);
        }
    }
    held_rows.sort_by(|a, b| {
        let (ac, av) = mover_rank_key(a);
        let (bc, bv) = mover_rank_key(b);
        bc.partial_cmp(&ac)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| bv.cmp(&av))
    });

    let mut by_sector: std::collections::BTreeMap<String, Vec<UniverseRow>> =
        std::collections::BTreeMap::new();
    for row in rest {
        let sec = sector_code(row.sector.as_deref());
        by_sector.entry(sec).or_default().push(row);
    }
    for queue in by_sector.values_mut() {
        queue.sort_by(|a, b| {
            let ar = recent_symbols.contains(&a.symbol.to_uppercase());
            let br = recent_symbols.contains(&b.symbol.to_uppercase());
            // Non-recent first (false < true), then |pct| desc, volume desc.
            ar.cmp(&br).then_with(|| {
                let (ac, av) = mover_rank_key(a);
                let (bc, bv) = mover_rank_key(b);
                bc.partial_cmp(&ac)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| bv.cmp(&av))
            })
        });
    }

    let mut shaped = held_rows;
    let mut queues: Vec<std::collections::VecDeque<UniverseRow>> = by_sector
        .into_values()
        .map(std::collections::VecDeque::from)
        .collect();
    loop {
        let mut progressed = false;
        for queue in &mut queues {
            if let Some(row) = queue.pop_front() {
                shaped.push(row);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    shaped
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
    let do_execute = settings.should_execute(trading_mode);

    let mut live_snap: Option<crate::wealth::WealthPortfolioSnapshot> = None;
    let (cash_for_agent, trading_venue, live_holdings) =
        if trading_mode == crate::wealth::TradingMode::Live {
            if let Some(session) = broker {
                if do_execute {
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
                if let Some(ref snap) = live_snap {
                    let mv = snap.holdings.iter().map(|h| h.current_value).sum::<f64>();
                    let _ = db.with_conn(|conn| {
                        crate::portfolio::EquityCurveService::insert_point(
                            conn,
                            venue,
                            snap.balance + mv,
                            snap.balance,
                            mv,
                        )
                    });
                }
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
    warnings.extend(generated.warnings);

    let mut signal_ids = generated.signal_ids.clone();
    if trading_mode == crate::wealth::TradingMode::Live {
        let extra = db.with_conn(|conn| {
            crate::execution::retryable_unexecuted_signal_ids(
                conn,
                24,
                crate::execution::retry_slot_cap(settings.max_live_actions.max(1) as usize),
                !generated.buys_disabled,
            )
        })?;
        let generated_set: std::collections::HashSet<String> =
            signal_ids.iter().cloned().collect();
        let retried: Vec<String> = extra
            .into_iter()
            .filter(|id| !generated_set.contains(id))
            .collect();
        if !retried.is_empty() {
            warnings.push(format!(
                "Retrying {} unexecuted signal(s) from earlier cycles.",
                retried.len()
            ));
            let mut merged = retried;
            merged.extend(signal_ids);
            signal_ids = merged;
        }
    }

    if generated.universe_size > 0 && generated.universe_size <= 20 {
        warnings.push(format!(
            "Universe is only {} names (seed size). Pulse ingest may be incomplete.",
            generated.universe_size
        ));
    }

    let live_market_open = if trading_mode == crate::wealth::TradingMode::Live {
        match broker {
            Some(session) => {
                crate::runtime_util::live_broker_market_open(session, calendar).await
            }
            None => false,
        }
    } else {
        true
    };

    let skip_result = settings.cycle_skip_result(trading_mode);

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
        skip_result,
    )
    .await?;
    warnings.extend(exec_warnings);

    if trading_mode == crate::wealth::TradingMode::Sandbox {
        db.with_conn(|conn| crate::portfolio::DailySnapshotService::create_snapshot(conn, cache, None))?;
    } else if do_execute {
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
        persist_cycle_audit(
            conn,
            cycle_id,
            generated.signal_ids.len(),
            executed,
            trading_mode.as_str(),
            generated.universe_size,
            warnings.first().map(String::as_str),
            &signal_ids,
            cash_for_agent,
        )
    });

    Ok(json!({
        "signals": generated.signal_ids.len(),
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

fn buy_blocked_by_reentry_cooldown(
    symbol: &str,
    recently_sold: &std::collections::HashSet<String>,
) -> bool {
    recently_sold.contains(&symbol.to_uppercase())
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

    #[test]
    fn shape_universe_round_robins_sectors_after_held() {
        let held = held(&["HOLD1"]);
        let recent = HashSet::new();
        let rows = vec![
            UniverseRow {
                symbol: "HOLD1".into(),
                sector: Some("Financial Services".into()),
                price: Some(10.0),
                change_percent: Some(0.1),
                volume: Some(100),
            },
            UniverseRow {
                symbol: "FIN_A".into(),
                sector: Some("Financial Services".into()),
                price: Some(10.0),
                change_percent: Some(5.0),
                volume: Some(9_000),
            },
            UniverseRow {
                symbol: "FIN_B".into(),
                sector: Some("Financial Services".into()),
                price: Some(10.0),
                change_percent: Some(4.0),
                volume: Some(8_000),
            },
            UniverseRow {
                symbol: "ICT_A".into(),
                sector: Some("ICT".into()),
                price: Some(10.0),
                change_percent: Some(3.0),
                volume: Some(7_000),
            },
            UniverseRow {
                symbol: "CG_A".into(),
                sector: Some("Consumer Goods".into()),
                price: Some(10.0),
                change_percent: Some(2.0),
                volume: Some(6_000),
            },
        ];
        let shaped = shape_universe_for_llm(rows, &held, &recent);
        assert_eq!(shaped[0].symbol, "HOLD1");
        let top_movers: Vec<&str> = shaped.iter().skip(1).take(3).map(|r| r.symbol.as_str()).collect();
        // Round-robin across FIN, ICT, CG (BTreeMap key order: CG, FIN, ICT)
        assert!(top_movers.contains(&"FIN_A"));
        assert!(top_movers.contains(&"ICT_A"));
        assert!(top_movers.contains(&"CG_A"));
        assert!(!top_movers.contains(&"FIN_B"));
    }

    #[test]
    fn shape_universe_downranks_recent_within_sector() {
        let held_syms = HashSet::new();
        let recent = held(&["FIN_HOT"]);
        let rows = vec![
            UniverseRow {
                symbol: "FIN_HOT".into(),
                sector: Some("Financial Services".into()),
                price: Some(10.0),
                change_percent: Some(9.0),
                volume: Some(99_000),
            },
            UniverseRow {
                symbol: "FIN_FRESH".into(),
                sector: Some("Financial Services".into()),
                price: Some(10.0),
                change_percent: Some(1.0),
                volume: Some(1_000),
            },
            UniverseRow {
                symbol: "ICT_A".into(),
                sector: Some("ICT".into()),
                price: Some(10.0),
                change_percent: Some(2.0),
                volume: Some(2_000),
            },
        ];
        let shaped = shape_universe_for_llm(rows, &held_syms, &recent);
        // Within FIN bucket, non-recent comes before recent despite lower |pct|.
        let fin_order: Vec<&str> = shaped
            .iter()
            .filter(|r| r.symbol.starts_with("FIN_"))
            .map(|r| r.symbol.as_str())
            .collect();
        assert_eq!(fin_order, vec!["FIN_FRESH", "FIN_HOT"]);
    }

    #[test]
    fn plan_buy_accept_respects_sector_cap_then_backfills() {
        let candidates = vec![
            ("A1".into(), "FIN".into()),
            ("A2".into(), "FIN".into()),
            ("A3".into(), "FIN".into()),
            ("A4".into(), "FIN".into()),
            ("B1".into(), "ICT".into()),
        ];
        let (idx, dropped) = plan_buy_accept_indices(&candidates, 5, 4, 2);
        // Pass A: A1,A2,B1; Pass B: A3 to reach min 4; A4 dropped
        assert_eq!(idx, vec![0, 1, 4, 2]);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn plan_buy_accept_no_backfill_when_min_already_met() {
        let candidates = vec![
            ("A1".into(), "FIN".into()),
            ("A2".into(), "FIN".into()),
            ("A3".into(), "FIN".into()),
            ("B1".into(), "ICT".into()),
            ("C1".into(), "CG".into()),
        ];
        let (idx, dropped) = plan_buy_accept_indices(&candidates, 8, 3, 2);
        assert_eq!(idx, vec![0, 1, 3, 4]);
        assert_eq!(dropped, 1); // A3 deferred and not needed
    }

    #[test]
    fn buy_blocked_by_reentry_cooldown_matches_sold_set() {
        let sold = held(&["HONYFLOUR", "TIP"]);
        assert!(buy_blocked_by_reentry_cooldown("HONYFLOUR", &sold));
        assert!(buy_blocked_by_reentry_cooldown("tip", &sold));
        assert!(!buy_blocked_by_reentry_cooldown("GTCO", &sold));
        assert!(!buy_blocked_by_reentry_cooldown("GTCO", &HashSet::new()));
    }

    fn sample_param_row() -> ParamSetRow {
        ParamSetRow {
            id: "t".into(),
            max_daily_trades: 5,
            stop_loss_pct: 0.08,
            take_profit_pct: Some(0.2),
            min_confidence_to_trade: 0.65,
            max_position_pct: 0.25,
            position_size_pct: 0.05,
            cycle_budget_pct: 0.20,
            max_daily_drawdown_pct: 0.05,
            time_stop_hours: 24.0,
            partial_tp_fraction: 1.0,
        }
    }

    fn remaining_for(cash: f64, min_n: f64) -> f64 {
        crate::execution::remaining_buy_budget(cash, sample_param_row().cycle_budget_pct, min_n)
    }

    #[test]
    fn capacity_skips_llm_when_no_cash_and_no_holds() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let cap = compute_trade_capacity(
            &row,
            &risk,
            0.0,
            0.0,
            0.0,
            0.0015,
            &universe,
            &[],
            &HashSet::new(),
            &HashSet::new(),
            0.0,
            remaining_for(0.0, 0.0),
            false,
        );
        assert!(cap.skip_llm);
        assert!(!cap.buy_allowed);
        assert!(!cap.sell_allowed);
        assert_eq!(cap.max_buy_signals, 0);
    }

    #[test]
    fn capacity_blocks_buys_when_drawdown_hit_but_allows_sells() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let held = held(&["GTCO"]);
        let cap = compute_trade_capacity(
            &row,
            &risk,
            1_000_000.0,
            1_000_000.0,
            row.max_daily_drawdown_pct, // at/over drawdown limit
            0.0015,
            &universe,
            &[("GTCO".into(), 100.0, 40.0)],
            &held,
            &HashSet::new(),
            0.0,
            remaining_for(1_000_000.0, 0.0),
            false,
        );
        assert!(!cap.skip_llm);
        assert!(!cap.buy_allowed);
        assert!(cap.sell_allowed);
        assert!(cap.buys_disabled);
        assert_eq!(cap.max_buy_signals, 0);
        assert_eq!(cap.max_sell_signals, 1);
        assert_eq!(cap.remaining_buy_slots, 0);
    }

    #[test]
    fn capacity_allows_buy_when_cash_funds_whole_share() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let cap = compute_trade_capacity(
            &row,
            &risk,
            1_000_000.0,
            1_000_000.0,
            0.0,
            0.0015,
            &universe,
            &[],
            &HashSet::new(),
            &HashSet::new(),
            0.0,
            remaining_for(1_000_000.0, 0.0),
            false,
        );
        assert!(!cap.skip_llm);
        assert!(cap.buy_allowed);
        assert_eq!(
            cap.remaining_buy_slots,
            crate::execution::MAX_SIGNAL_BUYS as i64
        );
        assert_eq!(cap.max_buy_signals, crate::execution::MAX_SIGNAL_BUYS);
    }

    #[test]
    fn capacity_blocks_buys_when_cycle_budget_cannot_fund_share() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        // Cash 200: 20% budget = 40 < one share at 50
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let cap = compute_trade_capacity(
            &row,
            &risk,
            200.0,
            200.0,
            0.0,
            0.0015,
            &universe,
            &[],
            &HashSet::new(),
            &HashSet::new(),
            0.0,
            remaining_for(200.0, 0.0),
            false,
        );
        assert!(cap.skip_llm);
        assert!(!cap.buy_allowed);
        assert!(cap
            .skip_reason
            .as_deref()
            .unwrap_or("")
            .contains("cycle cash budget"));
    }

    #[test]
    fn bamboo_min_notional_limits_buy_count() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let cap = compute_trade_capacity(
            &row,
            &risk,
            16_750.0,
            16_750.0,
            0.0,
            0.0015,
            &universe,
            &[],
            &HashSet::new(),
            &HashSet::new(),
            crate::execution::BAMBOO_MIN_ORDER_NOTIONAL,
            remaining_for(16_750.0, crate::execution::BAMBOO_MIN_ORDER_NOTIONAL),
            false,
        );
        assert!(cap.buy_allowed);
        assert_eq!(cap.max_buy_signals, 1);
        assert!(!cap.skip_llm);
        assert!(cap.capital_reason.is_none());
    }

    #[test]
    fn capacity_blocks_buys_when_cash_below_bamboo_minimum() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let cash = 4_000.0;
        let min_n = crate::execution::BAMBOO_MIN_ORDER_NOTIONAL;
        let cap = compute_trade_capacity(
            &row,
            &risk,
            cash,
            cash,
            0.0,
            0.0015,
            &universe,
            &[],
            &HashSet::new(),
            &HashSet::new(),
            min_n,
            remaining_for(cash, min_n),
            false,
        );
        assert!(!cap.buy_allowed);
        assert!(cap.buys_disabled);
        assert_eq!(cap.max_buy_signals, 0);
        assert!(cap.capital_reason.unwrap_or("").contains("minimum"));
    }

    #[test]
    fn capacity_allows_buys_from_leftover_cash() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let cash = 11_043.0;
        let min_n = crate::execution::BAMBOO_MIN_ORDER_NOTIONAL;
        let remaining = crate::execution::remaining_buy_budget(cash, 1.0, min_n);
        let cap = compute_trade_capacity(
            &row,
            &risk,
            cash,
            cash,
            0.0,
            0.0015,
            &universe,
            &[],
            &HashSet::new(),
            &HashSet::new(),
            min_n,
            remaining,
            false,
        );
        assert!(cap.buy_allowed);
        assert!(!cap.buys_disabled);
        assert_eq!(cap.max_buy_signals, 2);
        assert!(cap.capital_reason.is_none());
    }

    #[test]
    fn capacity_keeps_sells_when_capital_constrained() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let held = held(&["GTCO"]);
        let cash = 4_000.0;
        let min_n = crate::execution::BAMBOO_MIN_ORDER_NOTIONAL;
        let cap = compute_trade_capacity(
            &row,
            &risk,
            cash,
            cash,
            0.0,
            0.0015,
            &universe,
            &[("GTCO".into(), 100.0, 40.0)],
            &held,
            &HashSet::new(),
            min_n,
            remaining_for(cash, min_n),
            false,
        );
        assert!(!cap.skip_llm);
        assert!(cap.buys_disabled);
        assert!(cap.sell_allowed);
        assert_eq!(cap.max_sell_signals, 1);
        assert_eq!(cap.max_buy_signals, 0);
    }

    #[test]
    fn queued_unexecuted_buys_ignore_filled_and_sells() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE signals (
                id TEXT PRIMARY KEY,
                symbol TEXT,
                action TEXT,
                executed INTEGER,
                generated_at TEXT DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, executed) VALUES ('1', 'uacn', 'BUY', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, executed) VALUES ('2', 'GTCO', 'BUY', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, executed) VALUES ('3', 'ZENITHBANK', 'SELL', 0)",
            [],
        )
        .unwrap();
        let set = SignalGenerationService::queued_unexecuted_buy_symbols(&conn).unwrap();
        assert!(set.contains("UACN"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn pre_llm_filter_keeps_held_overbought_drops_unheld() {
        assert!(!keep_unheld_for_llm(false, true));
        assert!(keep_unheld_for_llm(true, true));
        assert!(keep_unheld_for_llm(false, false));
    }

    #[test]
    fn capacity_halts_buys_when_protective_only() {
        let row = sample_param_row();
        let risk = row.to_param_set();
        let universe = vec![UniverseRow {
            symbol: "GTCO".into(),
            sector: None,
            price: Some(50.0),
            change_percent: Some(1.0),
            volume: Some(1000),
        }];
        let held = held(&["GTCO"]);
        let cap = compute_trade_capacity(
            &row,
            &risk,
            1_000_000.0,
            1_000_000.0,
            0.0,
            0.0015,
            &universe,
            &[("GTCO".into(), 100.0, 40.0)],
            &held,
            &HashSet::new(),
            0.0,
            remaining_for(1_000_000.0, 0.0),
            true,
        );
        assert!(!cap.buy_allowed);
        assert!(cap.sell_allowed);
        assert!(cap.buys_disabled);
    }

    #[test]
    fn cycle_audit_persists_histogram_cash_and_90d_ttl() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE signals (
                id TEXT PRIMARY KEY, risk_policy_result TEXT, executed INTEGER
             );
             CREATE TABLE cycle_audits (
                id TEXT PRIMARY KEY, cycle_id TEXT, summary TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                expires_at TEXT NOT NULL, detail TEXT, blocked_histogram TEXT,
                cash REAL, executed_ids TEXT
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, risk_policy_result, executed) VALUES ('a', 'BLOCKED_CASH', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, risk_policy_result, executed) VALUES ('b', 'APPROVED', 1)",
            [],
        )
        .unwrap();
        persist_cycle_audit(
            &conn,
            Some("cyc-1"),
            2,
            1,
            "live",
            40,
            Some("warn"),
            &["a".into(), "b".into()],
            Some(4_200.0),
        )
        .unwrap();
        let (hist, cash, ids, ttl): (String, f64, String, i64) = conn
            .query_row(
                "SELECT blocked_histogram, cash, executed_ids,
                        CAST((julianday(expires_at) - julianday(created_at)) AS INTEGER)
                 FROM cycle_audits LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!(hist.contains("BLOCKED_CASH"));
        assert!((cash - 4_200.0).abs() < 1e-9);
        assert!(ids.contains("b"));
        assert!(ttl >= 80);
        let listed = list_recent_cycle_audits(&conn, 5).unwrap();
        assert_eq!(listed.len(), 1);
    }
}
