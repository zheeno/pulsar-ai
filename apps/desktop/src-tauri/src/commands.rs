use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::backtest::BacktestService;
use crate::calendar::TradingCalendar;
use crate::ingest::IngestionService;
use crate::ngx::NgxPulseClient;
use crate::portfolio::PortfolioService;
use crate::rate_limit::RateLimiter;
use crate::secrets::{delete_secret, get_secret, set_secret, SECRET_LLM_API_KEY, SECRET_PULSE_API_KEY, SECRET_PULSE_PASSWORD};
use crate::settings::{get_settings, save_settings, AppSettings};
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
    state
        .db
        .with_conn(|conn| save_settings(conn, &payload.settings))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_pulse_login(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    let password = get_secret(SECRET_PULSE_PASSWORD).map_err(|e| e.to_string())?;
    let api_key = get_secret(SECRET_PULSE_API_KEY).map_err(|e| e.to_string())?;
    let client = NgxPulseClient::from_settings(&settings, password, api_key);
    client.test_login().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_llm(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    state.agent.test_llm(&settings).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn portfolio_default(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
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
    serde_json::to_value(summary).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn portfolio_performance(id: String, state: State<'_, Arc<AppState>>) -> Result<Vec<serde_json::Value>, String> {
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

#[tauri::command]
pub async fn cycle_run(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
    let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
    let client = NgxPulseClient::from_settings(&settings, password, api_key);
    let calendar = TradingCalendar::default();

    let db = &state.db;
    let cache = &state.cache;
    let agent = &state.agent;

    db.with_conn(|conn| {
        tauri::async_runtime::block_on(async {
            run_cycle(conn, agent, &settings, cache, &client, &calendar).await
        })
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cycle_ingest(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    let password = get_secret(SECRET_PULSE_PASSWORD).ok().flatten();
    let api_key = get_secret(SECRET_PULSE_API_KEY).ok().flatten();
    let client = NgxPulseClient::from_settings(&settings, password, api_key);
    let calendar = TradingCalendar::default();

    let count = state
        .db
        .with_conn(|conn| {
            tauri::async_runtime::block_on(async {
                IngestionService::ingest_stocks(conn, &client, &state.cache, &calendar, true).await
            })
        })
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({ "count": count }))
}

#[tauri::command]
pub async fn generate_signals(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    let ids = state
        .db
        .with_conn(|conn| {
            tauri::async_runtime::block_on(async {
                SignalGenerationService::generate_for_portfolio(conn, &state.agent, &settings, None).await
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "signalIds": ids, "count": ids.len() }))
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
            rows.filter_map(|r| r.ok()).collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_trades(limit: Option<i64>, state: State<'_, Arc<AppState>>) -> Result<Vec<serde_json::Value>, String> {
    let limit = limit.unwrap_or(50);
    state
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, symbol, side, quantity, fill_price, simulated_fee, executed_at, resulting_cash_balance
                 FROM sandbox_trades ORDER BY executed_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "symbol": row.get::<_, String>(1)?,
                    "side": row.get::<_, String>(2)?,
                    "quantity": row.get::<_, f64>(3)?,
                    "fill_price": row.get::<_, f64>(4)?,
                    "simulated_fee": row.get::<_, f64>(5)?,
                    "executed_at": row.get::<_, String>(6)?,
                    "resulting_cash_balance": row.get::<_, f64>(7)?,
                }))
            })?;
            rows.filter_map(|r| r.ok()).collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_strategy(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT id, name, max_position_pct, max_daily_trades, stop_loss_pct, min_confidence_to_trade, max_daily_drawdown_pct, position_size_pct, allowed_symbols, is_active
                 FROM strategy_param_sets WHERE is_active = 1 LIMIT 1",
                [],
                |row| {
                    let allowed: Option<String> = row.get(8)?;
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "name": row.get::<_, String>(1)?,
                        "max_position_pct": row.get::<_, f64>(2)?,
                        "max_daily_trades": row.get::<_, i64>(3)?,
                        "stop_loss_pct": row.get::<_, f64>(4)?,
                        "min_confidence_to_trade": row.get::<_, f64>(5)?,
                        "max_daily_drawdown_pct": row.get::<_, f64>(6)?,
                        "position_size_pct": row.get::<_, f64>(7)?,
                        "allowed_symbols": allowed.and_then(|s| serde_json::from_str(&s).ok()),
                        "is_active": row.get::<_, i64>(9)? == 1,
                    }))
                },
            )
        })
        .map_err(|e| e.to_string())
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
        let settings = state_clone.db.with_conn(get_settings).unwrap_or_default();
        let _ = state_clone.db.with_conn(|conn| {
            tauri::async_runtime::block_on(async {
                BacktestService::run_async(
                    conn,
                    &state_clone.agent,
                    &settings,
                    &run_id_clone,
                    &strategy_param_set_id,
                    &start_date,
                    &end_date,
                )
                .await
            })
        });
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
