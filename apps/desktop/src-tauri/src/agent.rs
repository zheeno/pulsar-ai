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

use crate::net_policy::validate_llm_base_url;
use crate::secrets::{get_secret, SECRET_LLM_API_KEY};
use crate::settings::AppSettings;

const AGENT_IPC_TIMEOUT: Duration = Duration::from_secs(90);

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
        self.call(json!({ "op": "ping" }))
    }

    pub async fn ping(&self) -> Result<Value> {
        self.ping_sync()
    }

    pub fn test_llm_sync(&self, settings: &AppSettings) -> Result<Value> {
        let llm = self.build_llm_config(settings)?;
        self.call(json!({ "op": "test_llm", "llm": llm }))
    }

    pub async fn test_llm(&self, settings: &AppSettings) -> Result<Value> {
        self.test_llm_sync(settings)
    }

    pub fn portfolio_signals_sync(&self, settings: &AppSettings, context: Value) -> Result<Value> {
        let llm = self.build_llm_config(settings)?;
        self.call(json!({ "op": "portfolio_signals", "context": context, "llm": llm }))
    }

    pub async fn portfolio_signals(&self, settings: &AppSettings, context: Value) -> Result<Value> {
        self.portfolio_signals_sync(settings, context)
    }

    pub fn symbol_signal_sync(&self, settings: &AppSettings, context: Value) -> Result<Value> {
        let llm = self.build_llm_config(settings)?;
        self.call(json!({ "op": "symbol_signal", "context": context, "llm": llm }))
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

    fn call(&self, mut payload: Value) -> Result<Value> {
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
                let mut response_line = String::new();
                process.stdout.read_line(&mut response_line).context("read agent")?;
                let response: Value =
                    serde_json::from_str(response_line.trim()).context("parse agent response")?;
                if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                    let err = response
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Agent error");
                    return Err(anyhow!(err.to_string()));
                }
                Ok(response.get("data").cloned().unwrap_or(json!({})))
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
    if cfg!(debug_assertions) {
        if let Ok(path) = std::env::var("NGX_NODE_BIN") {
            return PathBuf::from(path);
        }
    }
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
    let bytes = std::fs::read(path).context("read bundled agent worker")?;
    let digest = Sha256::digest(&bytes);
    let actual = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if actual != expected {
        return Err(anyhow!("Bundled agent worker digest mismatch"));
    }
    Ok(())
}

/// Prefer bundled resource worker in packaged apps; fall back to repo path in dev.
pub fn resolve_worker_path(resource_dir: Option<&Path>) -> PathBuf {
    if cfg!(debug_assertions) {
        if let Ok(path) = std::env::var("NGX_AGENT_WORKER") {
            return PathBuf::from(path);
        }
    }

    if let Some(dir) = resource_dir {
        let bundled = dir.join("agent-worker.cjs");
        if bundled.is_file() {
            return bundled;
        }
        let nested = dir.join("resources").join("agent-worker.cjs");
        if nested.is_file() {
            return nested;
        }
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let packaged = manifest.join("resources/agent-worker.cjs");
    if packaged.is_file() {
        return packaged;
    }

    manifest
        .join("../../../packages/agent/dist/worker.js")
        .canonicalize()
        .unwrap_or_else(|_| manifest.join("../../../packages/agent/dist/worker.js"))
}
