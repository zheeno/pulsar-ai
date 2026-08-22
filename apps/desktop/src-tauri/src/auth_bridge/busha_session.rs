use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

const APP_SESSION_COOKIE: &str = "app__session";

#[derive(Debug, Clone)]
pub struct BushaWebSession {
    pub session_cookie: String,
    pub csrf_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BushaRefreshTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub csrf_token: String,
    pub account_hint: Option<String>,
    pub profile_id: Option<String>,
}

pub fn is_app_session_cookie(name: &str) -> bool {
    name.eq_ignore_ascii_case(APP_SESSION_COOKIE)
}

pub fn parse_app_session_cookie(raw: &str) -> Result<BushaWebSession> {
    let session_cookie = raw.trim().to_string();
    if session_cookie.is_empty() {
        return Err(anyhow!("empty app__session cookie"));
    }
    let json = decode_session_payload(&session_cookie)?;
    let parsed = parse_session_json(&json)?;
    Ok(BushaWebSession {
        session_cookie,
        csrf_token: parsed.csrf_token,
        refresh_token: parsed.refresh_token,
    })
}

pub fn parse_refresh_response(body: &Value) -> Result<BushaRefreshTokens> {
    let parsed = parse_session_json(body)?;
    let access_token = body
        .pointer("/tokens/accessToken")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("refresh response missing tokens.accessToken"))?;
    Ok(BushaRefreshTokens {
        access_token: access_token.to_string(),
        refresh_token: parsed.refresh_token,
        expires_at: parsed.expires_at,
        csrf_token: parsed.csrf_token,
        account_hint: parsed.account_hint,
        profile_id: parsed.profile_id,
    })
}

pub fn extract_set_cookie_session(headers: &reqwest::header::HeaderMap) -> Option<String> {
    for value in headers.get_all(reqwest::header::SET_COOKIE) {
        let Ok(raw) = value.to_str() else {
            continue;
        };
        for part in raw.split(';') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix("app__session=") {
                let cookie = rest.trim();
                if !cookie.is_empty() {
                    return Some(cookie.to_string());
                }
            }
        }
    }
    None
}

#[derive(Debug)]
struct ParsedSessionJson {
    csrf_token: String,
    refresh_token: Option<String>,
    expires_at: DateTime<Utc>,
    account_hint: Option<String>,
    profile_id: Option<String>,
}

fn parse_session_json(v: &Value) -> Result<ParsedSessionJson> {
    let csrf_token = v
        .get("csrfToken")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("session missing csrfToken"))?
        .to_string();
    let refresh_token = v
        .pointer("/tokens/refreshToken")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let expires_at = parse_expires_at(v)?;
    let account_hint = v
        .pointer("/userData/email")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let profile_id = v
        .pointer("/userData/uid")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(ParsedSessionJson {
        csrf_token,
        refresh_token,
        expires_at,
        account_hint,
        profile_id,
    })
}

fn parse_expires_at(v: &Value) -> Result<DateTime<Utc>> {
    if let Some(iso) = v
        .pointer("/tokens/expiresAt")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
            return Ok(dt.with_timezone(&Utc));
        }
    }
    if let Some(exp) = v.pointer("/userData/exp").and_then(|x| x.as_i64()) {
        return Utc
            .timestamp_opt(exp, 0)
            .single()
            .ok_or_else(|| anyhow!("invalid userData.exp"));
    }
    if let Some(access) = v
        .pointer("/tokens/accessToken")
        .and_then(|x| x.as_str())
    {
        if let Some(exp) = crate::secrets::jwt_expiry(access) {
            return Ok(exp);
        }
    }
    Err(anyhow!("session missing expiry"))
}

fn decode_session_payload(raw: &str) -> Result<Value> {
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .context("decode app__session")?;
    let payload_b64 = decoded
        .rsplit_once('.')
        .map(|(payload, _sig)| payload)
        .unwrap_or(decoded.as_ref());
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        payload_b64,
    )
    .or_else(|_| {
        base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            payload_b64,
        )
    })
    .context("base64 decode app__session payload")?;
    serde_json::from_slice(&bytes).context("parse app__session JSON payload")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_refresh_response_shape() {
        let body = json!({
            "tokens": {
                "status": "success",
                "expiresAt": "2026-08-22T08:50:40Z",
                "accessToken": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjE3ODczODg2NDB9.sig",
                "refreshToken": "rft_test"
            },
            "userData": {
                "exp": 1787388640,
                "email": "user@example.com",
                "uid": "uid-1"
            },
            "csrfToken": "csrf_test"
        });
        let parsed = parse_refresh_response(&body).unwrap();
        assert_eq!(parsed.csrf_token, "csrf_test");
        assert_eq!(parsed.refresh_token.as_deref(), Some("rft_test"));
        assert_eq!(parsed.account_hint.as_deref(), Some("user@example.com"));
        assert_eq!(parsed.profile_id.as_deref(), Some("uid-1"));
    }

    #[test]
    fn parses_signed_app_session_cookie_payload() {
        let inner = json!({
            "tokens": {
                "refreshToken": "rft_cookie",
                "expiresAt": "2026-08-22T08:50:40Z",
                "accessToken": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjE3ODczODg2NDB9.sig"
            },
            "csrfToken": "csrf_cookie",
            "userData": { "exp": 1787388640, "email": "a@b.c", "uid": "u1" }
        });
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            serde_json::to_vec(&inner).unwrap(),
        );
        let cookie = format!("{payload}.sig");
        let parsed = parse_app_session_cookie(&cookie).unwrap();
        assert_eq!(parsed.csrf_token, "csrf_cookie");
        assert_eq!(parsed.refresh_token.as_deref(), Some("rft_cookie"));
        assert_eq!(parsed.session_cookie, cookie);
    }
}
