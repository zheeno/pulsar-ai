use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::app_state::AppState;

pub fn start_scheduler(app: AppHandle, state: Arc<AppState>) {
    let app_schedule = app.clone();
    let state_schedule = state.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30 * 60));
        loop {
            interval.tick().await;
            let calendar = crate::calendar::TradingCalendar::default();
            if !calendar.is_market_open() && !calendar.is_post_close_window() {
                continue;
            }
            let _ = run_scheduled_ingest(&app_schedule, &state_schedule).await;
        }
    });

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = catch_up_on_launch(&app, &state).await;
    });
}

async fn catch_up_on_launch(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<()> {
    let settings = state.db.with_conn(|conn| crate::settings::get_settings(conn))?;
    let pulse_password = crate::secrets::get_secret(crate::secrets::SECRET_PULSE_PASSWORD)?;
    let pulse_api_key = crate::secrets::get_secret(crate::secrets::SECRET_PULSE_API_KEY)?;
    let client = crate::ngx::NgxPulseClient::from_settings(&settings, pulse_password, pulse_api_key);
    let calendar = crate::calendar::TradingCalendar::default();

    state.db.with_conn(|conn| {
        tauri::async_runtime::block_on(async {
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

async fn run_scheduled_ingest(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<()> {
    let settings = state.db.with_conn(|conn| crate::settings::get_settings(conn))?;
    let pulse_password = crate::secrets::get_secret(crate::secrets::SECRET_PULSE_PASSWORD)?;
    let pulse_api_key = crate::secrets::get_secret(crate::secrets::SECRET_PULSE_API_KEY)?;
    let client = crate::ngx::NgxPulseClient::from_settings(&settings, pulse_password, pulse_api_key);
    let calendar = crate::calendar::TradingCalendar::default();

    state.db.with_conn(|conn| {
        tauri::async_runtime::block_on(async {
            let _ = crate::ingest::IngestionService::ingest_stocks(conn, &client, &state.cache, &calendar, false).await;
            let _ = crate::ingest::IngestionService::ingest_market(conn, &client, &calendar, false).await;
            Ok(())
        })
    })?;

    let _ = app.emit("ingest:complete", ());
    Ok(())
}
