use std::fs;
use std::path::{Path, PathBuf};

fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let mut value = value.trim().to_string();
        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            value = value[1..value.len() - 1].to_string();
        }
        out.push((key.trim().to_string(), value));
    }
    out
}

fn find_repo_env(manifest_dir: &Path) -> Option<PathBuf> {
    let mut path = manifest_dir.to_path_buf();
    for _ in 0..6 {
        let candidate = path.join(".env");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !path.pop() {
            break;
        }
    }
    None
}

fn emit_pulse_env(key: &str, file_env: &[(String, String)]) {
    let value = std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            file_env
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        })
        .unwrap_or_default();
    println!("cargo:rustc-env={key}={value}");
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut file_env = Vec::new();
    if let Some(env_path) = find_repo_env(&manifest_dir) {
        println!("cargo:rerun-if-changed={}", env_path.display());
        if let Ok(text) = fs::read_to_string(&env_path) {
            file_env = parse_dotenv(&text);
        }
    } else {
        println!(
            "cargo:rerun-if-changed={}",
            manifest_dir
                .join("../../../.env")
                .display()
        );
    }

    emit_pulse_env("NGX_PULSE_BASE_URL", &file_env);
    emit_pulse_env("NGX_PULSE_SUPABASE_URL", &file_env);
    emit_pulse_env("NGX_PULSE_SUPABASE_ANON_KEY", &file_env);
    embed_agent_worker(&manifest_dir);
    generate_auth_bridge(&manifest_dir);

    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&[
                "ping",
                "settings_get",
                "settings_set",
                "confidence_journal",
                "list_cycle_audits",
                "logout",
                "test_pulse_login",
                "test_llm",
                "llm_status",
                "pulse_profile",
                "complete_onboarding",
                "portfolio_default",
                "portfolio_quotes",
                "portfolio_performance",
                "usage_ngx_pulse",
                "market_status",
                "cycle_status",
                "cycle_run",
                "cycle_ingest",
                "generate_signals",
                "list_signals",
                "list_trades",
                "memory_list",
                "memory_search",
                "memory_delete",
                "symbol_detail",
                "symbol_detail_pulse",
                "symbol_detail_busha",
                "get_strategy",
                "update_strategy",
                "desk_advice",
                "strategy_coach_propose",
                "strategy_coach_apply",
                "coach_list_sessions",
                "coach_get_session",
                "coach_new_session",
                "coach_delete_session",
                "coach_turn",
                "coach_execute_trade",
                "coach_cancel_trade",
                "export_database",
                "app_data_dir",
                "reset_local_data",
                "wealth_login",
                "wealth_verify_2fa",
                "wealth_profile",
                "wealth_logout",
                "bamboo_login",
                "bamboo_profile",
                "bamboo_logout",
                "broker_list",
                "set_selected_broker",
                "auth_bridge_authenticate",
                "auth_bridge_session",
                "auth_bridge_list",
                "auth_bridge_revoke",
                "auth_bridge_submit_candidate",
            ]),
        ),
    )
    .expect("tauri build");
}

fn embed_agent_worker(manifest_dir: &Path) {
    use sha2::{Digest, Sha256};
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let dest = out_dir.join("agent-worker.cjs");
    let candidates = [
        manifest_dir.join("resources/agent-worker.cjs"),
        manifest_dir.join("../../../packages/agent/dist/worker.js"),
    ];
    for path in candidates {
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_file() {
            if let Ok(bytes) = fs::read(&path) {
                let digest = Sha256::digest(&bytes);
                let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
                fs::copy(&path, &dest).expect("copy agent worker into OUT_DIR");
                println!("cargo:rustc-env=AGENT_WORKER_SHA256={hex}");
                return;
            }
        }
    }
    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile == "release" {
        panic!(
            "agent worker missing (resources/agent-worker.cjs). Run `npm run desktop:prepare` before a release build."
        );
    }
    fs::write(&dest, b"// agent worker not bundled in this debug build\n")
        .expect("write placeholder agent worker");
    println!("cargo:rustc-env=AGENT_WORKER_SHA256=");
}

fn generate_auth_bridge(manifest_dir: &Path) {
    let brokers_dir = manifest_dir.join("src/auth_bridge/brokers");
    println!("cargo:rerun-if-changed={}", brokers_dir.display());

    let mut files: Vec<String> = Vec::new();
    let mut ipc_urls: Vec<String> = vec![
        "http://127.0.0.1:*/*".into(),
        "http://localhost:*/*".into(),
    ];
    if let Ok(entries) = fs::read_dir(&brokers_dir) {
        let mut names: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            println!("cargo:rerun-if-changed={}", path.display());
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            files.push(name.clone());
            if let Ok(text) = fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(origins) = v.get("allowedOrigins").and_then(|x| x.as_array()) {
                        for origin in origins {
                            let Some(o) = origin.as_str() else { continue };
                            if is_idp_origin(o) {
                                continue;
                            }
                            let pattern = origin_to_url_pattern(o);
                            if !ipc_urls.contains(&pattern) {
                                ipc_urls.push(pattern);
                            }
                        }
                    }
                }
            }
        }
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut rust = String::from(
        "pub fn embedded_broker_json() -> &'static [&'static str] {\n    &[\n",
    );
    for name in &files {
        rust.push_str(&format!(
            "        include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/auth_bridge/brokers/{name}\")),\n"
        ));
    }
    rust.push_str("    ]\n}\n");
    fs::write(out_dir.join("auth_bridge_brokers.rs"), rust).expect("write auth_bridge_brokers.rs");

    let cap = serde_json::json!({
        "$schema": "../gen/schemas/desktop-schema.json",
        "identifier": "auth-bridge",
        "description": "Remote partner login webviews may only submit a capture candidate",
        "windows": ["auth-bridge-*"],
        "local": false,
        "remote": { "urls": ipc_urls },
        "permissions": ["allow-auth-bridge-submit-candidate"]
    });
    let cap_path = manifest_dir.join("capabilities/auth-bridge.json");
    let pretty = serde_json::to_string_pretty(&cap).unwrap() + "\n";
    if fs::read_to_string(&cap_path).unwrap_or_default() != pretty {
        fs::write(&cap_path, pretty).expect("write auth-bridge capability");
    }
}

fn is_idp_origin(origin: &str) -> bool {
    let o = origin.to_ascii_lowercase();
    o.contains("google.")
        || o.contains("youtube.com")
        || o.contains("apple.com")
        || o.contains("facebook.com")
        || o.contains("microsoftonline")
}

fn origin_to_url_pattern(origin: &str) -> String {
    let o = origin.trim().trim_end_matches('/');
    if o.ends_with("/*") {
        o.to_string()
    } else {
        format!("{o}/*")
    }
}
