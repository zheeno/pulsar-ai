mod capture;
mod config;
mod interceptor;
mod manager;
mod probe;
mod session;
mod store;
mod window;

#[allow(unused_imports)]
pub use manager::{get_token, notify_unauthorized, AuthBridge};
#[allow(unused_imports)]
pub use session::{AuthSessionStatus, SessionStatusKind};

use tauri::{AppHandle, State, WebviewWindow};

#[tauri::command]
pub async fn auth_bridge_authenticate(
    app: AppHandle,
    bridge: State<'_, AuthBridge>,
    broker_id: String,
) -> Result<AuthSessionStatus, String> {
    let id = broker_id_from_arg(&broker_id)?;
    bridge.authenticate(&app, &id).await
}

#[tauri::command]
pub fn auth_bridge_session(broker_id: String) -> Result<AuthSessionStatus, String> {
    let id = broker_id_from_arg(&broker_id)?;
    manager::session_status(&id)
}

#[tauri::command]
pub fn auth_bridge_list() -> Result<Vec<AuthSessionStatus>, String> {
    manager::list_sessions()
}

#[tauri::command]
pub fn auth_bridge_revoke(broker_id: String) -> Result<AuthSessionStatus, String> {
    let id = broker_id_from_arg(&broker_id)?;
    session::revoke(&id).map_err(|e| e.to_string())?;
    manager::session_status(&id)
}

#[tauri::command]
pub async fn auth_bridge_submit_candidate(
    app: AppHandle,
    webview: WebviewWindow,
    bridge: State<'_, AuthBridge>,
    session_nonce: String,
    header_name: String,
    value: String,
) -> Result<(), String> {
    if !webview.label().starts_with("auth-bridge-") {
        return Err("rejected".into());
    }
    bridge
        .submit_from_webview(&app, &webview, &session_nonce, &header_name, &value)
        .await
}

fn broker_id_from_arg(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err("invalid broker id".into());
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
            return Ok(id.trim().to_ascii_lowercase());
        }
    }
    Ok(trimmed.to_ascii_lowercase())
}

pub fn revoke_all() {
    session::revoke_all();
}

pub fn start_expiry_watcher(app: AppHandle) {
    manager::start_expiry_watcher(app);
}
