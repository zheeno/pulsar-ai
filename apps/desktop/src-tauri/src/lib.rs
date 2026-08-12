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
mod scheduler;
mod secrets;
mod seed;
mod settings;
mod signals;

use tauri::Manager;

use app_state::AppState;
use db::Database;
use seed::SeedService;
use settings::get_settings;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();

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
            commands::test_pulse_login,
            commands::test_llm,
            commands::portfolio_default,
            commands::portfolio_performance,
            commands::usage_ngx_pulse,
            commands::cycle_run,
            commands::cycle_ingest,
            commands::generate_signals,
            commands::list_signals,
            commands::list_trades,
            commands::get_strategy,
            commands::start_backtest,
            commands::get_backtest,
            commands::export_database,
            commands::app_data_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
