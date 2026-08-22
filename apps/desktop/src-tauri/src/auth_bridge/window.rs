use std::collections::HashSet;
use std::time::Duration;

use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, Runtime, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use uuid::Uuid;

use super::busha_session::{self, BushaWebSession};
use super::config::{origin_allowed, BrokerAuthConfig};
use super::interceptor;
use super::manager::AuthBridge;

const SIDECHANNEL_CAPTURE: &str = "pulsar_ab";

pub fn spawn_auth_window<R: Runtime>(
    app: &AppHandle<R>,
    bridge: &AuthBridge,
    config: &BrokerAuthConfig,
    session_nonce: &str,
    window_label: &str,
    window_title: Option<&str>,
    focus: bool,
) -> Result<WebviewWindow<R>, String> {
    let login: Url = config
        .login_url
        .parse()
        .map_err(|_| "invalid login URL".to_string())?;
    if !origin_allowed(&login, &config.allowed_origins) {
        return Err("login URL is not in the broker allowlist".into());
    }

    let script = interceptor::render(config, session_nonce);
    let allowed = config.allowed_origins.clone();
    let title = window_title
        .map(str::to_string)
        .unwrap_or_else(|| format!("Connect {}", config.display_name));
    // Incognito + clear on teardown so reconnect always shows a real login form
    // and the interceptor can capture a fresh Bearer (not a stale cookie session).

    let mut builder = WebviewWindowBuilder::new(app, window_label, WebviewUrl::External(login))
        .title(&title)
        .inner_size(480.0, 740.0)
        .resizable(true)
        .incognito(true)
        .devtools(false)
        .initialization_script(&script)
        .on_navigation(move |url| origin_allowed(&url, &allowed));

    let script_reload = script.clone();
    builder = builder.on_page_load(move |webview, payload| {
        if payload.event() == PageLoadEvent::Finished {
            let _ = webview.eval(&script_reload);
        }
    });

    let app_for_child = app.clone();
    let bridge_child = bridge.clone();
    let config_child = config.clone();
    let nonce_child = session_nonce.to_string();
    let parent_label = window_label.to_string();
    builder = builder.on_new_window(move |url, features| {
        if !origin_allowed(&url, &config_child.allowed_origins) {
            tracing::info!(
                target: "auth_bridge",
                broker = %config_child.id,
                "blocked popup origin"
            );
            return NewWindowResponse::Deny;
        }
        let child_label = format!("{parent_label}-w{}", &Uuid::new_v4().to_string()[..8]);
        let script = interceptor::render(&config_child, &nonce_child);
        let allowed = config_child.allowed_origins.clone();
        let child = WebviewWindowBuilder::new(
            &app_for_child,
            &child_label,
            WebviewUrl::External(url),
        )
        .title(&config_child.display_name)
        .incognito(true)
        .devtools(false)
        .initialization_script(&script)
        .on_navigation(move |u| origin_allowed(&u, &allowed))
        .window_features(features);

        match child.build() {
            Ok(window) => {
                bridge_child.register_window_label(&config_child.id, window.label().to_string());
                attach_close_handler(
                    &app_for_child,
                    &window,
                    bridge_child.clone(),
                    config_child.id.clone(),
                );
                NewWindowResponse::Create { window }
            }
            Err(e) => {
                tracing::warn!(target: "auth_bridge", error = %e, "oauth popup failed");
                NewWindowResponse::Deny
            }
        }
    });

    let window = builder.build().map_err(|e| e.to_string())?;
    attach_close_handler(app, &window, bridge.clone(), config.id.clone());
    start_cookie_poller(
        app.clone(),
        bridge.clone(),
        config.clone(),
        window_label.to_string(),
        session_nonce.to_string(),
    );
    if focus {
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(window)
}

fn attach_close_handler<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
    bridge: AuthBridge,
    broker_id: String,
) {
    let label = window.label().to_string();
    let app = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            bridge.on_window_destroyed(&app, &broker_id, &label);
        }
    });
}

fn start_cookie_poller<R: Runtime>(
    app: AppHandle<R>,
    bridge: AuthBridge,
    config: BrokerAuthConfig,
    label: String,
    session_nonce: String,
) {
    let mut cookie_names = config.capture.cookie_names.clone();
    cookie_names.push(SIDECHANNEL_CAPTURE.into());
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            if !bridge.is_awaiting(&config.id) {
                break;
            }
            let Some(window) = app.get_webview_window(&label) else {
                break;
            };
            let names = cookie_names.clone();
            let found = tokio::task::spawn_blocking(move || read_named_cookies(&window, &names))
                .await
                .ok()
                .unwrap_or_default();
            for (name, value) in found {
                if name.eq_ignore_ascii_case(SIDECHANNEL_CAPTURE) {
                    if let Some((header, nonce, token)) = parse_sidechannel(&value) {
                        let _ = bridge
                            .submit_candidate(&app, &label, None, &nonce, &header, &token)
                            .await;
                    }
                    continue;
                }
                let _ = bridge
                    .submit_candidate(&app, &label, None, &session_nonce, &name, &value)
                    .await;
            }
        }
    });
}

fn parse_sidechannel(raw: &str) -> Option<(String, String, String)> {
    let decoded = percent_decode(raw);
    let mut parts = decoded.splitn(3, '\u{1e}');
    let header = parts.next()?.to_string();
    let nonce = parts.next()?.to_string();
    let token = parts.next()?.to_string();
    if header.is_empty() || nonce.is_empty() || token.is_empty() {
        return None;
    }
    Some((header, nonce, token))
}

fn percent_decode(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned()
}

fn read_named_cookies<R: Runtime>(
    window: &WebviewWindow<R>,
    names: &[String],
) -> Vec<(String, String)> {
    let Ok(cookies) = window.cookies() else {
        return Vec::new();
    };
    let pairs: Vec<(String, String)> = cookies
        .iter()
        .map(|c| (c.name().to_string(), c.value().to_string()))
        .collect();
    all_matching_cookies(&pairs, names)
}

pub(crate) fn all_matching_cookies(
    cookies: &[(String, String)],
    names: &[String],
) -> Vec<(String, String)> {
    cookies
        .iter()
        .filter(|(name, value)| {
            !value.is_empty() && names.iter().any(|n| n.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect()
}

pub(crate) fn first_matching_cookie(
    cookies: &[(String, String)],
    names: &[String],
) -> Option<(String, String)> {
    all_matching_cookies(cookies, names).into_iter().next()
}

/// Last-chance read before auth window teardown — avoids losing `app__session` when
/// the Bearer interceptor wins the race against the cookie poller.
pub fn read_busha_web_session<R: Runtime>(
    app: &AppHandle<R>,
    window_label: &str,
) -> Option<BushaWebSession> {
    let window = app.get_webview_window(window_label)?;
    let names = vec!["app__session".into()];
    for (_, value) in read_named_cookies(&window, &names) {
        match busha_session::parse_app_session_cookie(&value) {
            Ok(web) => return Some(web),
            Err(e) => {
                tracing::warn!(
                    target: "auth_bridge",
                    broker = "busha",
                    error = %e,
                    "failed to parse app__session at persist time"
                );
            }
        }
    }
    None
}

pub fn teardown<R: Runtime>(app: &AppHandle<R>, labels: &HashSet<String>) {
    for label in labels {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.clear_all_browsing_data();
            let _ = window.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{first_matching_cookie, parse_sidechannel};

    #[test]
    fn parses_percent_encoded_sidechannel() {
        let raw = percent_encoding::utf8_percent_encode(
            "authorization\u{1e}nonce-1\u{1e}Bearer abc.def",
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();
        let (header, nonce, token) = parse_sidechannel(&raw).unwrap();
        assert_eq!(header, "authorization");
        assert_eq!(nonce, "nonce-1");
        assert_eq!(token, "Bearer abc.def");
    }

    #[test]
    fn cookie_poll_selects_named_httponly_equivalent() {
        let cookies = vec![
            ("other".into(), "x".into()),
            ("sessionid".into(), "sess_abc123".into()),
        ];
        let hit = first_matching_cookie(&cookies, &["sessionid".into()]).unwrap();
        assert_eq!(hit.0, "sessionid");
        assert_eq!(hit.1, "sess_abc123");
        assert!(first_matching_cookie(&cookies, &["missing".into()]).is_none());
    }
}
