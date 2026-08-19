use anyhow::{anyhow, Result};
use regex::Regex;
use sha2::{Digest, Sha256};

use super::config::BrokerAuthConfig;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub header_name: String,
    pub value: String,
}

pub fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

pub fn match_candidate(config: &BrokerAuthConfig, header_name: &str, value: &str) -> Result<Candidate> {
    let name = header_name.trim().to_ascii_lowercase();
    let value = value.trim();
    if value.is_empty() || value.len() > 16_384 {
        return Err(anyhow!("empty credential"));
    }

    let headers = super::config::header_names_lower(config);
    let is_ws = name == "websocket" && config.capture.websocket_url_token_param.is_some();
    let is_cookie = config
        .capture
        .cookie_names
        .iter()
        .any(|c| c.eq_ignore_ascii_case(&name));
    let is_header = headers.iter().any(|h| h == &name);

    if !is_header && !is_cookie && !is_ws {
        return Err(anyhow!("header not configured"));
    }

    if is_header || is_ws {
        let re = Regex::new(&config.capture.header_pattern).map_err(|_| anyhow!("bad header pattern"))?;
        if !re.is_match(value) {
            return Err(anyhow!("value failed pattern"));
        }
    }

    Ok(Candidate {
        header_name: name,
        value: value.to_string(),
    })
}

pub fn bearer_token(value: &str) -> &str {
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .unwrap_or(value)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_bridge::config::get;

    #[test]
    fn accepts_bearer_and_rejects_junk() {
        let cfg = get("busha").unwrap();
        let ok = match_candidate(&cfg, "Authorization", "Bearer abc.def.ghi").unwrap();
        assert_eq!(ok.header_name, "authorization");
        assert!(match_candidate(&cfg, "Authorization", "Basic abc").is_err());
        assert!(match_candidate(&cfg, "Cookie", "Bearer abc.def.ghi").is_err());
    }

    #[test]
    fn digest_is_stable() {
        assert_eq!(digest("a"), digest("a"));
        assert_ne!(digest("a"), digest("b"));
    }
}
