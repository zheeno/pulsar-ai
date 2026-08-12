use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use tokio::time::MissedTickBehavior;

use crate::app_state::AppState;
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
                Ok(Ok(())) => {
                    tracing::info!(target: "scheduler", "auto cycle completed");
                }
                Ok(Err(e)) => {
                    tracing::warn!(target: "scheduler", error = %e, "auto cycle failed");
                }
                Err(e) => {
                    tracing::warn!(target: "scheduler", error = %e, "auto cycle task join failed");
                }
            }
            last_cycle_at = Some(Instant::now());
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

fn run_scheduled_cycle(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<()> {
    let settings = state.db.with_conn(get_settings)?;
    if !settings.auto_cycle_enabled || !settings.onboarding_complete {
        return Ok(());
    }

    let pulse_password = get_secret(SECRET_PULSE_PASSWORD)?;
    let pulse_api_key = get_secret(SECRET_PULSE_API_KEY)?;
    let client = crate::ngx::NgxPulseClient::from_settings(&settings, pulse_password, pulse_api_key);
    let calendar = TradingCalendar::default();

    let result = state.db.with_conn(|conn| {
        block_on_local(run_cycle(
            conn,
            &state.agent,
            &settings,
            &state.cache,
            &client,
            &calendar,
        ))
    })?;

    let _ = app.emit("cycle:complete", result);
    Ok(())
}
