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

const AGENT_IPC_TIMEOUT: Duration = Duration::from_secs(180);
/// Max memory/coach tool IPC messages per request.
const MAX_TOOL_CALLS: u32 = 8;

pub fn parse_ipc_line(line: &str) -> Result<Value> {
    serde_json::from_str(line.trim()).context("parse agent ipc")
}

pub fn is_tool_message(msg: &Value) -> bool {
    msg.get("type").and_then(|v| v.as_str()) == Some("tool")
}

pub struct AgentBridge {
    child: Arc<Mutex<Option<AgentProcess>>>,
    worker_path: PathBuf,
    node_bin: PathBuf,
}

struct AgentProcess {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl AgentBridge {
    pub fn new(worker_path: PathBuf, node_bin: PathBuf) -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            worker_path,
            node_bin,
        }
    }

    pub fn ping_sync(&self) -> Result<Value> {
        self.call(json!({ "op": "ping" }), None, None)
    }

    pub async fn ping(&self) -> Result<Value> {
        self.ping_sync()
    }

    pub fn test_llm_sync(&self, settings: &AppSettings) -> Result<Value> {
        let llm = self.build_llm_config(settings)?;
        self.call(json!({ "op": "test_llm", "llm": llm }), None, None)
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
            None,
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
        self.call(json!({ "op": "symbol_signal", "context": context, "llm": llm }), None, None)
    }

    pub async fn symbol_signal(&self, settings: &AppSettings, context: Value) -> Result<Value> {
        self.symbol_signal_sync(settings, context)
    }

    pub fn strategy_coach_sync(
        &self,
        settings: &AppSettings,
        context: Value,
        db: &Database,
        traces: Option<std::sync::Arc<std::sync::Mutex<Vec<Value>>>>,
    ) -> Result<Value> {
        let llm = self.build_llm_config(settings)?;
        self.call(
            json!({ "op": "strategy_coach", "context": context, "llm": llm }),
            Some((db.clone(), settings.clone())),
            traces,
        )
    }

    pub async fn strategy_coach(
        &self,
        settings: &AppSettings,
        context: Value,
        db: &Database,
    ) -> Result<Value> {
        self.strategy_coach_sync(settings, context, db, None)
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
        if let Some(t) = settings.llm_temperature {
            llm["temperature"] = json!(t);
        }
        Ok(llm)
    }

    fn call(
        &self,
        mut payload: Value,
        tools: Option<(Database, AppSettings)>,
        traces: Option<std::sync::Arc<std::sync::Mutex<Vec<Value>>>>,
    ) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("id".into(), json!(id));
        }

        let line = serde_json::to_string(&payload)?;
        let child = self.child.clone();
        let worker_path = self.worker_path.clone();
        let node_bin = self.node_bin.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| {
                let mut guard = child.lock().unwrap();
                if guard.is_none() {
                    *guard = Some(spawn_worker(&worker_path, &node_bin)?);
                }
                let process = guard.as_mut().unwrap();
                // Always LF: writeln! emits CRLF on Windows, which can leave `\r` on
                // Node readline when the worker is spawned without a TTY.
                write!(process.stdin, "{line}\n").context("write agent")?;
                process.stdin.flush()?;
                let api_key = tools
                    .as_ref()
                    .map(|(_, _s)| get_secret(SECRET_LLM_API_KEY).ok().flatten().unwrap_or_default())
                    .unwrap_or_default();
                let mut tool_calls = 0u32;
                loop {
                    let mut response_line = String::new();
                    process
                        .stdout
                        .read_line(&mut response_line)
                        .context("read agent")?;
                    let response = parse_ipc_line(&response_line)?;
                    if is_tool_message(&response) {
                        tool_calls += 1;
                        let name = response
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args = response.get("arguments").cloned().unwrap_or(json!({}));
                        let result = if tool_calls > MAX_TOOL_CALLS {
                            // Soft-fail so the worker can force a final JSON response instead of
                            // aborting the whole cycle.
                            json!({
                                "ok": false,
                                "error": "tool budget exhausted — finish without more tools",
                            })
                        } else if let Some((db, settings)) = tools.as_ref() {
                            dispatch_agent_tool(db, settings, &api_key, name, &args)
                        } else {
                            json!({ "ok": false, "error": "tools unavailable" })
                        };
                        if let Some(buf) = traces.as_ref() {
                            if let Ok(mut g) = buf.lock() {
                                g.push(json!({
                                    "name": name,
                                    "args": crate::coach::redact_args(&args),
                                    "ok": result.get("ok") != Some(&json!(false)),
                                    "summary": crate::coach::summarize_tool_result(name, &result),
                                }));
                            }
                        }
                        let reply = json!({
                            "type": "tool_result",
                            "name": name,
                            "result": result,
                        });
                        write!(process.stdin, "{}\n", serde_json::to_string(&reply)?)
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

fn dispatch_agent_tool(
    db: &Database,
    settings: &AppSettings,
    api_key: &str,
    name: &str,
    args: &Value,
) -> Value {
    if crate::coach::tool_refused_for_active_intent(name) {
        return json!({
            "ok": false,
            "refused": true,
            "error": format!("{name} must be confirmed in the UI, not called from chat"),
        });
    }
    let mapped = if name == "search_memory" {
        "memory_search"
    } else {
        name
    };
    match mapped {
        "memory_search" | "memory_upsert" => memory::handle_tool(db, settings, api_key, mapped, args)
            .unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() })),
        _ => crate::coach::handle_tool(db, settings, mapped, args),
    }
}

fn resolve_system_node_bin() -> PathBuf {
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

/// Prefer bundled Node from app resources, then NGX_NODE_BIN, then system PATH.
pub fn resolve_node_bin(resource_dir: Option<&Path>, extras: &[PathBuf]) -> PathBuf {
    if let Ok(path) = std::env::var("NGX_NODE_BIN") {
        let p = PathBuf::from(&path);
        if p.is_file() {
            tracing::info!(node = %p.display(), "using NGX_NODE_BIN");
            return p;
        }
    }

    for candidate in bundled_node_candidates(resource_dir, extras) {
        if candidate.is_file() {
            tracing::info!(node = %candidate.display(), "using bundled Node runtime");
            return candidate;
        }
    }

    let system = resolve_system_node_bin();
    tracing::info!(node = %system.display(), "using system Node runtime");
    system
}

pub fn bundled_node_candidates(resource_dir: Option<&Path>, extras: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = extras.to_vec();
    let push = |out: &mut Vec<PathBuf>, p: PathBuf| {
        if !out.iter().any(|e| e == &p) {
            out.push(p);
        }
    };
    for name in ["node/bin/node", "node/node.exe"] {
        for c in bundled_resource_candidates(resource_dir, name) {
            push(&mut out, c);
        }
        for c in exe_resource_candidates(name) {
            push(&mut out, c);
        }
    }
    if cfg!(debug_assertions) {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        push(&mut out, manifest.join("resources/node/bin/node"));
        push(&mut out, manifest.join("resources/node/node.exe"));
    }
    out
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

const EMBEDDED_WORKER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent-worker.cjs"));

/// Node on Windows cannot exec `\\?\C:\...` verbatim paths: it treats `C:` as
/// the script and dies with `EISDIR: lstat 'C:'`. Strip the prefix for argv.
fn path_for_node(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        if let Some(unc) = rest.strip_prefix(r"UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

fn spawn_worker(path: &Path, node_bin: &Path) -> Result<AgentProcess> {
    verify_bundled_worker(path)?;
    let node = if node_bin.is_file() {
        node_bin.to_path_buf()
    } else {
        resolve_system_node_bin()
    };
    let node_arg = path_for_node(path);
    let mut child = Command::new(&node)
        .arg(&node_arg)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| {
            format!(
                "spawn agent worker with {} — bundled Node missing; install Node.js 20+ or set NGX_NODE_BIN",
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
        format!(
            "read bundled agent worker ({}) — packaged builds must not use the developer machine path",
            path.display()
        )
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

fn exe_resource_candidates(filename: &str) -> Vec<PathBuf> {
    match std::env::current_exe() {
        Ok(exe) => exe
            .parent()
            .map(|dir| bundled_resource_candidates(Some(dir), filename))
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn install_embedded_worker(app_data: &Path) -> Result<PathBuf> {
    if EMBEDDED_WORKER.is_empty() || EMBEDDED_WORKER.starts_with(b"// agent worker not bundled") {
        anyhow::bail!("This build does not include the agent worker");
    }
    std::fs::create_dir_all(app_data).context("create app data dir for agent worker")?;
    let dest = app_data.join("agent-worker.cjs");
    let stale = match std::fs::read(&dest) {
        Ok(existing) => Sha256::digest(&existing) != Sha256::digest(EMBEDDED_WORKER),
        Err(_) => true,
    };
    if stale {
        std::fs::write(&dest, EMBEDDED_WORKER).context("write embedded agent worker")?;
    }
    Ok(dest)
}

/// Prefer an on-disk bundled worker; in release, extract the worker compiled into the exe
/// so the app never depends on the machine that built the installer.
pub fn resolve_worker_path(
    resource_dir: Option<&Path>,
    extras: &[PathBuf],
    app_data: Option<&Path>,
) -> PathBuf {
    if cfg!(debug_assertions) {
        if let Ok(path) = std::env::var("NGX_AGENT_WORKER") {
            return PathBuf::from(path);
        }
    } else if let Some(dir) = app_data {
        if let Ok(installed) = install_embedded_worker(dir) {
            return installed;
        }
    }

    for extra in extras {
        if extra.is_file() {
            return extra.clone();
        }
    }

    for candidate in bundled_resource_candidates(resource_dir, "agent-worker.cjs")
        .into_iter()
        .chain(exe_resource_candidates("agent-worker.cjs"))
    {
        if candidate.is_file() {
            return candidate;
        }
    }

    if cfg!(debug_assertions) {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let packaged = manifest.join("resources").join("agent-worker.cjs");
        if packaged.is_file() {
            return packaged;
        }
        let dist = manifest.join("../../../packages/agent/dist/worker.js");
        return dist.canonicalize().unwrap_or(dist);
    }

    PathBuf::from("agent-worker.cjs")
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

    #[test]
    fn bundled_node_candidates_include_mac_and_win_layouts() {
        let dir = Path::new("/Applications/Pulsar AI.app/Contents/Resources");
        let c = bundled_node_candidates(Some(dir), &[]);
        let rendered: Vec<String> = c.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert!(rendered.iter().any(|p| p.ends_with("/node/bin/node")));
        assert!(rendered.iter().any(|p| p.ends_with("/node/node.exe")));
    }

    #[test]
    fn path_for_node_strips_windows_verbatim_prefix() {
        let verbatim = PathBuf::from(r"\\?\C:\Users\efezi\agent-worker.cjs");
        let simplified = path_for_node(&verbatim);
        assert_eq!(simplified, PathBuf::from(r"C:\Users\efezi\agent-worker.cjs"));
        assert!(!simplified.to_string_lossy().starts_with(r"\\?\"));
        let unc = PathBuf::from(r"\\?\UNC\server\share\agent-worker.cjs");
        assert_eq!(
            path_for_node(&unc),
            PathBuf::from(r"\\server\share\agent-worker.cjs")
        );
        let already = PathBuf::from(r"C:\Users\efezi\agent-worker.cjs");
        assert_eq!(path_for_node(&already), already);
    }

    #[test]
    fn release_fallback_is_not_cargo_manifest() {
        if !cfg!(debug_assertions) {
            let path = resolve_worker_path(None, &[], None);
            let s = path.to_string_lossy().replace('\\', "/");
            assert!(
                !s.contains("src-tauri/../../../packages/agent"),
                "packaged app used build-machine path: {s}"
            );
        }
    }
}
