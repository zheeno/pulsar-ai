use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::app_state::{AppState, CycleGateGuard};
use crate::backtest::BacktestService;
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
    if let Some(key) = payload.llm_api_key {
        set_secret(SECRET_LLM_API_KEY, &key).map_err(|e| e.to_string())?;
    }

    // onboarding_complete may only be set by complete_onboarding (or cleared by logout).
    let mut settings = payload.settings;
    let current = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    settings.onboarding_complete = current.onboarding_complete;

    state
        .db
        .with_conn(|conn| save_settings(conn, &settings))
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

#[tauri::command]
pub async fn portfolio_default(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
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
        }

        if !settings.wealth_connected {
            return Ok(value);
        }

        let wealth_password = get_secret(crate::secrets::SECRET_WEALTH_PASSWORD).ok().flatten();
        let client = crate::wealth::WealthClient::from_settings(&settings, wealth_password);
        let status = block_on_local(client.profile_status(&settings));
        // Show Wealth cash/positions whenever the session is connected and profile loads.
        // Live *order* routing still requires trading_verified (cycle/execution path).
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

        match block_on_local(async {
            let snap = client.get_portfolio().await?;
            let cash = match client.get_wallet().await {
                Ok(w) => w.brokerage_balance,
                Err(_) => status
                    .brokerage_balance
                    .unwrap_or(snap.balance),
            };
            Ok::<_, anyhow::Error>((cash, snap))
        }) {
            Ok((cash, snap)) => {
                let positions: Vec<serde_json::Value> = snap
                    .holdings
                    .iter()
                    .map(|h| {
                        serde_json::json!({
                            "id": format!("wealth-{}", h.stock_id),
                            "symbol": h.symbol,
                            "quantity": h.quantity,
                            "avg_cost": h.buy_price.unwrap_or(h.price),
                            "current_price": h.price,
                            "market_value": h.current_value,
                        })
                    })
                    .collect();
                let market_value = if snap.stock_value > 0.0 {
                    snap.stock_value
                } else {
                    snap.holdings.iter().map(|h| h.current_value).sum()
                };
                let total_equity = cash + market_value;
                let _ = state.db.with_conn(|conn| {
                    crate::portfolio::EquityCurveService::upsert_point(
                        conn,
                        "wealth",
                        total_equity,
                        cash,
                        market_value,
                    )
                });
                let pnl_today = state
                    .db
                    .with_conn(|conn| {
                        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                        Ok(conn
                            .query_row(
                                "SELECT pnl_daily FROM equity_curve_points WHERE venue = 'wealth' AND snapshot_date = ?1",
                                [&today],
                                |row| row.get::<_, f64>(0),
                            )
                            .unwrap_or(snap.profit))
                    })
                    .unwrap_or(snap.profit);
                Ok(serde_json::json!({
                    "portfolio": {
                        "id": id,
                        "name": "wealth-live",
                        "starting_capital": cash,
                        "cash_balance": cash,
                        "created_at": summary.portfolio.get("created_at").cloned().unwrap_or(serde_json::Value::Null),
                        "strategy_param_set_id": summary.portfolio.get("strategy_param_set_id").cloned().unwrap_or(serde_json::Value::Null),
                    },
                    "positions": positions,
                    "total_equity": total_equity,
                    "market_value": market_value,
                    "pnl_today": pnl_today,
                    "tradingMode": "live",
                    "tradingVerified": status.trading_verified,
                    "wealthStatus": status,
                }))
            }
            Err(e) => {
                tracing::warn!(target: "wealth", error = %e, "wealth portfolio overlay failed");
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("tradingMode".into(), serde_json::json!("sandbox"));
                    obj.insert(
                        "wealthStatus".into(),
                        serde_json::to_value(&status).unwrap_or(serde_json::Value::Null),
                    );
                    obj.insert(
                        "wealthError".into(),
                        serde_json::json!(e.to_string()),
                    );
                }
                Ok(value)
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
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
        let client = crate::wealth::WealthClient::from_settings(&settings, Some(payload.password.clone()));
        let result = block_on_local(client.login(&payload.email, &payload.password));
        let result = result.map_err(|e| e.to_string())?;
        if result.ok && !result.needs_2fa {
            let _ = set_secret(crate::secrets::SECRET_WEALTH_PASSWORD, &payload.password);
            state
                .db
                .with_conn(|conn| crate::settings::mark_wealth_connected(conn, &payload.email))
                .map_err(|e| e.to_string())?;
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
        let password = get_secret(crate::secrets::SECRET_WEALTH_PASSWORD).ok().flatten();
        let client = crate::wealth::WealthClient::from_settings(&settings, password);
        let result = block_on_local(client.verify_2fa(&payload.temp_token, &payload.code, &payload.email))
            .map_err(|e| e.to_string())?;
        if result.ok && !result.needs_2fa {
            state
                .db
                .with_conn(|conn| crate::settings::mark_wealth_connected(conn, &payload.email))
                .map_err(|e| e.to_string())?;
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

#[tauri::command]
pub fn portfolio_performance(
    id: Option<String>,
    venue: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<serde_json::Value>, String> {
    let venue = venue.unwrap_or_else(|| "sandbox".into());
    if venue == "wealth" || venue == "sandbox" {
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

        let (pulse_status, pulse_is_open) = state
            .db
            .with_conn(|conn| {
                Ok(block_on_local(async {
                    match client.get_market_status(conn).await {
                        Ok(s) => (Some(s.status), Some(s.is_open)),
                        Err(_) => (None, None),
                    }
                }))
            })
            .unwrap_or((None, None));

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
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let Some(_gate) = CycleGateGuard::acquire(state.clone()) else {
            return Err("A trading cycle is already running.".into());
        };

        let _ = app.emit(
            "cycle:start",
            serde_json::json!({ "source": "manual" }),
        );

        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
        let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
        let client = NgxPulseClient::from_settings(&settings, password, api_key);
        let wealth_password = get_secret(crate::secrets::SECRET_WEALTH_PASSWORD).ok().flatten();
        let wealth = if settings.wealth_connected {
            Some(crate::wealth::WealthClient::from_settings(
                &settings,
                wealth_password,
            ))
        } else {
            None
        };
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

        match state.db.with_conn(|conn| {
            block_on_local(run_cycle(
                conn,
                &state.agent,
                &settings,
                &state.cache,
                &client,
                &calendar,
                wealth.as_ref(),
            ))
        }) {
            Ok(result) => {
                let mut payload = result;
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("ok".into(), serde_json::json!(true));
                    obj.insert("source".into(), serde_json::json!("manual"));
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
        let count = state
            .db
            .with_conn(|conn| {
                block_on_local(IngestionService::ingest_stocks(
                    conn,
                    &client,
                    &state.cache,
                    &calendar,
                    true,
                ))
            })
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
        let wealth_password = get_secret(crate::secrets::SECRET_WEALTH_PASSWORD).ok().flatten();
        let wealth = if settings.wealth_connected {
            Some(crate::wealth::WealthClient::from_settings(
                &settings,
                wealth_password,
            ))
        } else {
            None
        };

        let ids = state
            .db
            .with_conn(|conn| {
                block_on_local(async {
                    let trading_mode = if let Some(ref w) = wealth {
                        w.resolve_trading_mode(&settings).await
                    } else {
                        crate::wealth::TradingMode::Sandbox
                    };
                    let (cash, venue) = if trading_mode == crate::wealth::TradingMode::Live {
                        let cash = if let Some(ref w) = wealth {
                            match w.get_wallet().await {
                                Ok(wallet) => Some(wallet.brokerage_balance),
                                Err(_) => w.get_portfolio().await.ok().map(|s| s.balance),
                            }
                        } else {
                            None
                        };
                        (cash, "wealth")
                    } else {
                        (None, "sandbox")
                    };
                    SignalGenerationService::generate_for_portfolio(
                        conn,
                        &state.agent,
                        &settings,
                        None,
                        cash,
                        venue,
                    )
                    .await
                })
            })
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "signalIds": ids, "count": ids.len() }))
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
        .with_conn(|conn| {
            let mut out = Vec::new();

            let mut stmt = conn.prepare(
                "SELECT id, symbol, side, quantity, fill_price, simulated_fee, executed_at, resulting_cash_balance
                 FROM sandbox_trades ORDER BY executed_at DESC LIMIT ?1",
            )?;
            for row in stmt.query_map([limit], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "symbol": row.get::<_, String>(1)?,
                    "side": row.get::<_, String>(2)?,
                    "quantity": row.get::<_, f64>(3)?,
                    "fill_price": row.get::<_, f64>(4)?,
                    "simulated_fee": row.get::<_, f64>(5)?,
                    "executed_at": row.get::<_, String>(6)?,
                    "resulting_cash_balance": row.get::<_, Option<f64>>(7)?,
                    "venue": "sandbox",
                    "status": "executed",
                }))
            })? {
                out.push(row?);
            }

            let mut stmt = conn.prepare(
                "SELECT id, symbol, side, quantity, fill_price, fee, created_at, status, rejection_reason
                 FROM broker_orders ORDER BY created_at DESC LIMIT ?1",
            )?;
            for row in stmt.query_map([limit], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "symbol": row.get::<_, String>(1)?,
                    "side": row.get::<_, String>(2)?,
                    "quantity": row.get::<_, f64>(3)?,
                    "fill_price": row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                    "simulated_fee": row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                    "executed_at": row.get::<_, String>(6)?,
                    "resulting_cash_balance": null,
                    "venue": "wealth",
                    "status": row.get::<_, String>(7)?,
                    "rejection_reason": row.get::<_, Option<String>>(8)?,
                }))
            })? {
                out.push(row?);
            }

            out.sort_by(|a, b| {
                let da = a.get("executed_at").and_then(|v| v.as_str()).unwrap_or("");
                let db = b.get("executed_at").and_then(|v| v.as_str()).unwrap_or("");
                db.cmp(da)
            });
            out.truncate(limit as usize);
            Ok(out)
        })
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

        let result = state.db.with_conn(|conn| {
            Ok(block_on_local(async {
                match client
                    .get_symbol_price(conn, &symbol_for_pulse, Some(&from_s), Some(&to_s))
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
            }))
        });

        result.map_err(|e: anyhow::Error| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_strategy(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    state
        .db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT id, name, max_position_pct, max_daily_trades, stop_loss_pct, take_profit_pct,
                        min_confidence_to_trade, max_daily_drawdown_pct, position_size_pct, is_active
                 FROM strategy_param_sets WHERE is_active = 1 LIMIT 1",
                [],
                |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "name": row.get::<_, String>(1)?,
                        "max_position_pct": row.get::<_, f64>(2)?,
                        "max_daily_trades": row.get::<_, i64>(3)?,
                        "stop_loss_pct": row.get::<_, f64>(4)?,
                        "take_profit_pct": row.get::<_, Option<f64>>(5)?,
                        "min_confidence_to_trade": row.get::<_, f64>(6)?,
                        "max_daily_drawdown_pct": row.get::<_, f64>(7)?,
                        "position_size_pct": row.get::<_, f64>(8)?,
                        "is_active": row.get::<_, i64>(9)? == 1,
                    }))
                },
            )?)
        })
        .map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyUpdate {
    pub max_position_pct: f64,
    pub max_daily_trades: i64,
    pub stop_loss_pct: f64,
    pub take_profit_pct: Option<f64>,
    pub min_confidence_to_trade: f64,
    pub max_daily_drawdown_pct: f64,
    pub position_size_pct: f64,
}

fn validate_ratio(name: &str, value: f64) -> Result<(), String> {
    if !(0.0..=1.0).contains(&value) {
        return Err(format!("{name} must be between 0 and 1"));
    }
    Ok(())
}

#[tauri::command]
pub fn update_strategy(
    strategy: StrategyUpdate,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    validate_ratio("maxPositionPct", strategy.max_position_pct)?;
    validate_ratio("stopLossPct", strategy.stop_loss_pct)?;
    validate_ratio("minConfidenceToTrade", strategy.min_confidence_to_trade)?;
    validate_ratio("maxDailyDrawdownPct", strategy.max_daily_drawdown_pct)?;
    validate_ratio("positionSizePct", strategy.position_size_pct)?;
    if let Some(tp) = strategy.take_profit_pct {
        validate_ratio("takeProfitPct", tp)?;
    }
    if strategy.max_daily_trades < 1 {
        return Err("maxDailyTrades must be at least 1".into());
    }

    state
        .db
        .with_conn(|conn| {
            let updated = conn.execute(
                "UPDATE strategy_param_sets SET
                    max_position_pct = ?1,
                    max_daily_trades = ?2,
                    stop_loss_pct = ?3,
                    take_profit_pct = ?4,
                    min_confidence_to_trade = ?5,
                    max_daily_drawdown_pct = ?6,
                    position_size_pct = ?7,
                    allowed_symbols = NULL
                 WHERE is_active = 1",
                rusqlite::params![
                    strategy.max_position_pct,
                    strategy.max_daily_trades,
                    strategy.stop_loss_pct,
                    strategy.take_profit_pct,
                    strategy.min_confidence_to_trade,
                    strategy.max_daily_drawdown_pct,
                    strategy.position_size_pct,
                ],
            )?;
            if updated == 0 {
                anyhow::bail!("No active strategy param set found");
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;

    get_strategy(state)
}

#[tauri::command]
pub async fn start_backtest(
    strategy_param_set_id: String,
    start_date: String,
    end_date: String,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let run_id = state
        .db
        .with_conn(|conn| BacktestService::start_run(conn, &strategy_param_set_id, &start_date, &end_date))
        .map_err(|e| e.to_string())?;

    let state_clone = state.inner().clone();
    let run_id_clone = run_id.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            let settings = state_clone.db.with_conn(get_settings).unwrap_or_default();
            state_clone.db.with_conn(|conn| {
                block_on_local(BacktestService::run_async(
                    conn,
                    &state_clone.agent,
                    &settings,
                    &run_id_clone,
                    &strategy_param_set_id,
                    &start_date,
                    &end_date,
                ))
            })
        })
        .await;
    });

    Ok(run_id)
}

#[tauri::command]
pub fn get_backtest(run_id: String, state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    state
        .db
        .with_conn(|conn| BacktestService::get_run(conn, &run_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Backtest not found".to_string())
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
