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
use crate::risk_exits::{evaluate_position_exits, ExitKind};
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
    if !crate::runtime_util::market_activity_allowed(&calendar) {
        return Ok(None);
    }

    let Some(_gate) = CycleGateGuard::acquire(state.clone()) else {
        tracing::debug!(target: "risk_monitor", "skipped — cycle gate held");
        return Ok(None);
    };

    let pulse_password = get_secret(SECRET_PULSE_PASSWORD)?;
    let pulse_api_key = get_secret(SECRET_PULSE_API_KEY)?;
    let client =
        crate::ngx::NgxPulseClient::from_settings(&settings, pulse_password, pulse_api_key);
    let broker = crate::broker::open_live_broker(&settings);

    block_on_local(async {
        let trading_mode = if let Some(ref session) = broker {
            session.resolve_trading_mode(&settings).await
        } else {
            TradingMode::Sandbox
        };

        let lots = match trading_mode {
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

        let candidates =
            evaluate_position_exits(&lots, param_set.stop_loss_pct, param_set.take_profit_pct, &prices);
        if candidates.is_empty() {
            return Ok(None);
        }

        let mut signal_ids = Vec::new();
        let mut exit_payloads = Vec::new();

        state.db.with_conn(|conn| {
            for exit in &candidates {
                let qty = lots
                    .iter()
                    .find(|l| l.symbol.eq_ignore_ascii_case(&exit.symbol))
                    .map(|l| l.quantity)
                    .unwrap_or(0.0);
                if qty <= 0.0 {
                    continue;
                }

                let pending = intents::pending_sell_qty(conn, &exit.symbol).unwrap_or(0.0);
                if pending > 0.0 {
                    tracing::debug!(
                        target: "risk_monitor",
                        symbol = %exit.symbol,
                        pending,
                        "skip exit — open sell intent"
                    );
                    continue;
                }

                if has_recent_unexecuted_rule_sell(conn, &exit.symbol)? {
                    tracing::debug!(
                        target: "risk_monitor",
                        symbol = %exit.symbol,
                        "skip exit — recent unexecuted rule sell already persisted"
                    );
                    continue;
                }

                if let Some(id) = SignalGenerationService::persist_rule_sell(
                    conn,
                    &exit.symbol,
                    exit.kind.model_name(),
                    &exit.rationale,
                    exit.confidence,
                )? {
                    signal_ids.push(id);
                    exit_payloads.push(serde_json::json!({
                        "symbol": exit.symbol,
                        "kind": match exit.kind {
                            ExitKind::StopLoss => "stop_loss",
                            ExitKind::TakeProfit => "take_profit",
                        },
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
                Some(session) => session.market_is_open().await.unwrap_or(false),
                None => false,
            }
        } else {
            true
        };

        // Persist always; execute when sandbox OR live-authorized (same gate as auto cycle).
        let live_authorized = trading_mode == TradingMode::Live
            && settings.live_trading_enabled
            && settings.scheduled_live_authorized
            && live_market_open;
        let do_execute = trading_mode == TradingMode::Sandbox || live_authorized;

        if trading_mode == TradingMode::Live && !do_execute {
            tracing::info!(
                target: "risk_monitor",
                signals = signal_ids.len(),
                live_trading_enabled = settings.live_trading_enabled,
                scheduled_live_authorized = settings.scheduled_live_authorized,
                live_market_open,
                "risk exits persisted but live execution skipped (not authorized)"
            );
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
        )
        .await?;

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

fn has_recent_unexecuted_rule_sell(
    conn: &rusqlite::Connection,
    symbol: &str,
) -> anyhow::Result<bool> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM signals
             WHERE UPPER(symbol) = UPPER(?1)
               AND action = 'SELL'
               AND model_name IN ('rules:stop-loss', 'rules:take-profit')
               AND executed = 0
               AND generated_at >= datetime('now', '-1 hour')
             LIMIT 1",
            [symbol],
            |row| row.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}
