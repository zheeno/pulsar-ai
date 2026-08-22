use super::config::{header_names_lower, BrokerAuthConfig};

const TEMPLATE: &str = include_str!("interceptor.js");

pub fn render(config: &BrokerAuthConfig, session_nonce: &str) -> String {
    let origins = serde_json::to_string(&effective_origins(config)).unwrap_or_else(|_| "[]".into());
    let headers = serde_json::to_string(&header_names_lower(config)).unwrap_or_else(|_| "[]".into());
    let ws = match &config.capture.websocket_url_token_param {
        Some(p) if !p.is_empty() => serde_json::to_string(p).unwrap_or_else(|_| "null".into()),
        _ => "null".to_string(),
    };
    TEMPLATE
        .replace("__PULSAR_SESSION_NONCE__", session_nonce)
        .replace("__PULSAR_ALLOWED_ORIGINS__", &origins)
        .replace("__PULSAR_HEADER_NAMES__", &headers)
        .replace("__PULSAR_WS_PARAM__", &ws)
}

fn effective_origins(config: &BrokerAuthConfig) -> Vec<String> {
    config
        .allowed_origins
        .iter()
        .filter(|o| !o.contains('*'))
        .filter(|o| !is_identity_provider(o))
        .cloned()
        .collect()
}

fn is_identity_provider(origin: &str) -> bool {
    let o = origin.to_ascii_lowercase();
    o.contains("google.")
        || o.contains("google.com")
        || o.contains("youtube.com")
        || o.contains("apple.com")
        || o.contains("facebook.com")
        || o.contains("microsoftonline")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_closes_over_nonce_not_global() {
        let cfg = crate::auth_bridge::config::get("busha").unwrap();
        let js = render(&cfg, "nonce-abc-123");
        assert!(js.contains("nonce-abc-123"));
        assert!(!js.contains("__PULSAR_"));
        assert!(!js.contains("window.__PULSAR"));
        assert!(js.contains("auth_bridge_submit_candidate"));
        assert!(js.contains("sessionNonce"));
        assert!(js.contains("pulsar_ab"));
    }
}
