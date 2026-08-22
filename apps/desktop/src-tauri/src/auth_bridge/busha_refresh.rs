use anyhow::{anyhow, Context, Result};

use super::busha_session::{self, BushaRefreshTokens, BushaWebSession};
use super::config;
use super::session::{self, AuthSessionStatus};
use super::store::{self, StoredCredential};

const REFRESH_URL: &str = "https://app.busha.io/api/auth/refresh-token";
const ORIGIN: &str = "https://app.busha.io";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";

pub fn can_silent_refresh_busha() -> bool {
    store::get("busha")
        .ok()
        .flatten()
        .map(|c| {
            c.busha_session_cookie
                .as_ref()
                .is_some_and(|s| !s.is_empty())
                && c.busha_csrf_token
                    .as_ref()
                    .is_some_and(|s| !s.is_empty())
        })
        .unwrap_or(false)
}

pub async fn refresh_busha_session() -> Result<AuthSessionStatus> {
    let cfg = config::get("busha").context("load busha config")?;
    let mut cred = store::get("busha")?
        .ok_or_else(|| anyhow!("Busha is not connected"))?;
    if !can_silent_refresh_busha() {
        return Err(anyhow!("Busha silent refresh credentials missing"));
    }

    let session_cookie = cred
        .busha_session_cookie
        .clone()
        .ok_or_else(|| anyhow!("missing app__session cookie"))?;
    let csrf = cred
        .busha_csrf_token
        .clone()
        .ok_or_else(|| anyhow!("missing csrf token"))?;

    let http = crate::http_client::http_client().context("http client")?;
    let response = http
        .post(REFRESH_URL)
        .header(
            reqwest::header::COOKIE,
            format!("app__session={session_cookie}"),
        )
        .header("x-csrf-token", csrf)
        .header("Origin", ORIGIN)
        .header("Referer", "https://app.busha.io/explore")
        .header("Accept", "*/*")
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("Busha refresh-token POST")?;

    let status = response.status();
    let headers = response.headers().clone();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "Busha refresh-token failed ({status}): {}",
            body.to_string()
        ));
    }

    let parsed = busha_session::parse_refresh_response(&body)?;
    let set_cookie = busha_session::extract_set_cookie_session(&headers);
    apply_refresh_tokens(&mut cred, &parsed, set_cookie.as_deref());
    store::put("busha", &cred).context("store refreshed Busha session")?;

    tracing::info!(
        target: "auth_bridge",
        broker = "busha",
        expires_at = %cred.expires_at,
        "Busha session silently refreshed"
    );
    Ok(session::status_for(&cfg))
}

pub fn apply_web_session(cred: &mut StoredCredential, web: &BushaWebSession) {
    cred.busha_session_cookie = Some(web.session_cookie.clone());
    cred.busha_csrf_token = Some(web.csrf_token.clone());
    cred.busha_refresh_token = web.refresh_token.clone();
}

fn apply_refresh_tokens(
    cred: &mut StoredCredential,
    parsed: &BushaRefreshTokens,
    set_cookie: Option<&str>,
) {
    cred.header_name = "authorization".into();
    cred.kind = "header".into();
    cred.token = if parsed.access_token.to_ascii_lowercase().starts_with("bearer ") {
        parsed.access_token.clone()
    } else {
        format!("Bearer {}", parsed.access_token)
    };
    cred.expires_at = parsed.expires_at;
    cred.busha_csrf_token = Some(parsed.csrf_token.clone());
    cred.busha_refresh_token = parsed.refresh_token.clone();
    if let Some(cookie) = set_cookie.filter(|s| !s.is_empty()) {
        cred.busha_session_cookie = Some(cookie.to_string());
    }
    if let Some(hint) = parsed.account_hint.clone() {
        cred.account_hint = Some(hint);
    }
    if let Some(pid) = parsed.profile_id.clone() {
        cred.profile_id = Some(pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn can_silent_refresh_requires_cookie_and_csrf() {
        let broker = "test-silent-refresh-broker";
        let mut cred = StoredCredential {
            kind: "header".into(),
            header_name: "authorization".into(),
            token: "Bearer x".into(),
            expires_at: Utc::now(),
            account_hint: None,
            profile_id: None,
            busha_session_cookie: Some("cookie".into()),
            busha_csrf_token: Some("csrf".into()),
            busha_refresh_token: Some("rft".into()),
        };
        store::put(broker, &cred).unwrap();
        assert!(store::get(broker).unwrap().unwrap().busha_session_cookie.is_some());
        cred.busha_csrf_token = None;
        store::put(broker, &cred).unwrap();
        assert!(store::get(broker)
            .unwrap()
            .unwrap()
            .busha_csrf_token
            .is_none());
        let _ = store::delete(broker);
    }

    #[test]
    fn apply_refresh_tokens_updates_bearer_and_web_fields() {
        let mut cred = StoredCredential {
            kind: "header".into(),
            header_name: "authorization".into(),
            token: "Bearer old".into(),
            expires_at: Utc::now(),
            account_hint: None,
            profile_id: None,
            busha_session_cookie: Some("old_cookie".into()),
            busha_csrf_token: Some("old_csrf".into()),
            busha_refresh_token: Some("old_rft".into()),
        };
        let body = json!({
            "tokens": {
                "accessToken": "new.jwt.token",
                "refreshToken": "rft_new",
                "expiresAt": "2026-08-22T08:50:40Z"
            },
            "userData": { "email": "a@b.c", "uid": "uid-1", "exp": 1787388640 },
            "csrfToken": "csrf_new"
        });
        let parsed = busha_session::parse_refresh_response(&body).unwrap();
        apply_refresh_tokens(&mut cred, &parsed, Some("new_cookie"));
        assert_eq!(cred.token, "Bearer new.jwt.token");
        assert_eq!(cred.busha_csrf_token.as_deref(), Some("csrf_new"));
        assert_eq!(cred.busha_session_cookie.as_deref(), Some("new_cookie"));
        assert_eq!(cred.profile_id.as_deref(), Some("uid-1"));
    }
}
