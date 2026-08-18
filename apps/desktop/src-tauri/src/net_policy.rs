use std::net::{IpAddr, ToSocketAddrs};

use anyhow::{anyhow, Result};
use reqwest::Url;

const DEFAULT_LLM_HOSTS: &[&str] = &[
    "api.openai.com",
    "api.anthropic.com",
    "openrouter.ai",
    "api.openrouter.ai",
];

pub fn validate_llm_base_url(raw: &str, allow_custom_host: bool) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("LLM base URL is empty"));
    }
    let url = Url::parse(trimmed).map_err(|_| anyhow!("LLM base URL is not a valid URL"))?;
    if url.scheme() != "https" {
        return Err(anyhow!("LLM base URL must use HTTPS"));
    }
    if url.username() != "" || url.password().is_some() {
        return Err(anyhow!("LLM base URL must not contain credentials"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("LLM base URL is missing a host"))?
        .to_ascii_lowercase();
    if host.parse::<IpAddr>().is_ok() {
        return Err(anyhow!("LLM base URL must not be a raw IP address"));
    }
    if !allow_custom_host && !is_default_provider_host(&host) {
        return Err(anyhow!(
            "LLM host '{host}' is not on the default provider allowlist; confirm a custom endpoint and re-enter the API key"
        ));
    }
    reject_private_resolved(&host, url.port().unwrap_or(443))?;
    Ok(url.to_string())
}

pub fn is_default_provider_host(host: &str) -> bool {
    let h = host.trim_start_matches("www.").to_ascii_lowercase();
    DEFAULT_LLM_HOSTS.iter().any(|allowed| h == *allowed || h.ends_with(&format!(".{allowed}")))
}

fn reject_private_resolved(host: &str, port: u16) -> Result<()> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|_| anyhow!("Could not resolve LLM host '{host}'"))?;
    for addr in addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(anyhow!(
                "LLM host '{host}' resolves to a private or reserved address"
            ));
        }
    }
    Ok(())
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_http_and_credentials() {
        assert!(validate_llm_base_url("http://api.openai.com/v1", true).is_err());
        assert!(validate_llm_base_url("https://user:pass@api.openai.com/v1", true).is_err());
        assert!(validate_llm_base_url("https://127.0.0.1/v1", true).is_err());
    }

    #[test]
    fn allowlist_hosts() {
        assert!(is_default_provider_host("api.openai.com"));
        assert!(is_default_provider_host("openrouter.ai"));
        assert!(!is_default_provider_host("evil.example"));
    }
}
