use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::Database;
use crate::memory;
use crate::net_policy::validate_llm_base_url;
use crate::secrets::{get_secret, SECRET_LLM_API_KEY};
use crate::settings::AppSettings;

const AGENT_IPC_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_TOOL_ROUNDS: u32 = 6;

pub fn parse_ipc_line(line: &str) -> Result<Value> {
    serde_json::from_str(line.trim()).context("parse agent ipc")
}

pub fn is_tool_message(msg: &Value) -> bool {
    msg.get("type").and_then(|v| v.as_str()) == Some("tool")
}

pub struct AgentBridge {
    child: Arc<Mutex<Option<AgentProcess>>>,
    worker_path: PathBuf,
}

struct AgentProcess {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl AgentBridge {
    pub fn new(worker_path: PathBuf) -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            worker_path,
        }
    }

    pub fn ping_sync(&self) -> Result<Value> {
        self.call(json!({ "op": "ping" }), None)
    }

    pub async fn ping(&self) -> Result<Value> {
        self.ping_sync()
    }

    pub fn test_llm_sync(&self, settings: &AppSettings) -> Result<Value> {
        let llm = self.build_llm_config(settings)?;
        self.call(json!({ "op": "test_llm", "llm": llm }), None)
    }

    pub async fn test_llm(&self, settings: &AppSettings) -> Result<Value> {
        self.test_llm_sync(settings)
    }

    pub fn portfolio_signals_sync(
        &self,
        settings: &AppSettings,
        context: Value,
        db: &Database,
    ) -> Result<Value> {
        let llm = self.build_llm_config(settings)?;
        self.call(
            json!({ "op": "portfolio_signals", "context": context, "llm": llm }),
            Some((db.clone(), settings.clone())),
        )
    }

    pub async fn portfolio_signals(
        &self,
        settings: &AppSettings,
        context: Value,
        db: &Database,
    ) -> Result<Value> {
        self.portfolio_signals_sync(settings, context, db)
    }

    pub fn symbol_signal_sync(&self, settings: &AppSettings, context: Value) -> Result<Value> {
        let llm = self.build_llm_config(settings)?;
        self.call(json!({ "op": "symbol_signal", "context": context, "llm": llm }), None)
    }

    pub async fn symbol_signal(&self, settings: &AppSettings, context: Value) -> Result<Value> {
        self.symbol_signal_sync(settings, context)
    }

    fn build_llm_config(&self, settings: &AppSettings) -> Result<Value> {
        let api_key = get_secret(SECRET_LLM_API_KEY)?.unwrap_or_default();
        if api_key.is_empty() {
            return Err(anyhow!("LLM API key not configured"));
        }
        let mut llm = json!({
            "provider": settings.llm_provider,
            "model": settings.llm_model,
            "apiKey": api_key,
        });
        // Zod optional() rejects null — omit empty/unset baseUrl for OpenAI defaults.
        if let Some(url) = settings.llm_base_url.as_ref().filter(|u| !u.is_empty()) {
            let normalized = validate_llm_base_url(url, true)?;
            llm["baseUrl"] = json!(normalized);
        }
        Ok(llm)
    }

    fn call(
        &self,
        mut payload: Value,
        tools: Option<(Database, AppSettings)>,
    ) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("id".into(), json!(id));
        }

        let line = serde_json::to_string(&payload)?;
        let child = self.child.clone();
        let worker_path = self.worker_path.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| {
                let mut guard = child.lock().unwrap();
                if guard.is_none() {
                    *guard = Some(spawn_worker(&worker_path)?);
                }
                let process = guard.as_mut().unwrap();
                writeln!(process.stdin, "{line}").context("write agent")?;
                process.stdin.flush()?;
                let api_key = tools
                    .as_ref()
                    .map(|(_, _s)| get_secret(SECRET_LLM_API_KEY).ok().flatten().unwrap_or_default())
                    .unwrap_or_default();
                let mut rounds = 0u32;
                loop {
                    let mut response_line = String::new();
                    process
                        .stdout
                        .read_line(&mut response_line)
                        .context("read agent")?;
                    let response = parse_ipc_line(&response_line)?;
                    if is_tool_message(&response) {
                        rounds += 1;
                        if rounds > MAX_TOOL_ROUNDS {
                            anyhow::bail!("Agent exceeded memory tool round limit");
                        }
                        let name = response
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args = response.get("arguments").cloned().unwrap_or(json!({}));
                        let result = if let Some((db, settings)) = tools.as_ref() {
                            memory::handle_tool(db, settings, &api_key, name, &args)
                                .unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() }))
                        } else {
                            json!({ "ok": false, "error": "memory tools unavailable" })
                        };
                        let reply = json!({
                            "type": "tool_result",
                            "name": name,
                            "result": result,
                        });
                        writeln!(process.stdin, "{}", serde_json::to_string(&reply)?)
                            .context("write tool result")?;
                        process.stdin.flush()?;
                        continue;
                    }
                    if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                        let err = response
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Agent error");
                        return Err(anyhow!(err.to_string()));
                    }
                    return Ok(response.get("data").cloned().unwrap_or(json!({})));
                }
            })();
            let _ = tx.send(result);
        });

        match rx.recv_timeout(AGENT_IPC_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                if let Ok(mut guard) = self.child.lock() {
                    if let Some(mut proc) = guard.take() {
                        let _ = proc._child.kill();
                    }
                }
                Err(anyhow!("Agent worker timed out and was restarted"))
            }
        }
    }
}

fn resolve_node_bin() -> PathBuf {
    if let Ok(path) = std::env::var("NGX_NODE_BIN") {
        let p = PathBuf::from(&path);
        if p.is_file() {
            return p;
        }
    }
    #[cfg(windows)]
    {
        return resolve_node_bin_windows();
    }
    #[cfg(not(windows))]
    {
        for candidate in [
            "/usr/local/bin/node",
            "/opt/homebrew/bin/node",
            "/usr/bin/node",
        ] {
            let p = PathBuf::from(candidate);
            if p.is_file() {
                return p;
            }
        }
        PathBuf::from("node")
    }
}

#[cfg(windows)]
fn resolve_node_bin_windows() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("ProgramFiles") {
        candidates.push(PathBuf::from(p).join("nodejs").join("node.exe"));
    }
    if let Ok(p) = std::env::var("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(p).join("nodejs").join("node.exe"));
    }
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        let local = PathBuf::from(p);
        candidates.push(local.join("Programs").join("nodejs").join("node.exe"));
        candidates.push(local.join("fnm").join("aliases").join("default").join("node.exe"));
        candidates.push(local.join("nvs").join("default").join("node.exe"));
    }
    if let Ok(nvm) = std::env::var("NVM_SYMLINK") {
        candidates.push(PathBuf::from(nvm).join("node.exe"));
    }
    candidates.push(PathBuf::from(r"C:\Program Files\nodejs\node.exe"));
    for p in &candidates {
        if p.is_file() {
            return p.clone();
        }
    }
    if let Ok(out) = Command::new("where.exe").arg("node.exe").output() {
        if out.status.success() {
            if let Some(line) = String::from_utf8_lossy(&out.stdout).lines().next() {
                let p = PathBuf::from(line.trim());
                if p.is_file() {
                    return p;
                }
            }
        }
    }
    PathBuf::from("node.exe")
}

fn spawn_worker(path: &PathBuf) -> Result<AgentProcess> {
    verify_bundled_worker(path)?;
    let node = resolve_node_bin();
    let mut child = Command::new(&node)
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| {
            format!(
                "spawn agent worker with {} — install Node.js 20+ or set NGX_NODE_BIN",
                node.display()
            )
        })?;

    let stdin = child.stdin.take().context("agent stdin")?;
    let stdout = child.stdout.take().context("agent stdout")?;
    Ok(AgentProcess {
        _child: child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

fn verify_bundled_worker(path: &Path) -> Result<()> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    let expected = option_env!("AGENT_WORKER_SHA256").unwrap_or("");
    if expected.is_empty() {
        return Err(anyhow!("Release build is missing AGENT_WORKER_SHA256"));
    }
    let bytes = std::fs::read(path).with_context(|| {
        format!("read bundled agent worker ({})", path.display())
    })?;
    let digest = Sha256::digest(&bytes);
    let actual = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if actual != expected {
        return Err(anyhow!("Bundled agent worker digest mismatch"));
    }
    Ok(())
}

/// Locations where a Tauri-bundled file may land (Windows NSIS uses `_up_`).
pub fn bundled_resource_candidates(resource_dir: Option<&Path>, filename: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<PathBuf>, p: PathBuf| {
        if !out.iter().any(|e| e == &p) {
            out.push(p);
        }
    };
    if let Some(dir) = resource_dir {
        push(&mut out, dir.join(filename));
        push(&mut out, dir.join("resources").join(filename));
        push(&mut out, dir.join("_up_").join(filename));
        push(&mut out, dir.join("_up_").join("resources").join(filename));
        if let Some(parent) = dir.parent() {
            push(&mut out, parent.join(filename));
            push(&mut out, parent.join("resources").join(filename));
        }
    }
    out
}

/// Prefer bundled resource worker in packaged apps; fall back to repo path in dev.
pub fn resolve_worker_path(resource_dir: Option<&Path>, extras: &[PathBuf]) -> PathBuf {
    if cfg!(debug_assertions) {
        if let Ok(path) = std::env::var("NGX_AGENT_WORKER") {
            return PathBuf::from(path);
        }
    }

    for extra in extras {
        if extra.is_file() {
            return extra.clone();
        }
    }

    for candidate in bundled_resource_candidates(resource_dir, "agent-worker.cjs") {
        if candidate.is_file() {
            return candidate;
        }
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let packaged = manifest.join("resources").join("agent-worker.cjs");
    if packaged.is_file() {
        return packaged;
    }

    let dist = manifest.join("../../../packages/agent/dist/worker.js");
    dist.canonicalize().unwrap_or(dist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_ipc_message() {
        let msg = parse_ipc_line(
            r#"{"type":"tool","name":"memory_search","arguments":{"query":"GTCO"}}"#,
        )
        .unwrap();
        assert!(is_tool_message(&msg));
        assert_eq!(msg.get("name").and_then(|v| v.as_str()), Some("memory_search"));
        let done = parse_ipc_line(r#"{"id":"1","ok":true,"data":{"signals":[]}}"#).unwrap();
        assert!(!is_tool_message(&done));
    }

    #[test]
    fn bundled_candidates_include_windows_nsis_layout() {
        let dir = Path::new("/Program Files/Pulsar AI/resources");
        let c = bundled_resource_candidates(Some(dir), "agent-worker.cjs");
        let rendered: Vec<String> = c.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert!(rendered.iter().any(|p| p.ends_with("/resources/agent-worker.cjs")));
        assert!(rendered.iter().any(|p| p.contains("/_up_/resources/agent-worker.cjs")));
        assert!(rendered.iter().any(|p| p.ends_with("/agent-worker.cjs") && !p.contains("/_up_/")));
    }
}
