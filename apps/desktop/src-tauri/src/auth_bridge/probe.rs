use anyhow::{anyhow, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use super::capture::{bearer_token, Candidate};
use super::config::BrokerAuthConfig;

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub ok: bool,
    pub status: u16,
    pub account_hint: Option<String>,
}

pub async fn probe(config: &BrokerAuthConfig, candidate: &Candidate) -> Result<ProbeResult> {
    let client = crate::http_client::http_client()?;
    let method = config.probe.method.trim().to_ascii_uppercase();
    let mut headers = HeaderMap::new();
    apply_credential_headers(&mut headers, candidate)?;
    apply_probe_headers(&mut headers, &config.probe.headers)?;

    let request = match method.as_str() {
        "GET" => client.get(&config.probe.url),
        "POST" => client.post(&config.probe.url),
        other => return Err(anyhow!("unsupported probe method {other}")),
    };

    let response = request.headers(headers).send().await?;
    let status = response.status().as_u16();
    let expected = &config.probe.expect_status;
    let ok = if expected.is_empty() {
        (200..300).contains(&status)
    } else {
        expected.contains(&status)
    };

    let account_hint = if ok {
        let body = response.text().await.unwrap_or_default();
        extract_hint(&body, config.probe.json_path_hint.as_deref())
    } else {
        None
    };

    Ok(ProbeResult {
        ok,
        status,
        account_hint,
    })
}

fn apply_credential_headers(headers: &mut HeaderMap, candidate: &Candidate) -> Result<()> {
    if candidate.header_name == "authorization" || candidate.header_name == "websocket" {
        let value = if candidate.value.to_ascii_lowercase().starts_with("bearer ") {
            candidate.value.clone()
        } else {
            format!("Bearer {}", bearer_token(&candidate.value))
        };
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&value).map_err(|_| anyhow!("invalid authorization header"))?,
        );
        return Ok(());
    }

    if candidate.header_name.contains('\n')
        || candidate.header_name.contains('\r')
        || candidate.value.contains('\n')
        || candidate.value.contains('\r')
    {
        return Err(anyhow!("invalid credential characters"));
    }

    let cookie = if candidate.value.contains('=') {
        candidate.value.clone()
    } else {
        format!("{}={}", candidate.header_name, candidate.value)
    };
    headers.insert(
        reqwest::header::COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| anyhow!("invalid cookie"))?,
    );
    Ok(())
}

fn apply_probe_headers(
    headers: &mut HeaderMap,
    extra: &std::collections::HashMap<String, String>,
) -> Result<()> {
    for (name, value) in extra {
        if name.eq_ignore_ascii_case("authorization") || name.eq_ignore_ascii_case("cookie") {
            continue;
        }
        let header_name =
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| anyhow!("invalid probe header"))?;
        headers.insert(
            header_name,
            HeaderValue::from_str(value).map_err(|_| anyhow!("invalid probe header value"))?,
        );
    }
    Ok(())
}

fn extract_hint(body: &str, path: Option<&str>) -> Option<String> {
    let json: Value = serde_json::from_str(body).ok()?;
    let mut candidates = Vec::new();
    if let Some(p) = path {
        candidates.push(p);
    }
    candidates.extend([
        "email",
        "data.email",
        "user.email",
        "profile.email",
        "data.0.email",
        "data.profile.email",
    ]);
    for p in candidates {
        if let Some(v) = walk(&json, p) {
            if let Some(s) = as_hint(v) {
                return Some(s);
            }
        }
    }
    find_email(&json)
}

fn as_hint(value: &Value) -> Option<String> {
    let s = value.as_str()?.trim();
    if !s.is_empty() && s.len() < 320 {
        Some(s.to_string())
    } else {
        None
    }
}

fn find_email(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if s.contains('@') && s.len() < 320 => Some(s.trim().to_string()),
        Value::Object(map) => {
            if let Some(v) = map.get("email").and_then(as_hint) {
                return Some(v);
            }
            for v in map.values() {
                if let Some(found) = find_email(v) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(find_email),
        _ => None,
    }
}

fn walk<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = value;
    for part in path.split('.') {
        cur = if let Ok(idx) = part.parse::<usize>() {
            cur.get(idx)?
        } else {
            cur.get(part)?
        };
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn spawn_ok_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let body = br#"{"email":"trader@example.com"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[tokio::test]
    async fn probe_reads_email_without_logging_token() {
        let base = spawn_ok_server();
        let mut cfg = crate::auth_bridge::config::get("busha").unwrap();
        cfg.probe.url = format!("{base}/me");
        cfg.probe.expect_status = vec![200];
        let candidate = Candidate {
            header_name: "authorization".into(),
            value: "Bearer aaa.bbb.ccc".into(),
        };
        let result = probe(&cfg, &candidate).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.account_hint.as_deref(), Some("trader@example.com"));
    }

    #[test]
    fn walk_nested() {
        let v: Value = serde_json::json!({"data": {"email": "a@b.c"}});
        assert_eq!(walk(&v, "data.email").and_then(|x| x.as_str()), Some("a@b.c"));
        let arr: Value = serde_json::json!({"data": [{"email": "arr@b.c"}]});
        assert_eq!(extract_hint(&arr.to_string(), Some("data.0.email")).as_deref(), Some("arr@b.c"));
    }

    /// Set BUSHA_LIVE_TEST=1 and BUSHA_LIVE_TOKEN (Bearer or raw JWT). Never prints the token.
    #[tokio::test]
    #[ignore]
    async fn busha_live_account_probe() {
        if std::env::var("BUSHA_LIVE_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let token = std::env::var("BUSHA_LIVE_TOKEN").expect("BUSHA_LIVE_TOKEN");
        let cfg = crate::auth_bridge::config::get("busha").unwrap();
        let value = if token.to_ascii_lowercase().starts_with("bearer ") {
            token
        } else {
            format!("Bearer {token}")
        };
        let candidate = Candidate {
            header_name: "authorization".into(),
            value,
        };
        let result = probe(&cfg, &candidate).await.expect("probe");
        assert!(
            result.ok,
            "Busha probe HTTP {} — check probe URL in brokers/busha.json",
            result.status
        );
        assert!(
            result.account_hint.is_some(),
            "logged-in profile JSON did not yield an account hint"
        );
    }
}
