mod agent;
mod app_state;
mod bamboo;
mod broker;
mod cache;
mod calendar;
mod commands;
mod cycle_auth;
mod db;
mod execution;
mod http_client;
mod indicators;
mod ingest;
mod intents;
mod memory;
mod net_policy;
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

use tauri::path::BaseDirectory;
use tauri::Manager;

use app_state::AppState;
use db::Database;
use seed::SeedService;
use settings::get_settings;

const COMPILED_PULSE_BASE_URL: &str = env!("NGX_PULSE_BASE_URL");
const COMPILED_PULSE_SUPABASE_URL: &str = env!("NGX_PULSE_SUPABASE_URL");
const COMPILED_PULSE_ANON_KEY: &str = env!("NGX_PULSE_SUPABASE_ANON_KEY");

fn is_placeholder_pulse_value(key: &str, value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    match key {
        "NGX_PULSE_SUPABASE_URL" => {
            value.contains("your-project.supabase.co") || value.contains("your-project")
        }
        "NGX_PULSE_SUPABASE_ANON_KEY" => {
            value == "your-anon-key" || value.starts_with("your-anon")
        }
        _ => false,
    }
}

fn set_env_skip_placeholder(key: &str, value: &str) {
    let value = value.trim();
    if is_placeholder_pulse_value(key, value) {
        return;
    }
    match std::env::var(key) {
        Ok(existing) if !is_placeholder_pulse_value(key, &existing) => {}
        _ => std::env::set_var(key, value),
    }
}

fn apply_compiled_pulse_env() {
    set_env_skip_placeholder("NGX_PULSE_BASE_URL", COMPILED_PULSE_BASE_URL);
    set_env_skip_placeholder("NGX_PULSE_SUPABASE_URL", COMPILED_PULSE_SUPABASE_URL);
    set_env_skip_placeholder("NGX_PULSE_SUPABASE_ANON_KEY", COMPILED_PULSE_ANON_KEY);

    let url = std::env::var("NGX_PULSE_SUPABASE_URL").unwrap_or_default();
    let has_anon = std::env::var("NGX_PULSE_SUPABASE_ANON_KEY")
        .ok()
        .is_some_and(|k| !is_placeholder_pulse_value("NGX_PULSE_SUPABASE_ANON_KEY", &k));
    tracing::info!(
        supabase_url = %url,
        anon_key = has_anon,
        "pulse env ready"
    );
}

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
    tracing::warn!("no .env found; will try bundled app.env / compiled Pulse defaults");
}

fn load_bundled_app_env(resource_dir: &std::path::Path, extras: &[PathBuf]) {
    let mut candidates = extras.to_vec();
    candidates.extend(crate::agent::bundled_resource_candidates(
        Some(resource_dir),
        "app.env",
    ));
    for candidate in candidates {
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
                    let key = key.trim();
                    let value = value.trim();
                    if key.starts_with("NGX_PULSE_") {
                        set_env_skip_placeholder(key, value);
                        continue;
                    }
                    // Shipped APP_ENV=production (and other non-Pulse keys) still override ambient .env.
                    if cfg!(dev) && key == "APP_ENV" && std::env::var("APP_ENV").is_ok() {
                        continue;
                    }
                    std::env::set_var(key, value);
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
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let resource_dir = app.path().resource_dir().ok();
            let mut env_extras = Vec::new();
            let mut worker_extras = Vec::new();
            for rel in ["app.env", "resources/app.env"] {
                if let Ok(p) = app.path().resolve(rel, BaseDirectory::Resource) {
                    env_extras.push(p);
                }
            }
            for rel in ["agent-worker.cjs", "resources/agent-worker.cjs"] {
                if let Ok(p) = app.path().resolve(rel, BaseDirectory::Resource) {
                    worker_extras.push(p);
                }
            }
            if let Some(ref dir) = resource_dir {
                load_bundled_app_env(dir, &env_extras);
            }
            // Compile-time Pulse config from repo .env fills gaps and beats placeholder app.env.
            apply_compiled_pulse_env();

            let worker_path = crate::agent::resolve_worker_path(resource_dir.as_deref(), &worker_extras);
            tracing::info!(worker = %worker_path.display(), exists = worker_path.is_file(), "agent worker path");

            let app_data = app.path().app_data_dir().expect("app data dir");
            crate::secrets::init(&app_data);
            crate::secrets::preload();
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
            commands::portfolio_quotes,
            commands::portfolio_performance,
            commands::usage_ngx_pulse,
            commands::market_status,
            commands::cycle_status,
            commands::cycle_run,
            commands::cycle_ingest,
            commands::generate_signals,
            commands::list_signals,
            commands::list_trades,
            commands::memory_list,
            commands::memory_search,
            commands::memory_delete,
            commands::symbol_detail,
            commands::symbol_detail_pulse,
            commands::get_strategy,
            commands::update_strategy,
            commands::export_database,
            commands::app_data_dir,
            commands::reset_local_data,
            commands::wealth_login,
            commands::wealth_verify_2fa,
            commands::wealth_profile,
            commands::wealth_logout,
            commands::bamboo_login,
            commands::bamboo_profile,
            commands::bamboo_logout,
            commands::broker_list,
            commands::set_selected_broker,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
