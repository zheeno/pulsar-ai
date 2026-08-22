use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, Url, WebviewWindow};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::capture::{self, digest};
use super::config::{self, origin_allowed, BrokerAuthConfig};
use super::probe;
use super::session::{
    self, AuthSessionStatus, SessionStatusKind, EXPIRED_EVENT, EXPIRING_EVENT,
    PROACTIVE_RENEW_GRACE_SECS, RENEWAL_LEAD_SECS,
};
use super::window as auth_window;

#[derive(Clone)]
pub struct AuthBridge {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    inflight: HashMap<String, InFlight>,
}

struct InFlight {
    config: BrokerAuthConfig,
    session_nonce: String,
    labels: HashSet<String>,
    seen: HashSet<[u8; 32]>,
    tx: Option<oneshot::Sender<FinishReason>>,
    proactive_renew: bool,
    renew_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
enum AuthMode {
    Manual,
    ProactiveRenew { expires_at: DateTime<Utc> },
}

#[derive(Debug)]
pub enum FinishReason {
    Captured(AuthSessionStatus),
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExpiredPayload {
    broker_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExpiringPayload {
    broker_id: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// Debounce expired/expiring emits per broker + expiry timestamp.
fn event_dedupe() -> &'static Mutex<HashMap<String, String>> {
    static DEDUPE: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    DEDUPE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn should_emit(kind: &str, broker_id: &str, epoch: &str) -> bool {
    let key = format!("{kind}:{broker_id}");
    let mut m = event_dedupe().lock();
    if m.get(&key).map(|s| s.as_str()) == Some(epoch) {
        return false;
    }
    m.insert(key, epoch.to_string());
    true
}

pub fn clear_event_dedupe(broker_id: &str) {
    let mut m = event_dedupe().lock();
    m.remove(&format!("expired:{broker_id}"));
    m.remove(&format!("expiring:{broker_id}"));
}

pub fn proactive_renew_timeout(expires_at: DateTime<Utc>) -> Duration {
    let grace_end = expires_at + chrono::Duration::seconds(PROACTIVE_RENEW_GRACE_SECS);
    let remaining = (grace_end - Utc::now()).num_seconds().max(1) as u64;
    Duration::from_secs(remaining)
}

impl AuthBridge {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                inflight: HashMap::new(),
            })),
        }
    }

    pub fn register_window_label(&self, broker_id: &str, label: String) {
        if let Some(flight) = self.inner.lock().inflight.get_mut(broker_id) {
            flight.labels.insert(label);
        }
    }

    pub fn is_awaiting(&self, broker_id: &str) -> bool {
        self.inner.lock().inflight.contains_key(broker_id)
    }

    pub fn on_window_destroyed<R: Runtime>(&self, app: &AppHandle<R>, broker_id: &str, label: &str) {
        enum Action {
            Respawn {
                config: BrokerAuthConfig,
                session_nonce: String,
                window_label: String,
            },
            Cancel(oneshot::Sender<FinishReason>),
            None,
        }

        let action = {
            let mut inner = self.inner.lock();
            let Some(flight) = inner.inflight.get_mut(broker_id) else {
                return;
            };
            flight.labels.remove(label);
            if !flight.labels.is_empty() {
                return;
            }
            if flight.proactive_renew {
                if let Some(expires_at) = flight.renew_expires_at {
                    let grace_end =
                        expires_at + chrono::Duration::seconds(PROACTIVE_RENEW_GRACE_SECS);
                    if Utc::now() < grace_end {
                        let config = flight.config.clone();
                        let session_nonce = flight.session_nonce.clone();
                        let window_label = format!(
                            "auth-bridge-{}-{}",
                            config.id,
                            &Uuid::new_v4().to_string()[..8]
                        );
                        flight.labels.insert(window_label.clone());
                        Action::Respawn {
                            config,
                            session_nonce,
                            window_label,
                        }
                    } else {
                        inner
                            .inflight
                            .remove(broker_id)
                            .and_then(|mut f| f.tx.take())
                            .map(Action::Cancel)
                            .unwrap_or(Action::None)
                    }
                } else {
                    inner
                        .inflight
                        .remove(broker_id)
                        .and_then(|mut f| f.tx.take())
                        .map(Action::Cancel)
                        .unwrap_or(Action::None)
                }
            } else {
                inner
                    .inflight
                    .remove(broker_id)
                    .and_then(|mut f| f.tx.take())
                    .map(Action::Cancel)
                    .unwrap_or(Action::None)
            }
        };

        match action {
            Action::Respawn {
                config,
                session_nonce,
                window_label,
            } => {
                tracing::info!(
                    target: "auth_bridge",
                    broker = %config.id,
                    "proactive renew window closed — reopening login"
                );
                let title = format!("Renew {} session", config.display_name);
                if let Err(e) = auth_window::spawn_auth_window(
                    app,
                    self,
                    &config,
                    &session_nonce,
                    &window_label,
                    Some(&title),
                    true,
                ) {
                    tracing::warn!(
                        target: "auth_bridge",
                        broker = %config.id,
                        error = %e,
                        "proactive renew respawn failed"
                    );
                    self.cancel_inflight(app, broker_id);
                }
            }
            Action::Cancel(tx) => {
                let _ = tx.send(FinishReason::Cancelled);
            }
            Action::None => {}
        }
    }

    pub async fn authenticate<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        broker_id: &str,
    ) -> Result<AuthSessionStatus, String> {
        self.authenticate_with_mode(app, broker_id, AuthMode::Manual)
            .await
    }

    pub async fn authenticate_proactive_renew<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        broker_id: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<AuthSessionStatus, String> {
        self.authenticate_with_mode(
            app,
            broker_id,
            AuthMode::ProactiveRenew { expires_at },
        )
        .await
    }

    async fn authenticate_with_mode<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        broker_id: &str,
        mode: AuthMode,
    ) -> Result<AuthSessionStatus, String> {
        let config = config::get(broker_id).map_err(|e| e.to_string())?;
        if matches!(mode, AuthMode::Manual) {
            self.cancel_inflight(app, &config.id);
        }

        let (proactive_renew, renew_expires_at, window_title, focus) = match &mode {
            AuthMode::Manual => (false, None, None, false),
            AuthMode::ProactiveRenew { expires_at } => (
                true,
                Some(*expires_at),
                Some(format!("Renew {} session", config.display_name)),
                true,
            ),
        };

        let session_nonce = Uuid::new_v4().to_string();
        let window_label = format!("auth-bridge-{}-{}", config.id, &session_nonce[..8]);
        let (tx, rx) = oneshot::channel();

        {
            let mut inner = self.inner.lock();
            let mut labels = HashSet::new();
            labels.insert(window_label.clone());
            inner.inflight.insert(
                config.id.clone(),
                InFlight {
                    config: config.clone(),
                    session_nonce: session_nonce.clone(),
                    labels,
                    seen: HashSet::new(),
                    tx: Some(tx),
                    proactive_renew,
                    renew_expires_at,
                },
            );
        }

        if let Err(e) = auth_window::spawn_auth_window(
            app,
            self,
            &config,
            &session_nonce,
            &window_label,
            window_title.as_deref(),
            focus,
        ) {
            self.cancel_inflight(app, &config.id);
            return Err(e);
        }

        tracing::info!(
            target: "auth_bridge",
            broker = %config.id,
            proactive = proactive_renew,
            "auth window opened"
        );

        let timeout = match &mode {
            AuthMode::Manual => Duration::from_millis(config.timeout_ms.max(5_000)),
            AuthMode::ProactiveRenew { expires_at } => proactive_renew_timeout(*expires_at),
        };
        let outcome = tokio::time::timeout(timeout, rx).await;
        match outcome {
            Ok(Ok(FinishReason::Captured(status))) => {
                if proactive_renew {
                    tracing::info!(
                        target: "auth_bridge",
                        broker = %config.id,
                        "proactive re-auth succeeded"
                    );
                }
                Ok(status)
            }
            Ok(Ok(FinishReason::Cancelled)) => Err("Login cancelled.".into()),
            Ok(Ok(FinishReason::TimedOut)) => Err("Login timed out.".into()),
            Ok(Err(_)) => Err("Login cancelled.".into()),
            Err(_) => {
                self.finish_timeout(app, &config.id);
                Err("Login timed out.".into())
            }
        }
    }

    pub async fn submit_from_webview<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        webview: &WebviewWindow<R>,
        session_nonce: &str,
        header_name: &str,
        value: &str,
    ) -> Result<(), String> {
        let label = webview.label().to_string();
        let page_url = webview.url().ok();
        self.submit_candidate(app, &label, page_url, session_nonce, header_name, value)
            .await
    }

    pub async fn submit_candidate<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        window_label: &str,
        page_url: Option<Url>,
        session_nonce: &str,
        header_name: &str,
        value: &str,
    ) -> Result<(), String> {
        let (config, nonce) = {
            let inner = self.inner.lock();
            let flight = inner
                .inflight
                .values()
                .find(|f| f.labels.contains(window_label));
            let Some(flight) = flight else {
                return Err("no active auth session".to_string());
            };
            if flight.session_nonce != session_nonce {
                return Err("rejected".into());
            }
            (flight.config.clone(), flight.session_nonce.clone())
        };
        let _ = nonce;

        if let Some(url) = page_url {
            if !origin_allowed(&url, &config.allowed_origins) {
                return Err("rejected".into());
            }
        }

        let candidate = match capture::match_candidate(&config, header_name, value) {
            Ok(c) => c,
            Err(_) => {
                return Err("rejected".into());
            }
        };
        let hash = digest(&candidate.value);
        {
            let mut inner = self.inner.lock();
            let Some(flight) = inner.inflight.get_mut(&config.id) else {
                return Err("rejected".into());
            };
            if !flight.seen.insert(hash) {
                return Ok(());
            }
        }

        tracing::info!(
            target: "auth_bridge",
            broker = %config.id,
            header = %candidate.header_name,
            "candidate accepted for probe"
        );

        let probed = match probe::probe(&config, &candidate).await {
            Ok(p) => p,
            Err(_) => {
                return Err("probe failed".to_string());
            }
        };
        if !probed.ok {
            tracing::info!(
                target: "auth_bridge",
                broker = %config.id,
                status = probed.status,
                "probe rejected candidate"
            );
            return Err("rejected".into());
        }

        let status = session::persist_valid(
            &config,
            &candidate.header_name,
            &candidate.value,
            probed.account_hint,
            probed.profile_id,
        )
        .map_err(|e| e.to_string())?;

        clear_event_dedupe(&config.id);
        self.complete_captured(app, &config.id, status.clone());
        Ok(())
    }

    fn complete_captured<R: Runtime>(&self, app: &AppHandle<R>, broker_id: &str, status: AuthSessionStatus) {
        let (labels, tx) = self.take_inflight(broker_id);
        auth_window::teardown(app, &labels);
        if let Some(tx) = tx {
            let _ = tx.send(FinishReason::Captured(status));
        }
    }

    fn finish_timeout<R: Runtime>(&self, app: &AppHandle<R>, broker_id: &str) {
        let (labels, tx) = self.take_inflight(broker_id);
        auth_window::teardown(app, &labels);
        if let Some(tx) = tx {
            let _ = tx.send(FinishReason::TimedOut);
        }
    }

    fn cancel_inflight<R: Runtime>(&self, app: &AppHandle<R>, broker_id: &str) {
        let (labels, tx) = self.take_inflight(broker_id);
        auth_window::teardown(app, &labels);
        if let Some(tx) = tx {
            let _ = tx.send(FinishReason::Cancelled);
        }
    }

    fn take_inflight(
        &self,
        broker_id: &str,
    ) -> (HashSet<String>, Option<oneshot::Sender<FinishReason>>) {
        let mut inner = self.inner.lock();
        match inner.inflight.remove(broker_id) {
            Some(mut f) => (f.labels, f.tx.take()),
            None => (HashSet::new(), None),
        }
    }
}

impl Default for AuthBridge {
    fn default() -> Self {
        Self::new()
    }
}

pub fn start_expiry_watcher<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            sweep_sessions(&app);
        }
    });
}

/// Soft-expire Busha on 401; hard-revoke other brokers. Emits `auth-bridge:expired`.
pub fn notify_unauthorized<R: Runtime>(app: &AppHandle<R>, broker_id: &str) {
    if broker_id == "busha" {
        let _ = session::mark_expired(broker_id);
    } else {
        let _ = session::revoke(broker_id);
    }
    let epoch = chrono::Utc::now().to_rfc3339();
    if should_emit("expired", broker_id, &epoch) {
        let _ = app.emit(
            EXPIRED_EVENT,
            ExpiredPayload {
                broker_id: broker_id.to_string(),
            },
        );
    }
    tracing::info!(target: "auth_bridge", broker = %broker_id, "session unauthorized");
}

fn spawn_proactive_reauth<R: Runtime>(
    app: AppHandle<R>,
    broker_id: &str,
    expires_at: DateTime<Utc>,
) {
    let bridge = {
        let Some(state) = app.try_state::<AuthBridge>() else {
            return;
        };
        if state.is_awaiting(broker_id) {
            tracing::info!(
                target: "auth_bridge",
                broker = %broker_id,
                "proactive re-auth skipped — auth already in flight"
            );
            return;
        }
        state.inner().clone()
    };
    let broker_id = broker_id.to_string();
    tracing::info!(
        target: "auth_bridge",
        broker = %broker_id,
        expires_at = %expires_at,
        "proactive re-auth starting"
    );
    tauri::async_runtime::spawn(async move {
        match bridge
            .authenticate_proactive_renew(&app, &broker_id, expires_at)
            .await
        {
            Ok(_) => {}
            Err(e) => tracing::warn!(
                target: "auth_bridge",
                broker = %broker_id,
                error = %e,
                "proactive re-auth failed"
            ),
        }
    });
}

fn sweep_sessions<R: Runtime>(app: &AppHandle<R>) {
    let Ok(cfgs) = config::builtin_configs() else {
        return;
    };
    for cfg in cfgs {
        let status = session::status_for(&cfg);
        match status.status {
            SessionStatusKind::Expired => {
                let epoch = status
                    .expires_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "expired".into());
                // Soft-expired Busha: keep credential so crypto mode does not flip to stocks.
                if cfg.id != "busha" {
                    let _ = session::revoke(&cfg.id);
                }
                if should_emit("expired", &cfg.id, &epoch) {
                    let _ = app.emit(
                        EXPIRED_EVENT,
                        ExpiredPayload {
                            broker_id: cfg.id.clone(),
                        },
                    );
                }
            }
            SessionStatusKind::Connected => {
                let Some(expires_at) = status.expires_at else {
                    continue;
                };
                let remaining = (expires_at - chrono::Utc::now()).num_seconds();
                if remaining > RENEWAL_LEAD_SECS {
                    continue;
                }
                let epoch = expires_at.to_rfc3339();
                if should_emit("expiring", &cfg.id, &epoch) {
                    let _ = app.emit(
                        EXPIRING_EVENT,
                        ExpiringPayload {
                            broker_id: cfg.id.clone(),
                            expires_at,
                        },
                    );
                    if cfg.id == "busha" {
                        spawn_proactive_reauth(app.clone(), &cfg.id, expires_at);
                    }
                    tracing::info!(
                        target: "auth_bridge",
                        broker = %cfg.id,
                        expires_at = %expires_at,
                        "session expiring — proactive renew triggered"
                    );
                }
            }
            _ => {}
        }
    }
}

#[allow(dead_code)]
pub fn get_token(broker_id: &str) -> anyhow::Result<Option<String>> {
    session::get_token(broker_id)
}

pub fn list_sessions() -> Result<Vec<AuthSessionStatus>, String> {
    let cfgs = config::builtin_configs().map_err(|e| e.to_string())?;
    Ok(cfgs.iter().map(session::status_for).collect())
}

pub fn session_status(broker_id: &str) -> Result<AuthSessionStatus, String> {
    let cfg = config::get(broker_id).map_err(|e| e.to_string())?;
    Ok(session::status_for(&cfg))
}

#[allow(dead_code)]
pub fn connecting_status(broker_id: &str) -> Result<AuthSessionStatus, String> {
    let cfg = config::get(broker_id).map_err(|e| e.to_string())?;
    Ok(AuthSessionStatus {
        broker_id: cfg.id,
        display_name: cfg.display_name,
        status: SessionStatusKind::Connecting,
        expires_at: None,
        account_hint: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_bridge::capture::Candidate;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn jwt(exp: i64) -> String {
        let header = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"alg":"none"}"#,
        );
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            format!(r#"{{"exp":{exp},"email":"a@b.c"}}"#).as_bytes(),
        );
        format!("Bearer {header}.{payload}.sig")
    }

    fn serve_json(status: u16, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_string();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[test]
    fn proactive_renew_timeout_uses_grace_not_config_timeout() {
        let expires_at = Utc::now() + chrono::Duration::minutes(5);
        let timeout = proactive_renew_timeout(expires_at);
        let expected_secs = (expires_at + chrono::Duration::seconds(PROACTIVE_RENEW_GRACE_SECS)
            - Utc::now())
        .num_seconds()
        .max(1) as u64;
        assert!(timeout.as_secs() >= expected_secs.saturating_sub(2));
        assert!(timeout.as_secs() <= expected_secs + 2);
        assert!(timeout.as_secs() > 300);
    }

    #[test]
    fn proactive_renew_not_awaiting_initially() {
        let bridge = AuthBridge::new();
        assert!(!bridge.is_awaiting("busha"));
    }

    #[tokio::test]
    async fn capture_probe_store_round_trip_spa_bearer() {
        let url = serve_json(200, r#"{"email":"spa@example.com"}"#);
        let mut cfg = config::get("busha").unwrap();
        cfg.id = "mock-spa".into();
        cfg.probe.url = format!("{url}/me");
        let token = jwt((chrono::Utc::now() + chrono::Duration::hours(1)).timestamp());
        let candidate = capture::match_candidate(&cfg, "authorization", &token).unwrap();
        let probed = probe::probe(&cfg, &candidate).await.unwrap();
        assert!(probed.ok);
        session::persist_valid(
            &cfg,
            &candidate.header_name,
            &candidate.value,
            probed.account_hint,
            probed.profile_id,
        )
        .unwrap();
        let stored = get_token("mock-spa").unwrap().unwrap();
        assert_eq!(stored, token);
        session::revoke("mock-spa").unwrap();
        assert!(get_token("mock-spa").unwrap().is_none());
    }

    #[tokio::test]
    async fn form_post_cookie_probe_round_trip() {
        let url = serve_json(200, r#"{"email":"cookie@example.com"}"#);
        let mut cfg = config::get("busha").unwrap();
        cfg.id = "mock-cookie".into();
        cfg.capture.cookie_names = vec!["sessionid".into()];
        cfg.capture.header_names = vec!["sessionid".into()];
        cfg.capture.header_pattern = r"^[\w.-]+$".into();
        cfg.probe.url = format!("{url}/account");
        let candidate = Candidate {
            header_name: "sessionid".into(),
            value: "sess_abc123".into(),
        };
        let probed = probe::probe(&cfg, &candidate).await.unwrap();
        assert!(probed.ok);
        assert_eq!(probed.account_hint.as_deref(), Some("cookie@example.com"));
        session::persist_valid(
            &cfg,
            "sessionid",
            "sess_abc123",
            probed.account_hint,
            probed.profile_id,
        )
        .unwrap();
        assert!(get_token("mock-cookie").unwrap().is_some());
        session::revoke("mock-cookie").unwrap();
    }

    #[test]
    fn unknown_broker_rejected() {
        assert!(config::get("not-a-broker").is_err());
    }
}
