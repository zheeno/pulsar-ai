use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use tokio::time::MissedTickBehavior;

use crate::app_state::{AppState, CycleGateGuard};
use crate::calendar::TradingCalendar;
use crate::runtime_util::block_on_local;
use crate::secrets::{get_secret, SECRET_PULSE_API_KEY, SECRET_PULSE_PASSWORD};
use crate::settings::get_settings;
use crate::signals::run_cycle;

const SCHEDULER_POLL_SECS: u64 = 30;

pub fn start_scheduler(app: AppHandle, state: Arc<AppState>) {
    let app_cycle = app.clone();
    let state_cycle = state.clone();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(SCHEDULER_POLL_SECS));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_cycle_at: Option<Instant> = None;

        loop {
            ticker.tick().await;

            let settings = match state_cycle.db.with_conn(get_settings) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(target: "scheduler", error = %e, "failed to load settings");
                    continue;
                }
            };

            if !settings.auto_cycle_enabled || !settings.onboarding_complete {
                continue;
            }

            let interval =
                Duration::from_secs(u64::from(settings.auto_cycle_interval_minutes.clamp(5, 120)) * 60);
            if let Some(last) = last_cycle_at {
                if last.elapsed() < interval {
                    continue;
                }
            }

            let calendar = TradingCalendar::default();
            if !crate::runtime_util::market_activity_allowed(&calendar) {
                continue;
            }

            let app2 = app_cycle.clone();
            let state2 = state_cycle.clone();
            match tokio::task::spawn_blocking(move || run_scheduled_cycle(&app2, &state2)).await {
                Ok(Ok(true)) => {
                    tracing::info!(target: "scheduler", "auto cycle completed");
                    last_cycle_at = Some(Instant::now());
                }
                Ok(Ok(false)) => {
                    tracing::info!(target: "scheduler", "auto cycle skipped (already running)");
                }
                Ok(Err(e)) => {
                    tracing::warn!(target: "scheduler", error = %e, "auto cycle failed");
                    last_cycle_at = Some(Instant::now());
                }
                Err(e) => {
                    tracing::warn!(target: "scheduler", error = %e, "auto cycle task join failed");
                    last_cycle_at = Some(Instant::now());
                }
            }
        }
    });

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = tokio::task::spawn_blocking(move || catch_up_on_launch(&app, &state)).await;
    });
}

fn catch_up_on_launch(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<()> {
    let settings = state.db.with_conn(get_settings)?;
    let pulse_password = get_secret(SECRET_PULSE_PASSWORD)?;
    let pulse_api_key = get_secret(SECRET_PULSE_API_KEY)?;
    let client = crate::ngx::NgxPulseClient::from_settings(&settings, pulse_password, pulse_api_key);
    let calendar = TradingCalendar::default();

    state.db.with_conn(|conn| {
        block_on_local(async {
            let _ = crate::ingest::IngestionService::ingest_stocks(
                conn,
                &client,
                &state.cache,
                &calendar,
                false,
            )
            .await;
            Ok(())
        })
    })?;

    let _ = app.emit("ingest:complete", ());
    Ok(())
}

/// Returns `Ok(true)` if a cycle ran, `Ok(false)` if skipped because another cycle holds the gate.
fn run_scheduled_cycle(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<bool> {
    let settings = state.db.with_conn(get_settings)?;
    if !settings.auto_cycle_enabled || !settings.onboarding_complete {
        return Ok(false);
    }

    let Some(_gate) = CycleGateGuard::acquire(state.clone()) else {
        return Ok(false);
    };

    let _ = app.emit(
        "cycle:start",
        serde_json::json!({ "source": "scheduler" }),
    );

    let pulse_password = get_secret(SECRET_PULSE_PASSWORD)?;
    let pulse_api_key = get_secret(SECRET_PULSE_API_KEY)?;
    let client = crate::ngx::NgxPulseClient::from_settings(&settings, pulse_password, pulse_api_key);
    let wealth_password = crate::secrets::get_secret(crate::secrets::SECRET_WEALTH_PASSWORD)?;
    let wealth = if settings.wealth_connected {
        Some(crate::wealth::WealthClient::from_settings(
            &settings,
            wealth_password,
        ))
    } else {
        None
    };
    let calendar = TradingCalendar::default();

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
                obj.insert("source".into(), serde_json::json!("scheduler"));
            }
            let _ = app.emit("cycle:complete", payload);
            Ok(true)
        }
        Err(e) => {
            let payload = serde_json::json!({
                "ok": false,
                "source": "scheduler",
                "signals": 0,
                "executed": 0,
                "warnings": [],
                "error": e.to_string(),
            });
            let _ = app.emit("cycle:complete", payload);
            Err(e)
        }
    }
}
