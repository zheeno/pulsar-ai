//! Continuous stop-loss / take-profit monitor (not bound to LLM cycles).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::OptionalExtension;
use tauri::{AppHandle, Emitter};
use tokio::time::MissedTickBehavior;

use crate::app_state::{AppState, CycleGateGuard};
use crate::calendar::TradingCalendar;
use crate::cache::CachedPrice;
use crate::intents;
use crate::runtime_util::block_on_local;
use crate::secrets::{get_secret, SECRET_PULSE_API_KEY, SECRET_PULSE_PASSWORD};
use crate::settings::get_settings;
use crate::signals::{HeldLot, SignalGenerationService};
use crate::wealth::TradingMode;

const RISK_MONITOR_SECS: u64 = 30;

static RISK_TICK_BUSY: AtomicBool = AtomicBool::new(false);

pub fn start_risk_monitor(app: AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(RISK_MONITOR_SECS));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // Skip the immediate first tick so launch ingest / onboarding settle.
        ticker.tick().await;

        loop {
            ticker.tick().await;

            if RISK_TICK_BUSY
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                continue;
            }

            let app2 = app.clone();
            let state2 = state.clone();
            let result = tokio::task::spawn_blocking(move || run_risk_tick(&app2, &state2)).await;
            RISK_TICK_BUSY.store(false, Ordering::SeqCst);

            match result {
                Ok(Ok(Some(summary))) => {
                    if summary.exits > 0 {
                        tracing::info!(
                            target: "risk_monitor",
                            exits = summary.exits,
                            executed = summary.executed,
                            "risk exits processed"
                        );
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => {
                    tracing::warn!(target: "risk_monitor", error = %e, "risk tick failed");
                }
                Err(e) => {
                    tracing::warn!(target: "risk_monitor", error = %e, "risk tick join failed");
                }
            }
        }
    });
}

struct RiskTickSummary {
    exits: usize,
    executed: i64,
}

fn run_risk_tick(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<Option<RiskTickSummary>> {
    let settings = state.db.with_conn(get_settings)?;
    if !settings.onboarding_complete {
        return Ok(None);
    }

    let calendar = TradingCalendar::default();
    let crypto_mode = crate::broker::busha_connected();
    if !crypto_mode && !crate::runtime_util::market_activity_allowed(&calendar) {
        return Ok(None);
    }

    let Some(_gate) = CycleGateGuard::acquire(state.clone()) else {
        tracing::debug!(target: "risk_monitor", "skipped — cycle gate held");
        return Ok(None);
    };

    let broker = crate::broker::open_live_broker(&settings);

    block_on_local(async {
        let trading_mode = if let Some(ref session) = broker {
            session.resolve_trading_mode(&settings).await
        } else {
            TradingMode::Sandbox
        };

        let mut live_cash = 0.0_f64;
        let mut lots = match trading_mode {
            TradingMode::Live => {
                if let Some(ref session) = broker {
                    let book = match session.refresh_book(&state.db).await {
                        Ok(b) => Some(b),
                        Err(e) => {
                            tracing::warn!(
                                target: "risk_monitor",
                                error = %e,
                                "could not refresh live holdings; using cached book"
                            );
                            session.load_book(&state.db).ok().flatten()
                        }
                    };
                    if let Some(ref b) = book {
                        live_cash = b.brokerage_balance;
                    }
                    book.map(|b| HeldLot::from_holdings(&b.holdings))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
            TradingMode::Sandbox => state
                .db
                .with_conn(|conn| SignalGenerationService::sandbox_lots(conn, None))?,
        };

        if lots.is_empty() {
            return Ok(None);
        }

        let symbols: Vec<String> = lots.iter().map(|l| l.symbol.clone()).collect();
        let mut prices: HashMap<String, f64> = HashMap::new();
        let today = calendar.today_wat();

        if crypto_mode {
            // Busha is the sole market-data source in crypto mode — never Pulse.
            match crate::busha::BushaClient::from_store() {
                Ok(busha) => match busha.pairs_ngn().await {
                    Ok(pairs) => {
                        let _ = state.db.with_conn(|conn| crate::busha::persist_pairs(conn, &pairs));
                        match state.db.with_conn(|conn| {
                            crate::busha::apply_sell_marks(
                                conn,
                                &state.cache,
                                &pairs,
                                &symbols,
                                &today,
                            )
                        }) {
                            Ok(marks) => {
                                prices.extend(marks);
                                tracing::debug!(
                                    target: "risk_monitor",
                                    n = prices.len(),
                                    "busha sell marks applied"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    target: "risk_monitor",
                                    error = %e,
                                    "failed to persist busha sell marks"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "risk_monitor",
                            error = %e,
                            "busha pairs refresh failed; falling back to cache/db"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        target: "risk_monitor",
                        error = %e,
                        "busha client unavailable for risk marks"
                    );
                }
            }
        } else {
            let pulse_password = get_secret(SECRET_PULSE_PASSWORD)?;
            let pulse_api_key = get_secret(SECRET_PULSE_API_KEY)?;
            let client = crate::ngx::NgxPulseClient::from_settings(
                &settings,
                pulse_password,
                pulse_api_key,
            );
            match client.get_latest_quotes(&symbols).await {
                Ok(quotes) => {
                    let _ = state.db.with_conn(|conn| {
                        for q in &quotes {
                            if q.last > 0.0 {
                                let _ = crate::ingest::IngestionService::upsert_last_quote(
                                    conn,
                                    &state.cache,
                                    q,
                                );
                                prices.insert(q.symbol.to_uppercase(), q.last);
                            }
                        }
                        Ok::<_, anyhow::Error>(())
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        target: "risk_monitor",
                        error = %e,
                        "pulse quote refresh failed; falling back to cache/db"
                    );
                }
            }
        }

        for lot in &lots {
            let key = lot.symbol.to_uppercase();
            if prices.contains_key(&key) {
                continue;
            }
            if let Some(cached) = state.cache.get_price(&lot.symbol) {
                if cached.price > 0.0 {
                    prices.insert(key, cached.price);
                    continue;
                }
            }
            if let Some(px) = lot.last_price.filter(|p| *p > 0.0) {
                prices.insert(key, px);
                continue;
            }
            if let Some(px) = state
                .db
                .with_conn(|conn| Ok(SignalGenerationService::last_db_price(conn, &lot.symbol)))
                .ok()
                .flatten()
            {
                prices.insert(key.clone(), px);
                state.cache.set_price(&CachedPrice {
                    symbol: lot.symbol.clone(),
                    price: px,
                    trade_date: calendar.today_wat(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                });
            } else {
                tracing::debug!(
                    target: "risk_monitor",
                    symbol = %lot.symbol,
                    "no price for holding; skipping"
                );
            }
        }

        let param_set = state
            .db
            .with_conn(|conn| SignalGenerationService::get_active_param_set(conn, None))?;
        let Some(param_set) = param_set else {
            return Ok(None);
        };

        if trading_mode == TradingMode::Live {
            state.db.with_conn(|conn| {
                crate::signals::SignalGenerationService::attach_live_opened_at(conn, &mut lots);
                Ok(())
            })?;
        }

        let flatten = settings.flatten_on_drawdown_armed
            && param_set.max_daily_drawdown_pct < 0.999
            && {
                let eq = live_cash
                    + lots
                        .iter()
                        .map(|l| {
                            let px = prices
                                .get(&l.symbol.to_uppercase())
                                .copied()
                                .or(l.last_price)
                                .unwrap_or(l.avg_cost);
                            l.quantity * px
                        })
                        .sum::<f64>();
                if trading_mode == TradingMode::Live {
                    if let Some(ref session) = broker {
                        state
                            .db
                            .with_conn(|conn| {
                                crate::execution::live_drawdown_pct(conn, session.id().as_str(), eq)
                            })
                            .unwrap_or(0.0)
                            >= param_set.max_daily_drawdown_pct
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

        let candidates = crate::risk_exits::evaluate_position_exits_full(
            &lots,
            crate::risk_exits::ExitParams {
                stop_loss_pct: param_set.stop_loss_pct,
                take_profit_pct: param_set.take_profit_pct,
                time_stop_hours: param_set.time_stop_hours,
                partial_tp_fraction: param_set.partial_tp_fraction,
            },
            &prices,
            flatten,
        );
        if candidates.is_empty() {
            return Ok(None);
        }

        if flatten {
            let n = candidates.len();
            let _ = state.db.with_conn(|conn| {
                crate::memory::insert_memory(
                    conn,
                    "freeform",
                    None,
                    &format!(
                        "ARMED flatten-on-drawdown fired: queued full SELL on {n} lot(s). This is off by default and requires an explicit Settings arm."
                    ),
                    "agent_upsert",
                    None,
                )
            });
        }

        let mut signal_ids = Vec::new();
        let mut exit_payloads = Vec::new();

        state.db.with_conn(|conn| {
            for exit in &candidates {
                let qty = crate::risk_exits::sell_qty_for_exit(
                    lots
                        .iter()
                        .find(|l| l.symbol.eq_ignore_ascii_case(&exit.symbol))
                        .map(|l| l.quantity)
                        .unwrap_or(0.0),
                    exit.sell_fraction,
                );
                if qty <= 0.0 {
                    continue;
                }

                let pending = intents::pending_sell_qty_on(
                    conn,
                    &exit.symbol,
                    broker.as_ref().map(|s| s.id().as_str()),
                )
                .unwrap_or(0.0);
                if pending > 0.0 {
                    tracing::debug!(
                        target: "risk_monitor",
                        symbol = %exit.symbol,
                        pending,
                        "skip exit — open sell intent"
                    );
                    continue;
                }

                if let Some(existing_id) = recent_unexecuted_rule_sell_id(conn, &exit.symbol)? {
                    if !signal_ids.iter().any(|id| id == &existing_id) {
                        tracing::info!(
                            target: "risk_monitor",
                            symbol = %exit.symbol,
                            signal_id = %existing_id,
                            "retrying unexecuted rule sell"
                        );
                        signal_ids.push(existing_id);
                        exit_payloads.push(serde_json::json!({
                            "symbol": exit.symbol,
                            "kind": exit.kind.hint(),
                            "rationale": exit.rationale,
                            "retry": true,
                        }));
                    }
                    continue;
                }

                if let Some(id) = SignalGenerationService::persist_rule_sell(
                    conn,
                    &exit.symbol,
                    exit.kind.model_name(),
                    &exit.rationale,
                    exit.confidence,
                    exit.sell_fraction,
                )? {
                    signal_ids.push(id);
                    exit_payloads.push(serde_json::json!({
                        "symbol": exit.symbol,
                        "kind": exit.kind.hint(),
                        "rationale": exit.rationale,
                    }));
                }
            }
            Ok::<_, anyhow::Error>(())
        })?;

        if signal_ids.is_empty() {
            return Ok(None);
        }

        let live_market_open = if trading_mode == TradingMode::Live {
            match broker.as_ref() {
                Some(session) => {
                    crate::runtime_util::live_broker_market_open(session, &calendar).await
                }
                None => false,
            }
        } else {
            true
        };

        let do_execute = settings.should_execute_risk_exits(trading_mode, live_market_open);
        let skip_result = settings.risk_exit_skip_result(trading_mode, live_market_open);

        if trading_mode == TradingMode::Live && !do_execute {
            tracing::info!(
                target: "risk_monitor",
                signals = signal_ids.len(),
                live_trading_enabled = settings.live_trading_enabled,
                live_market_open,
                skip_result,
                "risk exits persisted but live execution skipped"
            );
        }

        if do_execute && trading_mode == TradingMode::Live {
            if let Some(ref session) = broker {
                let _ = crate::execution::ExecutionService::reconcile_if_possible(
                    &state.db,
                    session,
                )
                .await;
            }
        }

        let (executed, warnings) = crate::execution::ExecutionService::process_signals(
            &state.db,
            &state.cache,
            &settings,
            &signal_ids,
            broker.as_ref(),
            trading_mode,
            live_market_open,
            do_execute,
            false,
            None,
            skip_result,
        )
        .await?;

        if !warnings.is_empty() {
            tracing::warn!(
                target: "risk_monitor",
                ?warnings,
                signals = signal_ids.len(),
                executed,
                do_execute,
                live_market_open,
                "risk exit execution warnings"
            );
        }

        if trading_mode == TradingMode::Sandbox && executed > 0 {
            let _ = state.db.with_conn(|conn| {
                crate::portfolio::DailySnapshotService::create_snapshot(conn, &state.cache, None)
            });
        }

        let label = if exit_payloads
            .iter()
            .any(|e| e.get("kind").and_then(|k| k.as_str()) == Some("stop_loss"))
        {
            "Stop-loss"
        } else if exit_payloads
            .iter()
            .any(|e| e.get("kind").and_then(|k| k.as_str()) == Some("time_stop"))
        {
            "Time-stop"
        } else if exit_payloads
            .iter()
            .any(|e| e.get("kind").and_then(|k| k.as_str()) == Some("flatten"))
        {
            "Flatten"
        } else {
            "Take-profit"
        };
        let symbols: Vec<&str> = exit_payloads
            .iter()
            .filter_map(|e| e.get("symbol").and_then(|s| s.as_str()))
            .collect();
        let _ = app.emit(
            "risk-exit",
            serde_json::json!({
                "ok": true,
                "exits": exit_payloads.len(),
                "executed": executed,
                "tradingMode": trading_mode.as_str(),
                "executedLive": do_execute && trading_mode == TradingMode::Live,
                "symbols": symbols,
                "label": label,
                "warnings": warnings,
                "items": exit_payloads,
            }),
        );

        Ok(Some(RiskTickSummary {
            exits: exit_payloads.len(),
            executed,
        }))
    })
}

pub(crate) fn recent_unexecuted_rule_sell_id(
    conn: &rusqlite::Connection,
    symbol: &str,
) -> anyhow::Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM signals
         WHERE UPPER(symbol) = UPPER(?1)
           AND action = 'SELL'
           AND model_name IN ('rules:stop-loss', 'rules:take-profit', 'rules:time-stop', 'rules:flatten')
           AND executed = 0
           AND generated_at >= datetime('now', '-24 hours')
         ORDER BY generated_at DESC
         LIMIT 1",
        [symbol],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use crate::db::Database;
    use crate::seed::SeedService;

    fn mem_with_signals() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE signals (
                id TEXT PRIMARY KEY,
                symbol TEXT,
                action TEXT,
                model_name TEXT,
                executed INTEGER,
                generated_at TEXT
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn recent_unexecuted_rule_sell_id_returns_latest() {
        let conn = mem_with_signals();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, model_name, executed, generated_at)
             VALUES ('old', 'NIDF', 'SELL', 'rules:stop-loss', 0, datetime('now', '-2 hours'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, model_name, executed, generated_at)
             VALUES ('new', 'NIDF', 'SELL', 'rules:stop-loss', 0, datetime('now', '-5 minutes'))",
            [],
        )
        .unwrap();
        let id = recent_unexecuted_rule_sell_id(&conn, "nidf").unwrap();
        assert_eq!(id.as_deref(), Some("new"));
    }

    #[test]
    fn recent_unexecuted_rule_sell_id_ignores_executed() {
        let conn = mem_with_signals();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, model_name, executed, generated_at)
             VALUES ('done', 'VFDGROUP', 'SELL', 'rules:take-profit', 1, datetime('now'))",
            [],
        )
        .unwrap();
        assert!(recent_unexecuted_rule_sell_id(&conn, "VFDGROUP")
            .unwrap()
            .is_none());
    }

    #[test]
    fn recent_unexecuted_rule_sell_id_respects_24h_window() {
        let conn = mem_with_signals();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, model_name, executed, generated_at)
             VALUES ('stale', 'NIDF', 'SELL', 'rules:stop-loss', 0, datetime('now', '-25 hours'))",
            [],
        )
        .unwrap();
        assert!(recent_unexecuted_rule_sell_id(&conn, "NIDF")
            .unwrap()
            .is_none());
    }

    fn test_db() -> (std::path::PathBuf, Database) {
        let dir = std::env::temp_dir().join(format!("pulsar-risk-{}", uuid::Uuid::new_v4()));
        let db = Database::open(&dir).expect("test db");
        db.with_conn(|conn| SeedService::seed_if_empty(conn, 10_000_000.0))
            .expect("seed");
        (dir, db)
    }

    fn insert_rule_sell(
        conn: &rusqlite::Connection,
        id: &str,
        symbol: &str,
        model: &str,
        executed: i64,
        generated_at_sql: &str,
    ) -> anyhow::Result<()> {
        conn.execute(
            &format!(
                "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed, generated_at)
                 VALUES (?1, ?2, 'SELL', 1.0, 'test', '{{}}', ?3, 'v2.4.0', 'BLOCKED_OTHER', ?4, datetime('now', '{generated_at_sql}'))"
            ),
            rusqlite::params![id, symbol, model, executed],
        )?;
        Ok(())
    }

    #[test]
    fn retries_unexecuted_stop_loss_and_take_profit_from_last_24h() {
        let (dir, db) = test_db();
        db.with_conn(|conn| {
            insert_rule_sell(conn, "old", "GTCO", "rules:stop-loss", 0, "-25 hours")?;
            insert_rule_sell(conn, "done", "GTCO", "rules:stop-loss", 1, "-1 hours")?;
            insert_rule_sell(conn, "sl", "GTCO", "rules:stop-loss", 0, "-2 hours")?;
            assert_eq!(recent_unexecuted_rule_sell_id(conn, "gtco")?.as_deref(), Some("sl"));

            insert_rule_sell(conn, "tp", "MTNN", "rules:take-profit", 0, "-3 hours")?;
            assert_eq!(recent_unexecuted_rule_sell_id(conn, "MTNN")?.as_deref(), Some("tp"));

            assert_eq!(recent_unexecuted_rule_sell_id(conn, "ZENITHBANK")?, None);
            Ok(())
        })
        .expect("retry selection");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retries_legacy_pending_confirm_unexecuted_sells() {
        let (dir, db) = test_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
                 VALUES ('legacy', 'GTCO', 'SELL', 1.0, 'stop', '{}', 'rules:stop-loss', 'v2.4.0', 'BLOCKED_PENDING_CONFIRM', 0)",
                [],
            )?;
            assert_eq!(
                recent_unexecuted_rule_sell_id(conn, "GTCO")?.as_deref(),
                Some("legacy")
            );
            Ok(())
        })
        .expect("legacy retry");
        let _ = std::fs::remove_dir_all(dir);
    }
}
