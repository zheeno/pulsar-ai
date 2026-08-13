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
mod wealth;

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
    tracing::warn!("no .env found; will try bundled app.env in setup");
}

fn load_bundled_app_env(resource_dir: &std::path::Path) {
    for candidate in [
        resource_dir.join("app.env"),
        resource_dir.join("resources").join("app.env"),
    ] {
        if !candidate.is_file() {
            continue;
        }
        match std::fs::read_to_string(&candidate) {
            Ok(text) => {
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    let Some((key, value)) = trimmed.split_once('=') else {
                        continue;
                    };
                    // Bundled production config must win over any ambient .env from the build machine.
                    std::env::set_var(key.trim(), value.trim());
                }
                tracing::info!("loaded bundled env from {}", candidate.display());
                return;
            }
            Err(e) => tracing::warn!("failed to read {}: {e}", candidate.display()),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ngx_pulse=debug,wealth=debug")),
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
            let resource_dir = app.path().resource_dir().ok();
            if let Some(ref dir) = resource_dir {
                // Bundled app.env wins for shipped builds (APP_ENV=production + Pulse URLs).
                load_bundled_app_env(dir);
            }

            let worker_path = crate::agent::resolve_worker_path(resource_dir.as_deref());
            tracing::info!(worker = %worker_path.display(), "agent worker path");

            let app_data = app.path().app_data_dir().expect("app data dir");
            let db = Database::open(&app_data).expect("open database");
            let settings = db.with_conn(get_settings).unwrap_or_default();
            db.with_conn(|conn| SeedService::seed_if_empty(conn, settings.default_starting_capital))
                .expect("seed database");

            let state = AppState::new(db, worker_path);
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
            commands::wealth_login,
            commands::wealth_verify_2fa,
            commands::wealth_profile,
            commands::wealth_logout,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
