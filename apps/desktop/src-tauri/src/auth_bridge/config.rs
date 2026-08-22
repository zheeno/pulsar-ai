use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

include!(concat!(env!("OUT_DIR"), "/auth_bridge_brokers.rs"));

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerAuthConfig {
    pub id: String,
    pub display_name: String,
    pub login_url: String,
    pub allowed_origins: Vec<String>,
    pub capture: CaptureRules,
    pub probe: ProbeConfig,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_fallback_ttl")]
    pub fallback_ttl_secs: u64,
    /// Optional URL for silent renew (already-logged-in app shell). Falls back to `login_url`.
    #[serde(default)]
    pub renew_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRules {
    pub header_names: Vec<String>,
    pub header_pattern: String,
    #[serde(default)]
    pub cookie_names: Vec<String>,
    #[serde(default)]
    pub websocket_url_token_param: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeConfig {
    #[serde(default = "default_method")]
    pub method: String,
    pub url: String,
    #[serde(default = "default_expect_status")]
    pub expect_status: Vec<u16>,
    #[serde(default)]
    pub json_path_hint: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

fn default_timeout_ms() -> u64 {
    300_000
}

fn default_fallback_ttl() -> u64 {
    86_400
}

fn default_method() -> String {
    "GET".into()
}

fn default_expect_status() -> Vec<u16> {
    vec![200]
}

pub fn builtin_configs() -> Result<Vec<BrokerAuthConfig>> {
    let mut out = Vec::new();
    for raw in embedded_broker_json() {
        out.push(serde_json::from_str(raw).context("parse auth-bridge broker config")?);
    }
    Ok(out)
}

pub fn get(id: &str) -> Result<BrokerAuthConfig> {
    let id = id.trim().to_ascii_lowercase();
    builtin_configs()?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| anyhow!("unknown auth-bridge broker: {id}"))
}

pub fn header_names_lower(config: &BrokerAuthConfig) -> Vec<String> {
    config
        .capture
        .header_names
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .collect()
}

pub fn origin_allowed(url: &tauri::Url, allowed: &[String]) -> bool {
    match url.scheme() {
        "about" | "ipc" | "tauri" => true,
        "http" | "https" => {
            let origin = url.origin().ascii_serialization();
            allowed.iter().any(|p| origin_pattern_matches(&origin, p))
        }
        _ => false,
    }
}

pub fn origin_pattern_matches(origin: &str, pattern: &str) -> bool {
    let pattern = pattern.trim().trim_end_matches('/');
    if origin.eq_ignore_ascii_case(pattern) {
        return true;
    }
    if let Some(idx) = pattern.find("://*.") {
        let scheme = &pattern[..idx];
        let rest = &pattern[idx + 5..];
        let Some((_, host)) = origin.split_once("://") else {
            return false;
        };
        origin.len() >= scheme.len()
            && origin[..scheme.len()].eq_ignore_ascii_case(scheme)
            && (host.eq_ignore_ascii_case(rest)
                || host
                    .to_ascii_lowercase()
                    .ends_with(&format!(".{}", rest.to_ascii_lowercase())))
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busha_config_loads() {
        let cfg = get("busha").unwrap();
        assert_eq!(cfg.login_url, "https://app.busha.io/login");
        assert!(cfg.capture.header_names.iter().any(|h| h == "authorization"));
    }

    #[test]
    fn wildcard_origin() {
        assert!(origin_pattern_matches(
            "https://api.busha.co",
            "https://*.busha.co"
        ));
        assert!(origin_pattern_matches(
            "https://busha.co",
            "https://*.busha.co"
        ));
        assert!(!origin_pattern_matches(
            "https://evil.com",
            "https://*.busha.co"
        ));
        assert!(!origin_pattern_matches("file://host", "https://app.busha.io"));
    }
}
