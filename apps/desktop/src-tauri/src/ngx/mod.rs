use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::http_client::http_client;
use crate::settings::AppSettings;

const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36";
const TOKEN_REFRESH_BUFFER_SEC: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NgxStock {
    pub symbol: String,
    pub name: Option<String>,
    pub price: f64,
    pub change_percent: Option<f64>,
    pub volume: Option<i64>,
    pub market_cap: Option<f64>,
    pub pe_ratio: Option<f64>,
    pub sector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NgxMarketOverview {
    pub asi: Option<AsiData>,
    pub market_cap: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsiData {
    pub value: f64,
    pub change_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NgxIndex {
    pub code: String,
    pub value: f64,
    pub points: Option<f64>,
    pub week_change: Option<f64>,
    pub month_change: Option<f64>,
    pub year_change: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NgxMarketStatus {
    pub status: String,
    pub is_open: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricePoint {
    pub date: String,
    pub price: f64,
    pub volume: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct LatestQuote {
    pub symbol: String,
    pub last: f64,
    pub prev_close: Option<f64>,
    pub trade_date: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Session,
    ApiKey,
    Mock,
}

impl AuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMode::Session => "session",
            AuthMode::ApiKey => "api_key",
            AuthMode::Mock => "mock",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseAuthReport {
    pub ok: bool,
    pub auth_mode: String,
    pub supabase_host: Option<String>,
    pub pulse_base_url: String,
    pub has_email: bool,
    pub email: Option<String>,
    pub has_password: bool,
    pub has_anon_key: bool,
    pub login_url: Option<String>,
    pub http_status: Option<u16>,
    pub token_expires_at: Option<i64>,
    pub message: String,
    pub logs: Vec<String>,
}

/// Safe NGX Pulse account summary for Settings (no secrets).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PulseProfile {
    pub ok: bool,
    pub auth_mode: String,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub display_name: Option<String>,
    pub phone: Option<String>,
    pub created_at: Option<String>,
    pub last_sign_in_at: Option<String>,
    pub email_confirmed: Option<bool>,
    pub message: String,
}

#[derive(Debug, Clone)]
struct SessionTokens {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

fn correlation_id() -> String {
    uuid::Uuid::new_v4().to_string().chars().take(8).collect()
}

fn redact_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) if !local.is_empty() => {
            format!("{}***@{}", local.chars().next().unwrap_or('*'), domain)
        }
        _ => "[redacted]".into(),
    }
}

fn sanitize_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() && *c != '\t')
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn is_valid_ticker(symbol: &str) -> bool {
    let s = symbol.trim();
    let re_ok = s.len() <= 16
        && !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.' || c == '-');
    re_ok
}

pub struct NgxPulseClient {
    http: Client,
    base_url: String,
    auth_mode: AuthMode,
    api_key: Option<String>,
    supabase_url: Option<String>,
    anon_key: Option<String>,
    email: Option<String>,
    password: Option<String>,
    tokens: Arc<Mutex<Option<SessionTokens>>>,
}

impl NgxPulseClient {
    pub fn from_settings(settings: &AppSettings, password: Option<String>, api_key: Option<String>) -> Self {
        // Prefer process/.env config; fall back to legacy SQLite settings for older installs.
        let supabase_url = std::env::var("NGX_PULSE_SUPABASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| settings.pulse_supabase_url.clone());
        let anon_key = std::env::var("NGX_PULSE_SUPABASE_ANON_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| settings.pulse_supabase_anon_key.clone());
        let base_url = std::env::var("NGX_PULSE_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| settings.pulse_base_url.clone());

        let auth_mode = if supabase_url.is_some()
            && anon_key.is_some()
            && settings.pulse_email.is_some()
            && password.as_ref().is_some_and(|p| !p.is_empty())
        {
            AuthMode::Session
        } else if api_key.as_ref().is_some_and(|k| !k.is_empty()) {
            AuthMode::ApiKey
        } else {
            AuthMode::Mock
        };

        Self {
            http: http_client().expect("http client"),
            base_url,
            auth_mode,
            api_key,
            supabase_url,
            anon_key,
            email: settings.pulse_email.clone(),
            password,
            tokens: Arc::new(Mutex::new(None)),
        }
    }

    pub fn auth_mode(&self) -> AuthMode {
        self.auth_mode
    }

    /// Attempt session/API-key auth and return a diagnostic report (safe for UI + logs).
    pub async fn diagnose_login(&self) -> PulseAuthReport {
        let mut logs = Vec::new();
        let supabase_host = self
            .supabase_url
            .as_ref()
            .and_then(|u| reqwest::Url::parse(u).ok())
            .map(|u| u.host_str().unwrap_or("").to_string())
            .filter(|s| !s.is_empty());
        let login_url = self.supabase_url.as_ref().map(|u| {
            format!("{}/auth/v1/token?grant_type=password", u.trim_end_matches('/'))
        });

        logs.push(format!(
            "auth_mode={} base_url={} supabase_host={:?} email={:?} has_password={} has_anon_key={}",
            self.auth_mode.as_str(),
            self.base_url,
            supabase_host,
            self.email.as_deref().map(redact_email),
            self.password.as_ref().is_some_and(|p| !p.is_empty()),
            self.anon_key.as_ref().is_some_and(|k| !k.is_empty()),
        ));

        match self.auth_mode {
            AuthMode::Mock => {
                let message = String::from(
                    "Auth mode is mock — missing session ingredients (env Supabase URL/anon key, Pulse email, and/or keychain password). No network call was made.",
                );
                logs.push(message.clone());
                tracing::warn!(target: "ngx_pulse", "{}", message);
                return PulseAuthReport {
                    ok: false,
                    auth_mode: self.auth_mode.as_str().into(),
                    supabase_host,
                    pulse_base_url: self.base_url.clone(),
                    has_email: self.email.is_some(),
                    email: self.email.clone(),
                    has_password: self.password.as_ref().is_some_and(|p| !p.is_empty()),
                    has_anon_key: self.anon_key.as_ref().is_some_and(|k| !k.is_empty()),
                    login_url,
                    http_status: None,
                    token_expires_at: None,
                    message,
                    logs,
                };
            }
            AuthMode::ApiKey => {
                logs.push("Using API key auth (not Supabase password grant)".into());
                return match self.get_access_token().await {
                    Ok(_token) => {
                        logs.push("API key present".into());
                        PulseAuthReport {
                            ok: true,
                            auth_mode: self.auth_mode.as_str().into(),
                            supabase_host,
                            pulse_base_url: self.base_url.clone(),
                            has_email: self.email.is_some(),
                            email: self.email.clone(),
                            has_password: false,
                            has_anon_key: self.anon_key.is_some(),
                            login_url: None,
                            http_status: None,
                            token_expires_at: None,
                            message: "API key auth configured".into(),
                            logs,
                        }
                    }
                    Err(e) => PulseAuthReport {
                        ok: false,
                        auth_mode: self.auth_mode.as_str().into(),
                        supabase_host,
                        pulse_base_url: self.base_url.clone(),
                        has_email: self.email.is_some(),
                        email: self.email.clone(),
                        has_password: false,
                        has_anon_key: self.anon_key.is_some(),
                        login_url: None,
                        http_status: None,
                        token_expires_at: None,
                        message: e.to_string(),
                        logs,
                    },
                };
            }
            AuthMode::Session => {}
        }

        let Some(url) = login_url.clone() else {
            let message = String::from("Missing Supabase URL");
            logs.push(message.clone());
            return PulseAuthReport {
                ok: false,
                auth_mode: self.auth_mode.as_str().into(),
                supabase_host,
                pulse_base_url: self.base_url.clone(),
                has_email: self.email.is_some(),
                email: self.email.clone(),
                has_password: self.password.as_ref().is_some_and(|p| !p.is_empty()),
                has_anon_key: self.anon_key.is_some(),
                login_url: None,
                http_status: None,
                token_expires_at: None,
                message,
                logs,
            };
        };

        let email_log = self.email.as_deref().map(redact_email);
        tracing::info!(target: "ngx_pulse", method = "POST", %url, email = ?email_log, "pulse supabase password grant");
        logs.push(format!("POST {url} (email={:?}, password=[redacted])", email_log));

        match self.login_with_status().await {
            Ok((status, tokens)) => {
                let cid = correlation_id();
                logs.push(format!(
                    "response status={status} expires_at={} cid={cid}",
                    tokens.expires_at
                ));
                tracing::info!(
                    target: "ngx_pulse",
                    %status,
                    expires_at = tokens.expires_at,
                    cid = %cid,
                    "pulse login ok"
                );
                *self.tokens.lock().unwrap() = Some(tokens.clone());
                PulseAuthReport {
                    ok: true,
                    auth_mode: self.auth_mode.as_str().into(),
                    supabase_host,
                    pulse_base_url: self.base_url.clone(),
                    has_email: true,
                    email: self.email.clone(),
                    has_password: true,
                    has_anon_key: true,
                    login_url: Some(url),
                    http_status: Some(status),
                    token_expires_at: Some(tokens.expires_at),
                    message: "Supabase session login succeeded".into(),
                    logs,
                }
            }
            Err((status, err)) => {
                logs.push(format!("response status={:?} error=[redacted]", status));
                tracing::error!(target: "ngx_pulse", ?status, cid = %correlation_id(), "pulse login failed");
                PulseAuthReport {
                    ok: false,
                    auth_mode: self.auth_mode.as_str().into(),
                    supabase_host,
                    pulse_base_url: self.base_url.clone(),
                    has_email: self.email.is_some(),
                    email: self.email.clone(),
                    has_password: self.password.as_ref().is_some_and(|p| !p.is_empty()),
                    has_anon_key: self.anon_key.is_some(),
                    login_url: Some(url),
                    http_status: status,
                    token_expires_at: None,
                    message: err,
                    logs,
                }
            }
        }
    }

    pub async fn test_login(&self) -> Result<()> {
        let report = self.diagnose_login().await;
        if report.ok {
            Ok(())
        } else {
            Err(anyhow!(report.message))
        }
    }

    pub async fn get_market_status(&self) -> Result<NgxMarketStatus> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(NgxMarketStatus {
                status: "Open".into(),
                is_open: true,
            });
        }
        let raw = self.fetch_raw("/ngxdata/market-status", "market-status").await?;
        let data = unwrap(raw);
        Ok(NgxMarketStatus {
            status: data.get("status").and_then(|v| v.as_str()).unwrap_or("Unknown").into(),
            is_open: data.get("is_open").and_then(|v| v.as_bool()).unwrap_or(false),
        })
    }

    /// Fetch the signed-in Supabase user for the Settings profile view.
    pub async fn get_user_profile(&self) -> PulseProfile {
        match self.auth_mode {
            AuthMode::Mock => PulseProfile {
                ok: false,
                auth_mode: self.auth_mode.as_str().into(),
                email: self.email.clone(),
                user_id: None,
                display_name: self.email.as_ref().map(|e| display_name_from_email(e)),
                phone: None,
                created_at: None,
                last_sign_in_at: None,
                email_confirmed: None,
                message: "Pulse session is not active (demo / mock mode).".into(),
            },
            AuthMode::ApiKey => PulseProfile {
                ok: true,
                auth_mode: self.auth_mode.as_str().into(),
                email: self.email.clone(),
                user_id: None,
                display_name: self.email.as_ref().map(|e| display_name_from_email(e)),
                phone: None,
                created_at: None,
                last_sign_in_at: None,
                email_confirmed: None,
                message: "Connected with API key (no user profile endpoint).".into(),
            },
            AuthMode::Session => match self.fetch_supabase_user().await {
                Ok(profile) => profile,
                Err(e) => PulseProfile {
                    ok: false,
                    auth_mode: self.auth_mode.as_str().into(),
                    email: self.email.clone(),
                    user_id: None,
                    display_name: self.email.as_ref().map(|e| display_name_from_email(e)),
                    phone: None,
                    created_at: None,
                    last_sign_in_at: None,
                    email_confirmed: None,
                    message: format!("Could not load Pulse profile: {e}"),
                },
            },
        }
    }

    async fn fetch_supabase_user(&self) -> Result<PulseProfile> {
        let supabase_url = self.supabase_url.as_ref().context("supabase url")?;
        let anon_key = self.anon_key.as_ref().context("anon key")?;
        let access = self.get_access_token().await?;
        let url = format!("{}/auth/v1/user", supabase_url.trim_end_matches('/'));

        let res = self
            .http
            .get(&url)
            .headers(supabase_user_headers(anon_key, &access))
            .send()
            .await
            .context("pulse user request failed")?;

        let status = res.status();
        let text = res.text().await.context("pulse user body")?;
        let body: serde_json::Value = serde_json::from_str(&text).context("pulse user json")?;
        if !status.is_success() {
            let msg = body
                .get("msg")
                .or(body.get("message"))
                .or(body.get("error_description"))
                .and_then(|v| v.as_str())
                .unwrap_or(&text);
            return Err(anyhow!("NGX Pulse user lookup failed ({status}): {msg}"));
        }

        let email = body
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| self.email.clone());
        let meta = body.get("user_metadata").cloned().unwrap_or(serde_json::Value::Null);
        let display_name = meta
            .get("full_name")
            .or(meta.get("name"))
            .or(meta.get("display_name"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| email.as_ref().map(|e| display_name_from_email(e)));

        let phone = body
            .get("phone")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        Ok(PulseProfile {
            ok: true,
            auth_mode: self.auth_mode.as_str().into(),
            email,
            user_id: body.get("id").and_then(|v| v.as_str()).map(str::to_string),
            display_name,
            phone,
            created_at: body
                .get("created_at")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            last_sign_in_at: body
                .get("last_sign_in_at")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            email_confirmed: body
                .get("email_confirmed_at")
                .and_then(|v| {
                    if v.is_null() {
                        Some(false)
                    } else if v.as_str().is_some() {
                        Some(true)
                    } else {
                        None
                    }
                }),
            message: "Pulse session active.".into(),
        })
    }

    pub async fn get_stocks(&self) -> Result<Vec<NgxStock>> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(mock_stocks());
        }

        let mut by_symbol: std::collections::HashMap<String, NgxStock> =
            std::collections::HashMap::new();
        let mut page: u32 = 1;
        loop {
            let path = if page == 1 {
                "/ngxdata/stocks".to_string()
            } else {
                format!("/ngxdata/stocks?page={page}")
            };
            let raw = self.fetch_raw(&path, "stocks").await?;
            let has_more = page_has_more(&raw, page);
            let chunk = normalize_stocks(raw);
            if chunk.is_empty() {
                break;
            }
            for stock in chunk {
                by_symbol.insert(stock.symbol.clone(), stock);
            }
            if !has_more || page >= 50 {
                break;
            }
            page += 1;
        }

        tracing::info!(
            target: "ngx_pulse",
            count = by_symbol.len(),
            pages = page,
            "pulse stocks ingested from API"
        );
        Ok(by_symbol.into_values().collect())
    }

    pub async fn get_market(&self) -> Result<NgxMarketOverview> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(NgxMarketOverview {
                asi: Some(AsiData {
                    value: 98000.0,
                    change_percent: Some(0.5),
                }),
                market_cap: None,
            });
        }
        let raw = self.fetch_raw("/ngxdata/market", "market").await?;
        Ok(normalize_market(raw))
    }

    pub async fn get_indices(&self) -> Result<Vec<NgxIndex>> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(vec![NgxIndex {
                code: "ASI".into(),
                value: 98000.0,
                points: Some(120.0),
                week_change: None,
                month_change: None,
                year_change: None,
            }]);
        }
        let raw = self.fetch_raw("/ngxdata/indices", "indices").await?;
        Ok(normalize_indices(raw))
    }

    pub async fn get_symbol_price(
        &self,
        symbol: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<PricePoint>> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(mock_historical(symbol));
        }
        let mut path = format!("/ngxdata/prices/{symbol}");
        let mut params = vec![];
        if let Some(f) = from {
            params.push(format!("from={f}"));
        }
        if let Some(t) = to {
            params.push(format!("to={t}"));
        }
        if !params.is_empty() {
            path.push('?');
            path.push_str(&params.join("&"));
        }
        let raw = self.fetch_raw(&path, &format!("prices/{symbol}")).await?;
        Ok(normalize_prices(raw))
    }

    /// Last print (and previous bar) for held names only. Caps at 20 symbols.
    pub async fn get_latest_quotes(&self, symbols: &[String]) -> Result<Vec<LatestQuote>> {
        let calendar = crate::calendar::TradingCalendar::default();
        let today = calendar.today_wat();
        let from = (calendar.now_wat() - chrono::Duration::days(7))
            .format("%Y-%m-%d")
            .to_string();
        let mut out = Vec::new();
        for symbol in symbols.iter().take(20) {
            if symbol.is_empty() {
                continue;
            }
            if self.auth_mode == AuthMode::ApiKey {
                break;
            }
            let mut points = match self
                .get_symbol_price(symbol, Some(&from), Some(&today))
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(target: "ngx_pulse", symbol = %symbol, error = %e, "quote fetch failed");
                    continue;
                }
            };
            if points.is_empty() {
                continue;
            }
            points.sort_by(|a, b| a.date.cmp(&b.date));
            let last = points.last().expect("non-empty");
            let prev_close = if points.len() >= 2 {
                points.get(points.len() - 2).map(|p| p.price)
            } else {
                None
            };
            out.push(LatestQuote {
                symbol: symbol.clone(),
                last: last.price,
                prev_close,
                trade_date: last.date.clone(),
            });
        }
        Ok(out)
    }

    async fn fetch_raw(&self, path: &str, endpoint: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let token = self.get_access_token().await?;
        let cid = correlation_id();
        tracing::info!(
            target: "ngx_pulse",
            method = "GET",
            %url,
            auth_mode = self.auth_mode.as_str(),
            cid = %cid,
            "pulse API request"
        );
        let mut res = self
            .http
            .get(&url)
            .headers(request_headers(&token))
            .send()
            .await
            .context("ngx pulse request")?;

        if res.status().as_u16() == 401 && self.auth_mode == AuthMode::Session {
            tracing::warn!(target: "ngx_pulse", %url, "401 — clearing session and retrying");
            *self.tokens.lock().unwrap() = None;
            let retry_token = self.get_access_token().await?;
            res = self
                .http
                .get(&url)
                .headers(request_headers(&retry_token))
                .send()
                .await
                .context("ngx pulse retry")?;
        }

        let status = res.status();
        tracing::info!(target: "ngx_pulse", %url, %status, endpoint, "pulse API response");

        if !status.is_success() {
            return Err(anyhow!("NGX Pulse error: {status}"));
        }

        res.json().await.context("parse ngx response")
    }

    async fn get_access_token(&self) -> Result<String> {
        match self.auth_mode {
            AuthMode::Mock => Ok(String::new()),
            AuthMode::ApiKey => self.api_key.clone().ok_or_else(|| anyhow!("API key missing")),
            AuthMode::Session => self.ensure_session().await,
        }
    }

    async fn ensure_session(&self) -> Result<String> {
        {
            let guard = self.tokens.lock().unwrap();
            if let Some(tokens) = guard.as_ref() {
                if !is_expiring_soon(tokens.expires_at) {
                    return Ok(tokens.access_token.clone());
                }
            }
        }

        let refresh = self
            .tokens
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.refresh_token.clone());
        if let Some(refresh) = refresh {
            if self.refresh_session(&refresh).await.is_ok() {
                return Ok(self
                    .tokens
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .access_token
                    .clone());
            }
            *self.tokens.lock().unwrap() = None;
        }

        self.login().await?;
        Ok(self
            .tokens
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .access_token
            .clone())
    }

    async fn login(&self) -> Result<()> {
        let (_, tokens) = self
            .login_with_status()
            .await
            .map_err(|(_, e)| anyhow!(e))?;
        *self.tokens.lock().unwrap() = Some(tokens);
        Ok(())
    }

    async fn login_with_status(&self) -> Result<(u16, SessionTokens), (Option<u16>, String)> {
        let supabase_url = self
            .supabase_url
            .as_ref()
            .ok_or_else(|| (None, "supabase url missing".into()))?;
        let anon_key = self
            .anon_key
            .as_ref()
            .ok_or_else(|| (None, "anon key missing".into()))?;
        let email = self
            .email
            .as_ref()
            .ok_or_else(|| (None, "email missing".into()))?;
        let password = self
            .password
            .as_ref()
            .ok_or_else(|| (None, "password missing".into()))?;

        let url = format!(
            "{}/auth/v1/token?grant_type=password",
            supabase_url.trim_end_matches('/')
        );
        let body = serde_json::json!({
            "email": email,
            "password": password,
            "gotrue_meta_security": {}
        });

        tracing::info!(target: "ngx_pulse", method = "POST", %url, %email, "sending password grant");

        let res = self
            .http
            .post(&url)
            .headers(supabase_headers(anon_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| (None, format!("pulse login request failed: {e}")))?;

        let status = res.status().as_u16();
        parse_auth_response(res)
            .await
            .map(|tokens| (status, tokens))
            .map_err(|e| (Some(status), e.to_string()))
    }

    async fn refresh_session(&self, refresh_token: &str) -> Result<()> {
        let supabase_url = self.supabase_url.as_ref().context("supabase url")?;
        let anon_key = self.anon_key.as_ref().context("anon key")?;
        let url = format!("{}/auth/v1/token?grant_type=refresh_token", supabase_url.trim_end_matches('/'));
        let body = serde_json::json!({ "refresh_token": refresh_token });

        tracing::info!(target: "ngx_pulse", method = "POST", %url, "sending refresh grant");

        let res = self
            .http
            .post(&url)
            .headers(supabase_headers(anon_key))
            .json(&body)
            .send()
            .await
            .context("pulse refresh")?;

        tracing::info!(target: "ngx_pulse", status = %res.status(), "pulse refresh response");

        let tokens = parse_auth_response(res).await?;
        *self.tokens.lock().unwrap() = Some(tokens);
        Ok(())
    }
}

fn is_expiring_soon(expires_at: i64) -> bool {
    let now = chrono::Utc::now().timestamp();
    expires_at - now <= TOKEN_REFRESH_BUFFER_SEC
}

fn request_headers(token: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::ACCEPT, "application/json".parse().unwrap());
    headers.insert(reqwest::header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    headers.insert(reqwest::header::REFERER, "https://ngxpulse.ng/".parse().unwrap());
    headers.insert(reqwest::header::USER_AGENT, USER_AGENT.parse().unwrap());
    headers
}

fn supabase_headers(anon_key: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("apikey", anon_key.parse().unwrap());
    headers.insert(reqwest::header::AUTHORIZATION, format!("Bearer {anon_key}").parse().unwrap());
    headers.insert(reqwest::header::CONTENT_TYPE, "application/json;charset=UTF-8".parse().unwrap());
    headers.insert("origin", "https://ngxpulse.ng".parse().unwrap());
    headers.insert(reqwest::header::REFERER, "https://ngxpulse.ng/".parse().unwrap());
    headers.insert(reqwest::header::USER_AGENT, USER_AGENT.parse().unwrap());
    headers.insert("x-client-info", "supabase-js/2.112.1; runtime=web".parse().unwrap());
    headers.insert("x-supabase-api-version", "2024-01-01".parse().unwrap());
    headers
}

fn supabase_user_headers(anon_key: &str, access_token: &str) -> reqwest::header::HeaderMap {
    let mut headers = supabase_headers(anon_key);
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {access_token}").parse().unwrap(),
    );
    headers
}

fn display_name_from_email(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    let cleaned = local.replace(['.', '_', '-'], " ");
    let mut parts = cleaned
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return "NGX User".into();
    }
    if parts.len() > 2 {
        parts.truncate(2);
    }
    parts.join(" ")
}

async fn parse_auth_response(res: reqwest::Response) -> Result<SessionTokens> {
    let status = res.status();
    let text = res.text().await?;
    let body: serde_json::Value = serde_json::from_str(&text).context("auth json")?;
    if !status.is_success() {
        let msg = body
            .get("error_description")
            .or(body.get("msg"))
            .or(body.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or(&text);
        return Err(anyhow!("NGX Pulse auth failed ({status}): {msg}"));
    }
    let access = body.get("access_token").and_then(|v| v.as_str()).context("access_token")?;
    let refresh = body.get("refresh_token").and_then(|v| v.as_str()).context("refresh_token")?;
    let expires_at = body
        .get("expires_at")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp() + body.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(3600));
    Ok(SessionTokens {
        access_token: access.into(),
        refresh_token: refresh.into(),
        expires_at,
    })
}

fn unwrap(payload: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    if let Some(data) = payload.get("data").and_then(|v| v.as_object()) {
        return data.clone();
    }
    payload.as_object().cloned().unwrap_or_default()
}

fn pagination_last_page(payload: &serde_json::Value) -> Option<u32> {
    let candidates = [
        payload.pointer("/meta/last_page"),
        payload.pointer("/last_page"),
        payload.pointer("/data/meta/last_page"),
        payload.pointer("/data/last_page"),
        payload.pointer("/pagination/last_page"),
        payload.pointer("/meta/lastPage"),
    ];
    for v in candidates.into_iter().flatten() {
        if let Some(n) = v.as_u64() {
            return Some(n as u32);
        }
        if let Some(n) = v.as_i64().filter(|x| *x > 0) {
            return Some(n as u32);
        }
    }
    None
}

fn page_has_more(payload: &serde_json::Value, page: u32) -> bool {
    if let Some(last) = pagination_last_page(payload) {
        return page < last;
    }
    payload
        .pointer("/links/next")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty() && s != "null")
}

fn stock_rows(payload: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    payload
        .get("stocks")
        .and_then(|v| v.as_array())
        .or_else(|| payload.get("data").and_then(|d| d.get("stocks")).and_then(|v| v.as_array()))
        .or_else(|| payload.get("data").and_then(|v| v.as_array()))
        .or_else(|| payload.as_array())
}

fn parse_stock_row(stock: &serde_json::Value) -> Option<NgxStock> {
    let symbol = stock
        .get("symbol")
        .or_else(|| stock.get("ticker"))
        .and_then(|v| v.as_str())?
        .to_uppercase();
    if !is_valid_ticker(&symbol) {
        tracing::warn!(target: "ngx_pulse", %symbol, "dropping invalid ticker");
        return None;
    }
    let price = stock
        .get("current_price")
        .or_else(|| stock.get("price"))
        .or_else(|| stock.get("close_price"))
        .or_else(|| stock.get("last_price"))
        .and_then(|v| v.as_f64())?;
    Some(NgxStock {
        symbol,
        name: stock
            .get("name")
            .and_then(|v| v.as_str())
            .map(sanitize_text)
            .filter(|s| !s.is_empty()),
        price,
        change_percent: stock
            .get("change_percent")
            .or_else(|| stock.get("official_change_percent"))
            .or_else(|| stock.get("pct_change"))
            .and_then(|v| v.as_f64()),
        volume: stock
            .get("volume")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|n| n as i64))),
        market_cap: stock.get("market_cap").and_then(|v| v.as_f64()),
        pe_ratio: stock.get("pe_ratio").and_then(|v| v.as_f64()),
        sector: stock
            .get("sector")
            .and_then(|v| v.as_str())
            .map(sanitize_text)
            .filter(|s| !s.is_empty()),
    })
}

fn normalize_stocks(payload: serde_json::Value) -> Vec<NgxStock> {
    let Some(arr) = stock_rows(&payload) else {
        return vec![];
    };
    arr.iter().filter_map(parse_stock_row).collect()
}

fn normalize_market(payload: serde_json::Value) -> NgxMarketOverview {
    let data = unwrap(payload);
    let asi_value = data
        .get("asi")
        .and_then(|v| v.get("value").and_then(|x| x.as_f64()).or(v.as_f64()))
        .unwrap_or(0.0);
    let change = data
        .get("asi")
        .and_then(|v| v.get("change_percent").and_then(|x| x.as_f64()))
        .or_else(|| data.get("pct_change").and_then(|v| v.as_f64()))
        .or_else(|| data.get("change_percent").and_then(|v| v.as_f64()));
    NgxMarketOverview {
        asi: if asi_value > 0.0 {
            Some(AsiData {
                value: asi_value,
                change_percent: change,
            })
        } else {
            None
        },
        market_cap: data.get("market_cap").and_then(|v| v.as_f64()),
    }
}

fn normalize_indices(payload: serde_json::Value) -> Vec<NgxIndex> {
    let arr = payload
        .get("data")
        .or(payload.get("data"))
        .and_then(|v| v.as_array())
        .or_else(|| payload.as_array());
    let Some(arr) = arr else {
        return vec![];
    };
    arr.iter()
        .filter_map(|idx| {
            Some(NgxIndex {
                code: idx.get("code")?.as_str()?.into(),
                value: idx.get("currentPrice").or(idx.get("value"))?.as_f64()?,
                points: idx.get("points").and_then(|v| v.as_f64()),
                week_change: idx.get("weekChange").and_then(|v| v.as_f64()),
                month_change: idx.get("monthChange").and_then(|v| v.as_f64()),
                year_change: idx.get("yearChange").and_then(|v| v.as_f64()),
            })
        })
        .collect()
}

fn normalize_prices(payload: serde_json::Value) -> Vec<PricePoint> {
    let arr = payload
        .get("prices")
        .or(payload.get("data"))
        .and_then(|v| v.as_array());
    let Some(arr) = arr else {
        return vec![];
    };
    arr.iter()
        .filter_map(|row| {
            let date = row
                .get("trade_date")
                .or(row.get("date"))?
                .as_str()?
                .split('T')
                .next()?
                .to_string();
            Some(PricePoint {
                date,
                price: row
                    .get("last_price")
                    .or(row.get("close_price"))
                    .or(row.get("price"))?
                    .as_f64()?,
                volume: row.get("volume").and_then(|v| v.as_i64()),
            })
        })
        .collect()
}

fn mock_stocks() -> Vec<NgxStock> {
    let symbols = [
        ("DANGCEM", 285.0),
        ("GTCO", 46.0),
        ("ZENITHBANK", 39.0),
        ("MTNN", 225.0),
        ("BUACEMENT", 98.0),
        ("ACCESSCORP", 22.0),
        ("UBA", 28.0),
        ("FBNH", 18.0),
        ("SEPLAT", 3200.0),
        ("NESTLE", 1200.0),
        ("BUAFOODS", 150.0),
        ("AIRTELAFRI", 2100.0),
        ("WAPCO", 35.0),
        ("GUARANTY", 55.0),
        ("STANBIC", 65.0),
        ("FLOURMILL", 42.0),
        ("PRESCO", 280.0),
        ("OKOMUOIL", 350.0),
        ("NASCON", 18.0),
        ("INTBREW", 5.0),
    ];
    symbols
        .iter()
        .map(|(s, p)| NgxStock {
            symbol: (*s).into(),
            name: None,
            price: *p,
            change_percent: Some(0.5),
            volume: Some(100_000),
            market_cap: None,
            pe_ratio: None,
            sector: None,
        })
        .collect()
}

fn mock_historical(_symbol: &str) -> Vec<PricePoint> {
    let mut rows = vec![];
    let today = chrono::Utc::now();
    let mut price = 100.0;
    for i in (0..30).rev() {
        let d = today - chrono::Duration::days(i);
        price *= 1.0 + (0.02 * (i as f64 / 30.0 - 0.5));
        rows.push(PricePoint {
            date: d.format("%Y-%m-%d").to_string(),
            price: (price * 100.0).round() / 100.0,
            volume: Some(1000),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::is_valid_ticker;

    #[test]
    fn rejects_malicious_tickers() {
        assert!(is_valid_ticker("GTCO"));
        assert!(is_valid_ticker("MTN-N"));
        assert!(!is_valid_ticker("GTCO\nDROP"));
        assert!(!is_valid_ticker("ignore previous instructions"));
        assert!(!is_valid_ticker(&"A".repeat(17)));
        assert!(!is_valid_ticker(""));
    }
}
