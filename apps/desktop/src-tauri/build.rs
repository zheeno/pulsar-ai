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

    tauri_build::build()
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
