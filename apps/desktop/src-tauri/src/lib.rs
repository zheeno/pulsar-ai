mod agent;
mod app_state;
mod backtest;
mod cache;
mod calendar;
mod commands;
mod db;
mod execution;
mod indicators;
mod ingest;
mod ngx;
mod portfolio;
mod rate_limit;
mod runtime_util;
mod scheduler;
mod secrets;
mod seed;
mod settings;
mod signals;

use std::path::PathBuf;

use tauri::Manager;

use app_state::AppState;
use db::Database;
use seed::SeedService;
use settings::get_settings;

/// Load repo-root `.env` for Pulse Supabase / base URL (email+password stay user-entered).
fn load_dotenv() {
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    for start in candidates {
        let mut path = start;
        for _ in 0..6 {
            let candidate = path.join(".env");
            if candidate.is_file() {
                match dotenvy::from_path(&candidate) {
                    Ok(_) => {
                        tracing::info!("loaded env from {}", candidate.display());
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("failed to load {}: {e}", candidate.display());
                    }
                }
            }
            if !path.pop() {
                break;
            }
        }
    }
    tracing::warn!("no .env found; NGX_PULSE_SUPABASE_* must be set in the process environment");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ngx_pulse=debug")),
        )
        .init();
    load_dotenv();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let app_data = app.path().app_data_dir().expect("app data dir");
            let db = Database::open(&app_data).expect("open database");
            let settings = db.with_conn(get_settings).unwrap_or_default();
            db.with_conn(|conn| SeedService::seed_if_empty(conn, settings.default_starting_capital))
                .expect("seed database");

            let state = AppState::new(db);
            app.manage(state.clone());

            scheduler::start_scheduler(app.handle().clone(), state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::settings_get,
            commands::settings_set,
            commands::logout,
            commands::test_pulse_login,
            commands::test_llm,
            commands::llm_status,
            commands::pulse_profile,
            commands::complete_onboarding,
            commands::portfolio_default,
            commands::portfolio_performance,
            commands::usage_ngx_pulse,
            commands::market_status,
            commands::cycle_status,
            commands::cycle_run,
            commands::cycle_ingest,
            commands::generate_signals,
            commands::list_signals,
            commands::list_trades,
            commands::symbol_detail,
            commands::symbol_detail_pulse,
            commands::get_strategy,
            commands::update_strategy,
            commands::start_backtest,
            commands::get_backtest,
            commands::export_database,
            commands::app_data_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
