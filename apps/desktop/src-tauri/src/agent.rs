use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::secrets::{get_secret, SECRET_LLM_API_KEY};
use crate::settings::AppSettings;

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
        Ok(json!({
            "provider": settings.llm_provider,
            "model": settings.llm_model,
            "apiKey": api_key,
            "baseUrl": settings.llm_base_url,
        }))
    }

    fn call(&self, mut payload: Value) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("id".into(), json!(id));
        }

        let line = serde_json::to_string(&payload)?;
        let mut guard = self.child.lock().unwrap();
        if guard.is_none() {
            *guard = Some(spawn_worker(&self.worker_path)?);
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
    }
}

fn spawn_worker(path: &PathBuf) -> Result<AgentProcess> {
    let mut child = Command::new("node")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawn agent worker")?;

    let stdin = child.stdin.take().context("agent stdin")?;
    let stdout = child.stdout.take().context("agent stdout")?;
    Ok(AgentProcess {
        _child: child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

pub fn resolve_worker_path() -> PathBuf {
    if let Ok(path) = std::env::var("NGX_AGENT_WORKER") {
        return PathBuf::from(path);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("../../../packages/agent/dist/worker.js")
        .canonicalize()
        .unwrap_or_else(|_| manifest.join("../../../packages/agent/dist/worker.js"))
}
