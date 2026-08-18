use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::app_state::{AppState, CycleGateGuard};
use crate::calendar::TradingCalendar;
use crate::ingest::IngestionService;
use crate::ngx::NgxPulseClient;
use crate::portfolio::PortfolioService;
use crate::rate_limit::RateLimiter;
use crate::runtime_util::block_on_local;
use crate::secrets::{delete_secret, get_secret, set_secret, SECRET_LLM_API_KEY, SECRET_PULSE_API_KEY, SECRET_PULSE_PASSWORD};
use crate::settings::{
    clear_session_settings, get_settings, mark_onboarding_complete, save_settings, AppSettings,
};
use crate::signals::{run_cycle, SignalGenerationService};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub settings: AppSettings,
    pub pulse_password: Option<String>,
    pub pulse_api_key: Option<String>,
    pub llm_api_key: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResponse {
    pub message: String,
    pub agent: serde_json::Value,
}

#[tauri::command]
pub async fn ping(state: State<'_, Arc<AppState>>) -> Result<PingResponse, String> {
    let agent = state.agent.ping().await.map_err(|e| e.to_string())?;
    Ok(PingResponse {
        message: "pong".into(),
        agent,
    })
}

#[tauri::command]
pub fn settings_get(state: State<'_, Arc<AppState>>) -> Result<AppSettings, String> {
    state.db.with_conn(get_settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn settings_set(payload: SettingsUpdate, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Some(pw) = payload.pulse_password {
        set_secret(SECRET_PULSE_PASSWORD, &pw).map_err(|e| e.to_string())?;
    }
    if let Some(key) = payload.pulse_api_key {
        if key.is_empty() {
            delete_secret(SECRET_PULSE_API_KEY).map_err(|e| e.to_string())?;
        } else {
            set_secret(SECRET_PULSE_API_KEY, &key).map_err(|e| e.to_string())?;
        }
    }
    let llm_key = payload.llm_api_key.clone();
    if let Some(key) = &llm_key {
        set_secret(SECRET_LLM_API_KEY, key).map_err(|e| e.to_string())?;
    }

    let mut settings = payload.settings;
    let current = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    settings.onboarding_complete = current.onboarding_complete;

    let new_url = settings.llm_base_url.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let old_url = current.llm_base_url.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(url) = new_url {
        let allow_custom = llm_key.as_ref().is_some_and(|k| !k.is_empty())
            && old_url.map(|u| u != url).unwrap_or(true);
        crate::net_policy::validate_llm_base_url(url, allow_custom || crate::net_policy::is_default_provider_host(
            reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.to_string())).as_deref().unwrap_or(""),
        ))
        .map_err(|e| e.to_string())?;
        if old_url.is_some() && old_url != new_url && llm_key.as_ref().is_none_or(|k| k.is_empty()) {
            return Err("Changing the LLM endpoint requires re-entering the API key".into());
        }
    }

    if settings.launch_at_login != current.launch_at_login {
        crate::launch_at_login::apply(settings.launch_at_login).map_err(|e| e.to_string())?;
    }

    state
        .db
        .with_conn(|conn| save_settings(conn, &settings))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn confidence_journal(state: State<'_, Arc<AppState>>) -> Result<Vec<serde_json::Value>, String> {
    state
        .db
        .with_conn(crate::outcomes::confidence_journal)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_cycle_audits(state: State<'_, Arc<AppState>>) -> Result<Vec<serde_json::Value>, String> {
    state
        .db
        .with_conn(|conn| crate::signals::list_recent_cycle_audits(conn, 30))
        .map_err(|e| e.to_string())
}

/// Clear Pulse + LLM secrets and session flags; keeps portfolio / market data.
#[tauri::command]
pub fn logout(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    // Best-effort keychain clears — missing entries should not block logout.
    let _ = delete_secret(SECRET_PULSE_PASSWORD);
    let _ = delete_secret(SECRET_PULSE_API_KEY);
    let _ = delete_secret(SECRET_LLM_API_KEY);
    crate::wealth::WealthClient::clear_local_secrets();
    state
        .db
        .with_conn(clear_session_settings)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_pulse_login(state: State<'_, Arc<AppState>>) -> Result<crate::ngx::PulseAuthReport, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(SECRET_PULSE_PASSWORD).map_err(|e| e.to_string())?;
        let api_key = get_secret(SECRET_PULSE_API_KEY).map_err(|e| e.to_string())?;
        let client = NgxPulseClient::from_settings(&settings, password, api_key);
        Ok(block_on_local(client.diagnose_login()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn test_llm(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    state.agent.test_llm(&settings).await.map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStatus {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub temperature: Option<f64>,
    pub configured: bool,
    pub masked_key: Option<String>,
}

fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "••••••••".into();
    }
    let prefix: String = chars.iter().take(3).collect();
    let suffix: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("{prefix}••••••••{suffix}")
}

/// Safe LLM credential summary for Settings (never returns the raw key).
#[tauri::command]
pub fn llm_status(state: State<'_, Arc<AppState>>) -> Result<LlmStatus, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    let key = get_secret(SECRET_LLM_API_KEY).map_err(|e| e.to_string())?;
    let configured = key.as_ref().is_some_and(|k| !k.is_empty());
    Ok(LlmStatus {
        provider: settings.llm_provider,
        model: settings.llm_model,
        base_url: settings.llm_base_url,
        temperature: settings.llm_temperature,
        configured,
        masked_key: key
            .filter(|k| !k.is_empty())
            .map(|k| mask_api_key(&k)),
    })
}

/// NGX Pulse account profile for Settings (no secrets).
#[tauri::command]
pub async fn pulse_profile(state: State<'_, Arc<AppState>>) -> Result<crate::ngx::PulseProfile, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
        let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
        let client = NgxPulseClient::from_settings(&settings, password, api_key);
        Ok(block_on_local(client.get_user_profile()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Verify Pulse session + LLM, then mark onboarding complete. Rejects mock / failed auth.
#[tauri::command]
pub async fn complete_onboarding(state: State<'_, Arc<AppState>>) -> Result<crate::ngx::PulseAuthReport, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(SECRET_PULSE_PASSWORD).map_err(|e| e.to_string())?;
        let api_key = get_secret(SECRET_PULSE_API_KEY).map_err(|e| e.to_string())?;
        let client = NgxPulseClient::from_settings(&settings, password, api_key);
        let report = block_on_local(client.diagnose_login());
        if !report.ok || report.auth_mode != "session" {
            return Err(format!(
                "Pulse session required before completing setup: {}",
                report.message
            ));
        }

        state
            .agent
            .test_llm_sync(&settings)
            .map_err(|e| format!("LLM verification failed: {e}"))?;

        state
            .db
            .with_conn(mark_onboarding_complete)
            .map_err(|e| e.to_string())?;

        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) fn load_portfolio_default_sync(state: &Arc<AppState>) -> Result<serde_json::Value, String> {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let id = state
            .db
            .with_conn(PortfolioService::get_default_portfolio_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "No default portfolio".to_string())?;
        let summary = state
            .db
            .with_conn(|conn| PortfolioService::get_portfolio(conn, &state.cache, &id))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Portfolio not found".to_string())?;

        let mut value = serde_json::to_value(&summary).map_err(|e| e.to_string())?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("tradingMode".into(), serde_json::json!("sandbox"));
            obj.insert(
                "brokerId".into(),
                serde_json::json!(crate::broker::BrokerId::parse(&settings.selected_broker).as_str()),
            );
            obj.insert(
                "brokerName".into(),
                serde_json::json!(crate::broker::BrokerId::parse(&settings.selected_broker).short_name()),
            );
        }

        let Some(session) = crate::broker::open_live_broker(&settings) else {
            return Ok(value);
        };

        let status = block_on_local(session.profile_status(&settings));
        if !status.ok || !status.connected {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("tradingMode".into(), serde_json::json!("sandbox"));
                obj.insert(
                    "wealthStatus".into(),
                    serde_json::to_value(&status).unwrap_or(serde_json::Value::Null),
                );
            }
            return Ok(value);
        }

        match block_on_local(session.refresh_book(&state.db)) {
            Ok(book) => Ok(live_portfolio_payload(
                &id,
                &summary,
                &book,
                &status,
                false,
                wealth_pnl_today(state, session.id().as_str(), book.profit),
                session.id(),
            )),
            Err(e) => {
                tracing::warn!(target: "broker", error = %e, "live portfolio sync failed");
                if let Some(book) = session.load_book(&state.db).ok().flatten() {
                    Ok(live_portfolio_payload(
                        &id,
                        &summary,
                        &book,
                        &status,
                        true,
                        wealth_pnl_today(state, session.id().as_str(), book.profit),
                        session.id(),
                    ))
                } else if let Some(obj) = value.as_object_mut() {
                    obj.insert("tradingMode".into(), serde_json::json!("sandbox"));
                    obj.insert(
                        "wealthStatus".into(),
                        serde_json::to_value(&status).unwrap_or(serde_json::Value::Null),
                    );
                    obj.insert("wealthError".into(), serde_json::json!(e.to_string()));
                    Ok(value)
                } else {
                    Ok(value)
                }
            }
        }
}

#[tauri::command]
pub async fn portfolio_default(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || load_portfolio_default_sync(&state))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn portfolio_quotes(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let id = state
            .db
            .with_conn(PortfolioService::get_default_portfolio_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "No default portfolio".to_string())?;

        let sandbox = state
            .db
            .with_conn(|conn| PortfolioService::get_portfolio(conn, &state.cache, &id))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Portfolio not found".to_string())?;

        let mut trading_mode = "sandbox";
        let mut cash = sandbox
            .portfolio
            .get("cash_balance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        // id, symbol, qty, avg_cost, broker_last, broker_value
        let mut lots: Vec<(String, String, f64, f64, f64, f64)> = sandbox
            .positions
            .iter()
            .filter_map(|p| {
                Some((
                    p.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    p.get("symbol").and_then(|v| v.as_str())?.to_string(),
                    p.get("quantity").and_then(|v| v.as_f64())?,
                    p.get("avg_cost").and_then(|v| v.as_f64())?,
                    p.get("current_price").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    p.get("market_value").and_then(|v| v.as_f64()).unwrap_or(0.0),
                ))
            })
            .filter(|(_, _, q, _, _, _)| *q > 0.0)
            .collect();

        let mut live_pnl_fallback: Option<f64> = None;
        let mut wealth_synced_at: Option<String> = None;
        let mut live_broker = crate::broker::BrokerId::parse(&settings.selected_broker);

        if let Some(session) = crate::broker::open_live_broker(&settings) {
            live_broker = session.id();
            let status = block_on_local(session.profile_status(&settings));
            if status.ok && status.connected {
                let book = match block_on_local(session.refresh_book(&state.db)) {
                    Ok(b) => Some(b),
                    Err(_) => session.load_book(&state.db).ok().flatten(),
                };
                if let Some(book) = book {
                    trading_mode = "live";
                    cash = book.brokerage_balance;
                    live_pnl_fallback = Some(wealth_pnl_today(&state, live_broker.as_str(), book.profit));
                    wealth_synced_at = Some(book.synced_at.clone());
                    lots = book
                        .holdings
                        .iter()
                        .filter(|h| h.quantity > 0.0)
                        .map(|h| {
                            (
                                format!("wealth-{}", h.stock_id),
                                h.symbol.clone(),
                                h.quantity,
                                h.buy_price.unwrap_or(h.price),
                                h.price,
                                h.current_value,
                            )
                        })
                        .collect();
                }
            }
        }

        if trading_mode == "live" {
            let mut market_value = 0.0;
            let mut unrealized_pnl = 0.0;
            let mut positions = Vec::new();
            for (pid, symbol, qty, avg_cost, broker_last, broker_value) in &lots {
                let last = if *broker_last > 0.0 { *broker_last } else { 0.0 };
                let value = if *broker_value > 0.0 {
                    *broker_value
                } else if last > 0.0 {
                    qty * last
                } else {
                    0.0
                };
                market_value += value;
                if last > 0.0 && *avg_cost > 0.0 {
                    unrealized_pnl += qty * (last - avg_cost);
                }
                positions.push(serde_json::json!({
                    "id": pid,
                    "symbol": symbol,
                    "quantity": qty,
                    "avg_cost": avg_cost,
                    "current_price": last,
                    "market_value": value,
                }));
            }
            let pnl_today = live_pnl_fallback.unwrap_or(0.0);
            let collapsed = !lots.is_empty() && market_value <= 0.0 && cash <= 0.0;
            return Ok(serde_json::json!({
                "positions": positions,
                "total_equity": cash + market_value,
                "market_value": market_value,
                "pnl_today": pnl_today,
                "unrealized_pnl": unrealized_pnl,
                "quotesAsOf": wealth_synced_at.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
                "quotedSymbols": Vec::<String>::new(),
                "stale": collapsed,
                "tradingMode": trading_mode,
                "brokerId": live_broker.as_str(),
                "brokerName": live_broker.short_name(),
                "portfolio": {
                    "id": id,
                    "cash_balance": cash,
                },
            }));
        }

        let mut symbols: Vec<String> = Vec::new();
        for (_, sym, _, _, _, _) in &lots {
            if !symbols.contains(sym) {
                symbols.push(sym.clone());
            }
        }

        let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
        let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
        let pulse = NgxPulseClient::from_settings(&settings, password, api_key);
        let quotes = block_on_local(async { pulse.get_latest_quotes(&symbols).await })
            .unwrap_or_else(|_| vec![]);
        let quotes_empty = quotes.is_empty();
        let quoted_symbols: Vec<String> = quotes.iter().map(|q| q.symbol.clone()).collect();

        let _ = state.db.with_conn(|conn| {
            for q in &quotes {
                let _ = crate::ingest::IngestionService::upsert_last_quote(conn, &state.cache, q);
            }
            Ok::<_, anyhow::Error>(())
        });

        let quote_map: std::collections::HashMap<String, crate::ngx::LatestQuote> =
            quotes.into_iter().map(|q| (q.symbol.clone(), q)).collect();

        let mut market_value = 0.0;
        let mut pnl_today = 0.0;
        let mut unrealized_pnl = 0.0;
        let mut positions = Vec::new();
        let mut missing_pulse = 0usize;

        for (pid, symbol, qty, avg_cost, broker_last, broker_value) in &lots {
            let (pulse_last, prev_close) = if let Some(q) = quote_map.get(symbol) {
                (q.last, q.prev_close)
            } else {
                let last = state
                    .db
                    .with_conn(|conn| Ok(PortfolioService::get_price(conn, &state.cache, symbol)))
                    .unwrap_or(0.0);
                let prev = state
                    .db
                    .with_conn(|conn| {
                        Ok(conn
                            .query_row(
                                "SELECT price FROM price_history WHERE symbol = ?1
                                 ORDER BY trade_date DESC LIMIT 1 OFFSET 1",
                                [symbol],
                                |row| row.get::<_, f64>(0),
                            )
                            .ok())
                    })
                    .ok()
                    .flatten();
                (last, prev)
            };
            let last = if pulse_last > 0.0 {
                pulse_last
            } else if *broker_last > 0.0 {
                missing_pulse += 1;
                *broker_last
            } else {
                missing_pulse += 1;
                0.0
            };
            let value = if last > 0.0 {
                qty * last
            } else if *broker_value > 0.0 {
                *broker_value
            } else {
                0.0
            };
            market_value += value;
            if last > 0.0 {
                if let Some(px) = prev_close {
                    pnl_today += qty * (last - px);
                }
                if *avg_cost > 0.0 {
                    unrealized_pnl += qty * (last - avg_cost);
                }
            }
            positions.push(serde_json::json!({
                "id": pid,
                "symbol": symbol,
                "quantity": qty,
                "avg_cost": avg_cost,
                "current_price": if last > 0.0 { last } else { *broker_last },
                "market_value": value,
            }));
        }

        let stale = quotes_empty && !symbols.is_empty() && missing_pulse == lots.len();

        Ok(serde_json::json!({
            "positions": positions,
            "total_equity": cash + market_value,
            "market_value": market_value,
            "pnl_today": pnl_today,
            "unrealized_pnl": unrealized_pnl,
            "quotesAsOf": chrono::Utc::now().to_rfc3339(),
            "quotedSymbols": quoted_symbols,
            "stale": stale,
            "tradingMode": trading_mode,
            "portfolio": {
                "id": id,
                "cash_balance": cash,
            },
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn wealth_pnl_today(state: &Arc<AppState>, venue: &str, fallback: f64) -> f64 {
    state
        .db
        .with_conn(|conn| crate::portfolio::EquityCurveService::pnl_today(conn, venue))
        .ok()
        .flatten()
        .unwrap_or(fallback)
}

fn live_portfolio_payload(
    sandbox_id: &str,
    summary: &crate::portfolio::PortfolioSummary,
    book: &crate::wealth::CachedWealthBook,
    status: &crate::wealth::WealthProfileStatus,
    stale: bool,
    pnl_today: f64,
    broker_id: crate::broker::BrokerId,
) -> serde_json::Value {
    let cash = book.brokerage_balance;
    let market_value = book.market_value();
    let mut unrealized_pnl = 0.0;
    let positions: Vec<serde_json::Value> = book
        .holdings
        .iter()
        .filter(|h| h.quantity > 0.0)
        .map(|h| {
            let avg = h.buy_price.unwrap_or(h.price);
            if h.price > 0.0 && avg > 0.0 {
                unrealized_pnl += h.quantity * (h.price - avg);
            }
            serde_json::json!({
                "id": format!("wealth-{}", h.stock_id),
                "symbol": h.symbol,
                "quantity": h.quantity,
                "avg_cost": avg,
                "current_price": h.price,
                "market_value": h.current_value,
            })
        })
        .collect();
    serde_json::json!({
        "portfolio": {
            "id": sandbox_id,
            "name": "wealth-live",
            "starting_capital": cash,
            "cash_balance": cash,
            "created_at": summary.portfolio.get("created_at").cloned().unwrap_or(serde_json::Value::Null),
            "strategy_param_set_id": summary.portfolio.get("strategy_param_set_id").cloned().unwrap_or(serde_json::Value::Null),
        },
        "positions": positions,
        "total_equity": cash + market_value,
        "market_value": market_value,
        "pnl_today": pnl_today,
        "unrealized_pnl": unrealized_pnl,
        "quotesAsOf": book.synced_at,
        "tradingMode": "live",
        "brokerId": broker_id.as_str(),
        "brokerName": broker_id.short_name(),
        "tradingVerified": status.trading_verified,
        "wealthStatus": status,
        "stale": stale,
        "syncedAt": book.synced_at,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WealthLoginPayload {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wealth2faPayload {
    pub email: String,
    pub temp_token: String,
    pub code: String,
}

#[tauri::command]
pub async fn wealth_login(
    payload: WealthLoginPayload,
    state: State<'_, Arc<AppState>>,
) -> Result<crate::wealth::WealthLoginResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        if crate::broker::BrokerId::parse(&settings.selected_broker) != crate::broker::BrokerId::Wealth {
            return Err("Select Coronation Wealth as the live broker before connecting.".into());
        }
        let client = crate::wealth::WealthClient::from_settings(&settings, Some(payload.password.clone()));
        let result = block_on_local(client.login(&payload.email, &payload.password));
        let result = result.map_err(|e| e.to_string())?;
        if result.ok && !result.needs_2fa {
            let _ = set_secret(crate::secrets::SECRET_WEALTH_PASSWORD, &payload.password);
            state
                .db
                .with_conn(|conn| crate::settings::mark_wealth_connected(conn, &payload.email))
                .map_err(|e| e.to_string())?;
            let _ = block_on_local(crate::wealth::WealthSyncService::refresh(&state.db, &client));
        } else if result.needs_2fa {
            let _ = set_secret(crate::secrets::SECRET_WEALTH_PASSWORD, &payload.password);
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn wealth_verify_2fa(
    payload: Wealth2faPayload,
    state: State<'_, Arc<AppState>>,
) -> Result<crate::wealth::WealthLoginResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        if crate::broker::BrokerId::parse(&settings.selected_broker) != crate::broker::BrokerId::Wealth {
            return Err("Select Coronation Wealth as the live broker before connecting.".into());
        }
        let password = get_secret(crate::secrets::SECRET_WEALTH_PASSWORD).ok().flatten();
        let client = crate::wealth::WealthClient::from_settings(&settings, password);
        let result = block_on_local(client.verify_2fa(&payload.temp_token, &payload.code, &payload.email))
            .map_err(|e| e.to_string())?;
        if result.ok && !result.needs_2fa {
            state
                .db
                .with_conn(|conn| crate::settings::mark_wealth_connected(conn, &payload.email))
                .map_err(|e| e.to_string())?;
            let _ = block_on_local(crate::wealth::WealthSyncService::refresh(&state.db, &client));
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn wealth_profile(state: State<'_, Arc<AppState>>) -> Result<crate::wealth::WealthProfileStatus, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(crate::secrets::SECRET_WEALTH_PASSWORD).ok().flatten();
        let client = crate::wealth::WealthClient::from_settings(&settings, password);
        Ok(block_on_local(client.profile_status(&settings)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn wealth_logout(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(crate::secrets::SECRET_WEALTH_PASSWORD).ok().flatten();
        let client = crate::wealth::WealthClient::from_settings(&settings, password);
        block_on_local(client.logout_remote());
        crate::wealth::WealthClient::clear_local_secrets();
        state
            .db
            .with_conn(crate::settings::clear_wealth_settings)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BambooLoginPayload {
    pub phone_number: String,
    pub password: String,
    pub transaction_pin: Option<String>,
}

#[tauri::command]
pub async fn bamboo_login(
    payload: BambooLoginPayload,
    state: State<'_, Arc<AppState>>,
) -> Result<crate::bamboo::BambooLoginResult, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        if crate::broker::BrokerId::parse(&settings.selected_broker) != crate::broker::BrokerId::Bamboo {
            return Err("Select Bamboo as the live broker before connecting.".into());
        }
        let client = crate::bamboo::BambooClient::from_settings(&settings);
        let pin = payload.transaction_pin.as_deref();
        let result = block_on_local(client.login(&payload.phone_number, &payload.password, pin))
            .map_err(|e| e.to_string())?;
        if result.ok {
            let _ = set_secret(crate::secrets::SECRET_BAMBOO_PASSWORD, &payload.password);
            state
                .db
                .with_conn(|conn| crate::settings::mark_bamboo_connected(conn, &payload.phone_number))
                .map_err(|e| e.to_string())?;
            let _ = block_on_local(crate::bamboo::BambooSyncService::refresh(&state.db, &client));
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn bamboo_profile(state: State<'_, Arc<AppState>>) -> Result<crate::wealth::WealthProfileStatus, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let client = crate::bamboo::BambooClient::from_settings(&settings);
        Ok(block_on_local(client.profile_status(&settings)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn bamboo_logout(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        crate::bamboo::BambooClient::clear_local_secrets();
        state
            .db
            .with_conn(crate::settings::clear_bamboo_settings)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn broker_list(state: State<'_, Arc<AppState>>) -> Result<Vec<crate::broker::BrokerListItem>, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    Ok(crate::broker::catalog(&settings))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectBrokerPayload {
    pub broker_id: String,
}

#[tauri::command]
pub fn set_selected_broker(
    payload: SelectBrokerPayload,
    state: State<'_, Arc<AppState>>,
) -> Result<AppSettings, String> {
    let id = crate::broker::BrokerId::parse(&payload.broker_id);
    state
        .db
        .with_conn(|conn| crate::settings::set_selected_broker(conn, id.as_str()))
        .map_err(|e| e.to_string())?;
    state.db.with_conn(get_settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn portfolio_performance(
    id: Option<String>,
    venue: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<serde_json::Value>, String> {
    let venue = venue.unwrap_or_else(|| "sandbox".into());
    if venue == "wealth" || venue == "sandbox" || venue == "bamboo" {
        return state
            .db
            .with_conn(|conn| crate::portfolio::EquityCurveService::get_curve(conn, &venue))
            .map_err(|e| e.to_string());
    }
    let id = id.ok_or_else(|| "id or venue required".to_string())?;
    state
        .db
        .with_conn(|conn| PortfolioService::get_performance(conn, &id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn usage_ngx_pulse(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
    let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
    let client = NgxPulseClient::from_settings(&settings, password, api_key);
    let auth_mode = match client.auth_mode() {
        crate::ngx::AuthMode::Session => "session",
        crate::ngx::AuthMode::ApiKey => "api_key",
        crate::ngx::AuthMode::Mock => "mock",
    };
    let (daily, limit, remaining) = state
        .db
        .with_conn(RateLimiter::usage_stats)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "daily": daily,
        "limit": if auth_mode == "session" { serde_json::Value::Null } else { limit.into() },
        "remaining": if auth_mode == "session" { serde_json::Value::Null } else { remaining.into() },
        "authMode": auth_mode,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketStatusResponse {
    pub is_open: bool,
    pub is_post_close: bool,
    pub is_trading_day: bool,
    pub phase: String,
    pub today_wat: String,
    pub now_wat: String,
    pub pulse_status: Option<String>,
    pub pulse_is_open: Option<bool>,
    pub app_env: String,
    pub market_hours_enforced: bool,
}

#[tauri::command]
pub async fn market_status(state: State<'_, Arc<AppState>>) -> Result<MarketStatusResponse, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let calendar = TradingCalendar::default();
        let is_open = calendar.is_market_open();
        let is_post_close = calendar.is_post_close_window();
        let is_trading_day = calendar.is_trading_day(None);
        let phase = if is_open {
            "open"
        } else if is_post_close {
            "post_close"
        } else {
            "closed"
        };

        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
        let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
        let client = NgxPulseClient::from_settings(&settings, password, api_key);

        let (pulse_status, pulse_is_open) = block_on_local(async {
            match client.get_market_status().await {
                Ok(s) => (Some(s.status), Some(s.is_open)),
                Err(_) => (None, None),
            }
        });

        Ok(MarketStatusResponse {
            is_open,
            is_post_close,
            is_trading_day,
            phase: phase.into(),
            today_wat: calendar.today_wat(),
            now_wat: calendar.now_wat().format("%H:%M WAT").to_string(),
            pulse_status,
            pulse_is_open,
            app_env: crate::runtime_util::app_env().into(),
            market_hours_enforced: crate::runtime_util::enforce_market_hours(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn cycle_status(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "running": state.is_cycle_running(),
    }))
}

#[tauri::command]
pub async fn cycle_run(
    app: AppHandle,
    allow_bulk_liquidation: Option<bool>,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    let allow_bulk = allow_bulk_liquidation.unwrap_or(false);
    tokio::task::spawn_blocking(move || {
        let Some(_gate) = CycleGateGuard::acquire(state.clone()) else {
            return Err("A trading cycle is already running.".into());
        };

        let _ = app.emit(
            "cycle:start",
            serde_json::json!({ "source": "manual" }),
        );

        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        if !settings.onboarding_complete {
            return Err("Complete onboarding before running a cycle.".into());
        }
        let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
        let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
        let client = NgxPulseClient::from_settings(&settings, password, api_key);
        let broker = crate::broker::open_live_broker(&settings);
        let calendar = TradingCalendar::default();
        if !crate::runtime_util::market_activity_allowed(&calendar) {
            let payload = serde_json::json!({
                "ok": false,
                "source": "manual",
                "signals": 0,
                "executed": 0,
                "warnings": [],
                "error": "NGX market is closed. Cycles are blocked in production outside market hours (set APP_ENV=dev to bypass).",
            });
            let _ = app.emit("cycle:complete", payload.clone());
            return Err(
                "NGX market is closed. Cycles are blocked in production outside market hours (set APP_ENV=dev to bypass)."
                    .into(),
            );
        }

        let trading_mode = if let Some(ref session) = broker {
            block_on_local(session.resolve_trading_mode(&settings))
        } else {
            crate::wealth::TradingMode::Sandbox
        };
        if trading_mode == crate::wealth::TradingMode::Live && broker.is_none() {
            return Err("Connect the selected live broker before live trading.".into());
        }

        let cycle_id = uuid::Uuid::new_v4().to_string();
        match block_on_local(run_cycle(
            &state.db,
            &state.agent,
            &settings,
            &state.cache,
            &client,
            &calendar,
            broker.as_ref(),
            allow_bulk,
            Some(&cycle_id),
        )) {
            Ok(result) => {
                let mut payload = result;
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("ok".into(), serde_json::json!(true));
                    obj.insert("source".into(), serde_json::json!("manual"));
                    if trading_mode == crate::wealth::TradingMode::Live && !settings.live_trading_enabled {
                        obj.insert("liveDisabled".into(), serde_json::json!(true));
                    }
                }
                let _ = app.emit("cycle:complete", payload.clone());
                Ok(payload)
            }
            Err(e) => {
                let msg = e.to_string();
                let payload = serde_json::json!({
                    "ok": false,
                    "source": "manual",
                    "signals": 0,
                    "executed": 0,
                    "warnings": [],
                    "error": msg,
                });
                let _ = app.emit("cycle:complete", payload);
                Err(e.to_string())
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn cycle_ingest(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
        let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
        let client = NgxPulseClient::from_settings(&settings, password, api_key);
        let calendar = TradingCalendar::default();
        let count = block_on_local(IngestionService::ingest_stocks(
            &state.db,
            &client,
            &state.cache,
            &calendar,
            true,
        ))
        .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "count": count }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn generate_signals(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let broker = crate::broker::open_live_broker(&settings);

        let ids = block_on_local(async {
                    let trading_mode = if let Some(ref session) = broker {
                        session.resolve_trading_mode(&settings).await
                    } else {
                        crate::wealth::TradingMode::Sandbox
                    };
                    let (cash, venue, holdings) = if trading_mode == crate::wealth::TradingMode::Live {
                        if let Some(ref session) = broker {
                            let book = match session.refresh_book(&state.db).await {
                                Ok(b) => Some(b),
                                Err(_) => session.load_book(&state.db).ok().flatten(),
                            };
                            let cash = book.as_ref().map(|b| b.brokerage_balance);
                            let holdings = book
                                .as_ref()
                                .map(|b| crate::signals::HeldLot::from_holdings(&b.holdings))
                                .unwrap_or_default();
                            (cash, session.id().as_str(), holdings)
                        } else {
                            (None, "sandbox", Vec::new())
                        }
                    } else {
                        (None, "sandbox", Vec::new())
                    };
                    SignalGenerationService::generate_for_portfolio(
                        &state.db,
                        &state.agent,
                        &settings,
                        None,
                        cash,
                        venue,
                        if venue != "sandbox" {
                            Some(holdings.as_slice())
                        } else {
                            None
                        },
                    )
                    .await
                })
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "signalIds": ids.signal_ids,
            "count": ids.signal_ids.len(),
            "universeSize": ids.universe_size,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_signals(limit: Option<i64>, state: State<'_, Arc<AppState>>) -> Result<Vec<serde_json::Value>, String> {
    let limit = limit.unwrap_or(50);
    state
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, symbol, generated_at, action, confidence, rationale, model_name, executed, risk_policy_result
                 FROM signals ORDER BY generated_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "symbol": row.get::<_, String>(1)?,
                    "generated_at": row.get::<_, String>(2)?,
                    "action": row.get::<_, String>(3)?,
                    "confidence": row.get::<_, f64>(4)?,
                    "rationale": row.get::<_, String>(5)?,
                    "model_name": row.get::<_, String>(6)?,
                    "executed": row.get::<_, i64>(7)? == 1,
                    "risk_policy_result": row.get::<_, String>(8)?,
                }))
            })?;
            Ok(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_trades(limit: Option<i64>, state: State<'_, Arc<AppState>>) -> Result<Vec<serde_json::Value>, String> {
    let limit = limit.unwrap_or(50);
    state
        .db
        .with_conn(|conn| crate::strategy_coach::load_recent_trades(conn, limit))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn memory_list(
    limit: Option<i64>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<crate::memory::MemoryRecord>, String> {
    let limit = limit.unwrap_or(200);
    state
        .db
        .with_conn(|conn| crate::memory::list_memories(conn, limit))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn memory_search(
    query: String,
    symbol: Option<String>,
    k: Option<u32>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<crate::memory::MemoryRecord>, String> {
    let settings = state
        .db
        .with_conn(get_settings)
        .map_err(|e| e.to_string())?;
    let api_key = get_secret(SECRET_LLM_API_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    let q = query.trim();
    if q.is_empty() {
        return state
            .db
            .with_conn(|conn| crate::memory::list_memories(conn, k.unwrap_or(40) as i64))
            .map_err(|e| e.to_string());
    }
    crate::memory::search_memories(
        &state.db,
        &settings,
        &api_key,
        q,
        symbol.as_deref(),
        k.map(|n| n as usize),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn memory_delete(id: String, state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    if id.trim().is_empty() {
        return Err("Memory id is required".into());
    }
    state
        .db
        .with_conn(|conn| crate::memory::delete_memory(conn, id.trim()))
        .map_err(|e| e.to_string())
}

/// Fast path: local DB + in-memory price cache only (no NGX Pulse network calls).
#[tauri::command]
pub fn symbol_detail(
    symbol: String,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let symbol = symbol.trim().to_uppercase();
    if symbol.is_empty() {
        return Err("Symbol is required".into());
    }

    state
        .db
        .with_conn(|conn| {
            use rusqlite::OptionalExtension;

            let instrument: Option<(String, Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT symbol, name, sector FROM instruments WHERE symbol = ?1",
                    [&symbol],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;

            let mut price_stmt = conn.prepare(
                "SELECT trade_date, price, change_percent, volume, market_cap, pe_ratio
                 FROM price_history WHERE symbol = ?1
                 ORDER BY trade_date DESC LIMIT 90",
            )?;
            let mut prices: Vec<serde_json::Value> = price_stmt
                .query_map([&symbol], |row| {
                    Ok(serde_json::json!({
                        "date": row.get::<_, String>(0)?,
                        "price": row.get::<_, f64>(1)?,
                        "changePercent": row.get::<_, Option<f64>>(2)?,
                        "volume": row.get::<_, Option<i64>>(3)?,
                        "marketCap": row.get::<_, Option<f64>>(4)?,
                        "peRatio": row.get::<_, Option<f64>>(5)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();
            prices.reverse();

            let portfolio_id = PortfolioService::get_default_portfolio_id(conn)?;
            let position = if let Some(pid) = portfolio_id {
                conn.query_row(
                    "SELECT quantity, avg_cost FROM sandbox_positions
                     WHERE portfolio_id = ?1 AND symbol = ?2",
                    rusqlite::params![pid, symbol],
                    |row| {
                        Ok(serde_json::json!({
                            "quantity": row.get::<_, f64>(0)?,
                            "avgCost": row.get::<_, f64>(1)?,
                        }))
                    },
                )
                .optional()?
            } else {
                None
            };

            let mut sig_stmt = conn.prepare(
                "SELECT id, generated_at, action, confidence, rationale, model_name, executed, risk_policy_result
                 FROM signals WHERE symbol = ?1 ORDER BY generated_at DESC LIMIT 25",
            )?;
            let signals: Vec<serde_json::Value> = sig_stmt
                .query_map([&symbol], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "generatedAt": row.get::<_, String>(1)?,
                        "action": row.get::<_, String>(2)?,
                        "confidence": row.get::<_, f64>(3)?,
                        "rationale": row.get::<_, String>(4)?,
                        "modelName": row.get::<_, String>(5)?,
                        "executed": row.get::<_, i64>(6)? == 1,
                        "riskPolicyResult": row.get::<_, String>(7)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();

            let mut trade_stmt = conn.prepare(
                "SELECT id, side, quantity, fill_price, simulated_fee, executed_at, resulting_cash_balance
                 FROM sandbox_trades WHERE symbol = ?1 ORDER BY executed_at DESC LIMIT 25",
            )?;
            let trades: Vec<serde_json::Value> = trade_stmt
                .query_map([&symbol], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "side": row.get::<_, String>(1)?,
                        "quantity": row.get::<_, f64>(2)?,
                        "fillPrice": row.get::<_, f64>(3)?,
                        "simulatedFee": row.get::<_, f64>(4)?,
                        "executedAt": row.get::<_, String>(5)?,
                        "resultingCashBalance": row.get::<_, f64>(6)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();

            let latest = prices.last().cloned();
            let cached = state.cache.get_price(&symbol);
            let quote_price = cached
                .as_ref()
                .map(|c| c.price)
                .or_else(|| latest.as_ref().and_then(|p| p.get("price")).and_then(|v| v.as_f64()));
            let pulse_quote = latest.as_ref().map(|last| {
                serde_json::json!({
                    "symbol": symbol,
                    "name": instrument.as_ref().and_then(|(_, n, _)| n.clone()),
                    "price": quote_price,
                    "changePercent": last.get("changePercent").cloned().unwrap_or(serde_json::Value::Null),
                    "volume": last.get("volume").cloned().unwrap_or(serde_json::Value::Null),
                    "marketCap": last.get("marketCap").cloned().unwrap_or(serde_json::Value::Null),
                    "peRatio": last.get("peRatio").cloned().unwrap_or(serde_json::Value::Null),
                    "sector": instrument.as_ref().and_then(|(_, _, s)| s.clone()),
                    "source": if cached.is_some() { "cache" } else { "local" },
                })
            });

            Ok(serde_json::json!({
                "symbol": symbol,
                "name": instrument.as_ref().and_then(|(_, n, _)| n.clone()),
                "sector": instrument.as_ref().and_then(|(_, _, s)| s.clone()),
                "found": instrument.is_some() || !prices.is_empty(),
                "latest": latest,
                "prices": prices,
                "position": position,
                "signals": signals,
                "trades": trades,
                "pulseQuote": pulse_quote,
                "needsPulsePrices": prices.len() < 5,
            }))
        })
        .map_err(|e| e.to_string())
}

/// Optional enrichment: bounded single-symbol price history from Pulse (never loads full stocks).
#[tauri::command]
pub async fn symbol_detail_pulse(
    symbol: String,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let symbol = symbol.trim().to_uppercase();
    if symbol.is_empty() {
        return Err("Symbol is required".into());
    }
    let state = state.inner().clone();
    let symbol_for_pulse = symbol.clone();

    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
        let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
        let client = NgxPulseClient::from_settings(&settings, password, api_key);

        let to = chrono::Utc::now().date_naive();
        let from = to - chrono::Duration::days(120);
        let from_s = from.format("%Y-%m-%d").to_string();
        let to_s = to.format("%Y-%m-%d").to_string();

        let result = block_on_local(async {
                match client
                    .get_symbol_price(&symbol_for_pulse, Some(&from_s), Some(&to_s))
                    .await
                {
                    Ok(pts) => {
                        let mut prices: Vec<serde_json::Value> = pts
                            .into_iter()
                            .map(|p| {
                                serde_json::json!({
                                    "date": p.date,
                                    "price": p.price,
                                    "volume": p.volume,
                                })
                            })
                            .collect();
                        if prices.len() > 90 {
                            let skip = prices.len() - 90;
                            prices = prices.into_iter().skip(skip).collect();
                        }
                        let pulse_quote = prices.last().map(|last| {
                            let prev = if prices.len() >= 2 {
                                prices.get(prices.len() - 2)
                            } else {
                                None
                            };
                            let price = last.get("price").and_then(|v| v.as_f64());
                            let prev_price = prev.and_then(|p| p.get("price")).and_then(|v| v.as_f64());
                            let change_percent = match (price, prev_price) {
                                (Some(c), Some(p)) if p.abs() > f64::EPSILON => {
                                    Some(((c - p) / p) * 100.0)
                                }
                                _ => None,
                            };
                            serde_json::json!({
                                "symbol": symbol_for_pulse,
                                "price": price,
                                "changePercent": change_percent,
                                "volume": last.get("volume").cloned().unwrap_or(serde_json::Value::Null),
                                "source": "ngx_pulse",
                            })
                        });
                        serde_json::json!({
                            "symbol": symbol_for_pulse,
                            "prices": prices,
                            "pulseQuote": pulse_quote,
                            "error": null,
                        })
                    }
                    Err(e) => serde_json::json!({
                        "symbol": symbol_for_pulse,
                        "prices": [],
                        "pulseQuote": null,
                        "error": e.to_string(),
                    }),
                }
            });

        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_strategy(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    state
        .db
        .with_conn(crate::strategy_coach::read_active_strategy_json)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_strategy(
    strategy: crate::strategy_coach::StrategyParams,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    crate::strategy_coach::apply_strategy_params(&state.db, &strategy)?;
    get_strategy(state)
}

#[tauri::command]
pub fn export_database(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    Ok(state.db.path.display().to_string())
}

#[tauri::command]
pub fn app_data_dir(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    Ok(state
        .db
        .path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| state.db.path.display().to_string()))
}

#[tauri::command]
pub fn reset_local_data(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let capital = state
        .db
        .with_conn(get_settings)
        .map(|s| s.default_starting_capital)
        .unwrap_or(10_000_000.0);

    let _ = delete_secret(SECRET_PULSE_PASSWORD);
    let _ = delete_secret(SECRET_PULSE_API_KEY);
    let _ = delete_secret(SECRET_LLM_API_KEY);
    crate::wealth::WealthClient::clear_local_secrets();

    state
        .db
        .wipe_and_reseed(capital)
        .map_err(|e| e.to_string())?;
    state.cache.clear();

    Ok(serde_json::json!({
        "ok": true,
        "dataDir": state.db.path.parent().map(|p| p.display().to_string()),
    }))
}
