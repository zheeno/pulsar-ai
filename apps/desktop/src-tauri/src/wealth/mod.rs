//! Coronation Wealth App REST client for live trading.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http_client::http_client;
use crate::runtime_util::is_dev;
use crate::secrets::{
    delete_secret, get_secret, set_secret, SECRET_WEALTH_PASSWORD, SECRET_WEALTH_REFRESH_TOKEN,
    SECRET_WEALTH_TOKEN, SECRET_WEALTH_TOKEN_EXPIRES,
};
use crate::settings::AppSettings;

pub const WEALTH_BASE_URL_DEV: &str = "https://wealthapp-api-stg-app.azurewebsites.net/v1";
pub const WEALTH_BASE_URL_PROD: &str = "https://api.wealthapp.coronation.ng/v1";

const ORDER_POLL_INTERVAL_MS: u64 = 2_000;
const ORDER_POLL_MAX_ATTEMPTS: u32 = 30;

pub fn wealth_base_url() -> &'static str {
    if is_dev() {
        WEALTH_BASE_URL_DEV
    } else {
        WEALTH_BASE_URL_PROD
    }
}

/// Parse money/qty fields that Wealth may send as numbers or numeric strings.
fn json_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t = s.trim().replace(',', "");
            if t.is_empty() {
                None
            } else {
                t.parse().ok()
            }
        }
        _ => None,
    }
}

fn json_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|u| u as i64)).or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && *f >= 0.0)
                .map(|f| f as i64)
        }),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn json_field_f64(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(json_f64)
}

fn pick_cash(wallet: Option<&WealthWallet>, previous: Option<f64>) -> f64 {
    if let Some(w) = wallet {
        return w.brokerage_balance;
    }
    previous.unwrap_or(0.0)
}

fn json_symbol(obj: Option<&Value>) -> Option<String> {
    let obj = obj?;
    ["symbol", "ticker", "stock_symbol", "code"]
        .into_iter()
        .find_map(|k| obj.get(k).and_then(|v| v.as_str()))
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
}

fn parse_stock_lot(row: &Value) -> Option<WealthHolding> {
    let nested = row.get("stock");
    let symbol = json_symbol(nested).or_else(|| json_symbol(Some(row))).unwrap_or_default();
    let stock_id = nested
        .and_then(|s| s.get("id"))
        .or_else(|| row.get("stock_id"))
        .or_else(|| row.get("id"))
        .and_then(json_i64)
        .unwrap_or(0);
    if symbol.is_empty() && stock_id <= 0 {
        return None;
    }
    let quantity = json_field_f64(row, "quantity").unwrap_or(0.0);
    let price = nested
        .and_then(|s| s.get("price").or_else(|| s.get("close_price")))
        .and_then(json_f64)
        .or_else(|| json_field_f64(row, "price"))
        .or_else(|| json_field_f64(row, "close_price"))
        .unwrap_or(0.0);
    let current_value = json_field_f64(row, "current_value").unwrap_or(quantity * price);
    let buy_price = json_field_f64(row, "buy_price");
    Some(WealthHolding {
        stock_id,
        symbol,
        quantity,
        current_value,
        buy_price,
        price,
    })
}

fn parse_portfolio_snapshot(raw: &Value) -> WealthPortfolioSnapshot {
    let root = if raw.get("stocks").is_some() {
        raw
    } else {
        raw.get("data").unwrap_or(raw)
    };
    let stocks = root.get("stocks").and_then(|v| v.as_array());
    let stocks_present = stocks.is_some();
    let mut value_change_sum = 0.0;
    let mut any_value_change = false;
    let holdings: Vec<WealthHolding> = stocks
        .map(|arr| {
            arr.iter()
                .filter_map(|row| {
                    if let Some(vc) = json_field_f64(row, "value_change") {
                        value_change_sum += vc;
                        any_value_change = true;
                    }
                    parse_stock_lot(row)
                })
                .collect()
        })
        .unwrap_or_default();
    let stock_value = json_field_f64(root, "stock_value")
        .filter(|v| *v > 0.0)
        .unwrap_or_else(|| holdings.iter().map(|h| h.current_value).sum());
    let profit = if any_value_change {
        value_change_sum
    } else {
        json_field_f64(root, "profit").unwrap_or(0.0)
    };
    WealthPortfolioSnapshot {
        balance: json_field_f64(root, "balance").unwrap_or(0.0),
        profit,
        stock_value,
        holdings,
        stocks_present,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TradingMode {
    Sandbox,
    Live,
}

impl TradingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TradingMode::Sandbox => "sandbox",
            TradingMode::Live => "live",
        }
    }
}

#[derive(Debug, Clone)]
struct SessionToken {
    access_token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WealthLoginResult {
    pub ok: bool,
    pub needs_2fa: bool,
    pub temp_token: Option<String>,
    pub connected: bool,
    pub message: String,
    pub email: Option<String>,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WealthProfileStatus {
    pub ok: bool,
    pub connected: bool,
    pub email: Option<String>,
    pub trading_profile: Option<String>,
    pub trading_verified: bool,
    pub trading_mode: String,
    pub brokerage_balance: Option<f64>,
    pub available_balance: Option<f64>,
    pub current_balance: Option<f64>,
    pub base_url: String,
    pub message: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WealthWallet {
    pub brokerage_balance: f64,
    pub available_balance: f64,
    pub current_balance: f64,
}

#[derive(Debug, Clone)]
pub struct WealthHolding {
    pub stock_id: i64,
    pub symbol: String,
    pub quantity: f64,
    pub current_value: f64,
    pub buy_price: Option<f64>,
    pub price: f64,
}

#[derive(Debug, Clone)]
pub struct WealthPortfolioSnapshot {
    pub balance: f64,
    pub profit: f64,
    pub stock_value: f64,
    pub holdings: Vec<WealthHolding>,
    pub stocks_present: bool,
}

#[derive(Debug, Clone)]
pub struct WealthFee {
    pub accurate_fee: f64,
    pub rounded_fee: f64,
}

#[derive(Debug, Clone)]
pub struct WealthOrder {
    pub id: i64,
    pub stock_id: i64,
    pub transaction_type: String,
    pub status: String,
    pub quantity: f64,
    pub quote_price: Option<f64>,
    pub unit_price: Option<f64>,
    pub estimated_amount: Option<f64>,
    pub rejection_reason: Option<String>,
}

pub struct WealthClient {
    http: Client,
    base_url: String,
    email: Option<String>,
    password: Option<String>,
    tokens: Arc<Mutex<Option<SessionToken>>>,
}

impl WealthClient {
    pub fn from_settings(settings: &AppSettings, password: Option<String>) -> Self {
        let token = get_secret(SECRET_WEALTH_TOKEN).ok().flatten();
        let expires = get_secret(SECRET_WEALTH_TOKEN_EXPIRES)
            .ok()
            .flatten()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc));

        let session = match (token, expires) {
            (Some(access_token), Some(expires_at)) if !access_token.is_empty() => {
                Some(SessionToken {
                    access_token,
                    expires_at,
                })
            }
            _ => None,
        };

        Self {
            http: http_client().expect("http client"),
            base_url: wealth_base_url().to_string(),
            email: settings.wealth_email.clone(),
            password,
            tokens: Arc::new(Mutex::new(session)),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn has_session(&self) -> bool {
        self.tokens.lock().unwrap().is_some()
            || get_secret(SECRET_WEALTH_TOKEN)
                .ok()
                .flatten()
                .is_some_and(|t| !t.is_empty())
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<WealthLoginResult> {
        let url = format!("{}/login", self.base_url.trim_end_matches('/'));
        tracing::info!(target: "wealth", %url, email = %email, "wealth login request");
        let res = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await
            .context("wealth login request")?;

        let status = res.status();
        let body: Value = res.json().await.context("parse wealth login")?;
        tracing::info!(
            target: "wealth",
            %status,
            keys = %json_keys_preview(&body),
            "wealth login response"
        );

        let auth = unwrap_auth_body(&body);

        if needs_2fa(&body) || needs_2fa(&auth) {
            let temp = extract_token(&auth).or_else(|| extract_token(&body));
            return Ok(WealthLoginResult {
                ok: true,
                needs_2fa: true,
                temp_token: temp,
                connected: false,
                message: "Two-factor authentication required.".into(),
                email: Some(email.to_string()),
                base_url: self.base_url.clone(),
            });
        }

        if !status.is_success() {
            let msg = extract_error_message(&body)
                .or_else(|| extract_error_message(&auth))
                .unwrap_or_else(|| format!("Wealth login failed ({status})"));
            return Ok(WealthLoginResult {
                ok: false,
                needs_2fa: false,
                temp_token: None,
                connected: false,
                message: msg,
                email: Some(email.to_string()),
                base_url: self.base_url.clone(),
            });
        }

        // Success with no token often means 2FA challenge with an atypical shape.
        if extract_token(&auth).or_else(|| extract_token(&body)).is_none() {
            if looks_like_2fa_challenge(&body) || looks_like_2fa_challenge(&auth) {
                return Ok(WealthLoginResult {
                    ok: true,
                    needs_2fa: true,
                    temp_token: extract_temp_token(&auth).or_else(|| extract_temp_token(&body)),
                    connected: false,
                    message: "Two-factor authentication required.".into(),
                    email: Some(email.to_string()),
                    base_url: self.base_url.clone(),
                });
            }
        }

        self.persist_auth(&auth, email, Some(password)).or_else(|_| {
            self.persist_auth(&body, email, Some(password))
        })?;
        Ok(WealthLoginResult {
            ok: true,
            needs_2fa: false,
            temp_token: None,
            connected: true,
            message: "Wealth account connected.".into(),
            email: Some(email.to_string()),
            base_url: self.base_url.clone(),
        })
    }

    pub async fn verify_2fa(&self, temp_token: &str, code: &str, email: &str) -> Result<WealthLoginResult> {
        let url = format!("{}/auth/check-2fa", self.base_url.trim_end_matches('/'));
        let res = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "token": temp_token, "code": code }))
            .send()
            .await
            .context("wealth 2fa request")?;

        let status = res.status();
        let body: Value = res.json().await.context("parse wealth 2fa")?;
        tracing::info!(
            target: "wealth",
            %status,
            keys = %json_keys_preview(&body),
            "wealth 2fa response"
        );
        let auth = unwrap_auth_body(&body);

        if !status.is_success() {
            let msg = extract_error_message(&body)
                .or_else(|| extract_error_message(&auth))
                .unwrap_or_else(|| format!("2FA verification failed ({status})"));
            return Ok(WealthLoginResult {
                ok: false,
                needs_2fa: true,
                temp_token: Some(temp_token.to_string()),
                connected: false,
                message: msg,
                email: Some(email.to_string()),
                base_url: self.base_url.clone(),
            });
        }

        let password = self.password.clone().or_else(|| {
            get_secret(SECRET_WEALTH_PASSWORD).ok().flatten()
        });
        self.persist_auth(&auth, email, password.as_deref()).or_else(|_| {
            self.persist_auth(&body, email, password.as_deref())
        })?;
        Ok(WealthLoginResult {
            ok: true,
            needs_2fa: false,
            temp_token: None,
            connected: true,
            message: "Wealth account connected.".into(),
            email: Some(email.to_string()),
            base_url: self.base_url.clone(),
        })
    }

    pub async fn logout_remote(&self) {
        if let Ok(token) = self.access_token().await {
            let url = format!("{}/auth/logout", self.base_url.trim_end_matches('/'));
            let _ = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await;
        }
    }

    pub fn clear_local_secrets() {
        let _ = delete_secret(SECRET_WEALTH_TOKEN);
        let _ = delete_secret(SECRET_WEALTH_TOKEN_EXPIRES);
        let _ = delete_secret(SECRET_WEALTH_REFRESH_TOKEN);
        let _ = delete_secret(SECRET_WEALTH_PASSWORD);
    }

    fn persist_auth(&self, body: &Value, email: &str, password: Option<&str>) -> Result<()> {
        let token = extract_token(body).ok_or_else(|| {
            anyhow!(
                "Wealth auth response missing token (keys: {})",
                json_keys_preview(body)
            )
        })?;

        let ttl_mins = extract_ttl_minutes(body).unwrap_or(60);
        let expires_at = body
            .get("refresh_token_ttl")
            .or_else(|| body.get("refresh_ttl"))
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::minutes(ttl_mins.max(1)));

        set_secret(SECRET_WEALTH_TOKEN, &token)?;
        set_secret(SECRET_WEALTH_TOKEN_EXPIRES, &expires_at.to_rfc3339())?;
        if let Some(refresh) = body
            .get("refresh_token")
            .or_else(|| body.get("refreshToken"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            let _ = set_secret(SECRET_WEALTH_REFRESH_TOKEN, refresh);
        }
        if let Some(pw) = password {
            set_secret(SECRET_WEALTH_PASSWORD, pw)?;
        }

        *self.tokens.lock().unwrap() = Some(SessionToken {
            access_token: token,
            expires_at,
        });

        tracing::info!(
            target: "wealth",
            email = %email,
            expires_at = %expires_at.to_rfc3339(),
            ttl_mins,
            "wealth auth persisted"
        );
        Ok(())
    }

    async fn access_token(&self) -> Result<String> {
        {
            let guard = self.tokens.lock().unwrap();
            if let Some(tok) = guard.as_ref() {
                if tok.expires_at > Utc::now() + chrono::Duration::seconds(30) {
                    return Ok(tok.access_token.clone());
                }
            }
        }

        if let Ok(Some(stored)) = get_secret(SECRET_WEALTH_TOKEN) {
            if !stored.is_empty() {
                if self.refresh_token().await.is_ok() {
                    if let Some(tok) = self.tokens.lock().unwrap().as_ref() {
                        return Ok(tok.access_token.clone());
                    }
                }
                // Fall through to re-login if refresh fails but we still have a stored token.
                let expires = get_secret(SECRET_WEALTH_TOKEN_EXPIRES)
                    .ok()
                    .flatten()
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc));
                if let Some(exp) = expires {
                    if exp > Utc::now() {
                        *self.tokens.lock().unwrap() = Some(SessionToken {
                            access_token: stored.clone(),
                            expires_at: exp,
                        });
                        return Ok(stored);
                    }
                }
            }
        }

        if let (Some(email), Some(password)) = (
            self.email.clone(),
            self.password
                .clone()
                .or_else(|| get_secret(SECRET_WEALTH_PASSWORD).ok().flatten()),
        ) {
            let result = self.login(&email, &password).await?;
            if result.ok && !result.needs_2fa {
                if let Some(tok) = self.tokens.lock().unwrap().as_ref() {
                    return Ok(tok.access_token.clone());
                }
            }
            return Err(anyhow!(result.message));
        }

        Err(anyhow!("Wealth session expired — reconnect in Settings"))
    }

    async fn refresh_token(&self) -> Result<()> {
        let current = get_secret(SECRET_WEALTH_TOKEN)?
            .ok_or_else(|| anyhow!("no wealth token to refresh"))?;
        let url = format!("{}/auth/refresh", self.base_url.trim_end_matches('/'));
        let res = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {current}"))
            .send()
            .await
            .context("wealth refresh")?;

        if !res.status().is_success() {
            return Err(anyhow!("Wealth token refresh failed"));
        }
        let body: Value = res.json().await.context("parse wealth refresh")?;
        let auth = unwrap_auth_body(&body);
        let token = extract_token(&auth)
            .or_else(|| extract_token(&body))
            .ok_or_else(|| {
                anyhow!(
                    "refresh missing token (keys: {})",
                    json_keys_preview(&body)
                )
            })?;
        let ttl_mins = auth
            .get("ttl")
            .or_else(|| body.get("ttl"))
            .and_then(|v| v.as_f64())
            .unwrap_or(60.0) as i64;
        let expires_at = Utc::now() + chrono::Duration::minutes(ttl_mins.max(1));
        set_secret(SECRET_WEALTH_TOKEN, &token)?;
        set_secret(SECRET_WEALTH_TOKEN_EXPIRES, &expires_at.to_rfc3339())?;
        *self.tokens.lock().unwrap() = Some(SessionToken {
            access_token: token,
            expires_at,
        });
        Ok(())
    }

    async fn request_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let token = self.access_token().await?;
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        tracing::debug!(target: "wealth", method = %method, %url, "wealth API request");
        let mut req = self
            .http
            .request(method.clone(), &url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json");
        if let Some(b) = body.clone() {
            req = req.json(&b);
        }
        let res = req.send().await.context("wealth request")?;
        if res.status().as_u16() == 401 {
            tracing::warn!(target: "wealth", %url, "401 — refreshing wealth token");
            self.refresh_token().await?;
            let token = self.access_token().await?;
            let mut retry = self
                .http
                .request(method, &url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/json")
                .header("Content-Type", "application/json");
            if let Some(b) = body {
                retry = retry.json(&b);
            }
            let retry_res = retry.send().await.context("wealth retry")?;
            if !retry_res.status().is_success() {
                let status = retry_res.status();
                let _err_body = retry_res.text().await.unwrap_or_default();
                tracing::warn!(target: "wealth", %url, %status, "wealth retry failed");
                return Err(anyhow!("Wealth API error {status}"));
            }
            return retry_res.json().await.context("parse wealth retry");
        }
        if !res.status().is_success() {
            let status = res.status();
            let _err_body = res.text().await.unwrap_or_default();
            tracing::warn!(target: "wealth", %url, %status, "wealth API error");
            return Err(anyhow!("Wealth API error {status}"));
        }
        tracing::info!(target: "wealth", %url, status = %res.status(), "wealth API ok");
        // Some endpoints return empty body (204)
        let text = res.text().await.unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).context("parse wealth json")
    }

    pub async fn get_profile_raw(&self) -> Result<Value> {
        // Prefer documented with[] relations; fall back to bare /profile.
        match self
            .request_json(
                reqwest::Method::GET,
                "/profile?with%5B%5D=wallet&with%5B%5D=profile_verification_state&with_kyc_data=true",
                None,
            )
            .await
        {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(target: "wealth", error = %e, "profile with relations failed; retrying bare /profile");
                self.request_json(reqwest::Method::GET, "/profile", None)
                    .await
            }
        }
    }

    pub async fn profile_status(&self, settings: &AppSettings) -> WealthProfileStatus {
        let base = WealthProfileStatus {
            ok: false,
            connected: settings.wealth_connected && self.has_session(),
            email: settings.wealth_email.clone(),
            trading_profile: None,
            trading_verified: false,
            trading_mode: TradingMode::Sandbox.as_str().into(),
            brokerage_balance: None,
            available_balance: None,
            current_balance: None,
            base_url: self.base_url.clone(),
            message: "Wealth account not connected.".into(),
            display_name: None,
        };

        if !settings.wealth_connected || !self.has_session() {
            return base;
        }

        match self.get_profile_raw().await {
            Ok(profile) => {
                tracing::info!(
                    target: "wealth",
                    keys = %json_keys_preview(&profile),
                    "wealth profile response"
                );
                let root = profile
                    .get("data")
                    .cloned()
                    .unwrap_or_else(|| profile.clone());

                let trading_profile = find_trading_profile(&root).or_else(|| find_trading_profile(&profile));
                let verified = trading_profile
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case("verified"))
                    .unwrap_or(false);
                let wallet = root
                    .get("wallet")
                    .or_else(|| profile.get("wallet"))
                    .or_else(|| root.pointer("/user/wallet"))
                    .cloned();
                let brokerage = wallet
                    .as_ref()
                    .and_then(|w| json_field_f64(w, "brokerage_balance"));
                let available = wallet
                    .as_ref()
                    .and_then(|w| json_field_f64(w, "available_balance"));
                let current = wallet.as_ref().and_then(|w| json_field_f64(w, "current_balance"));
                let display_name = root
                    .get("first_name")
                    .or_else(|| root.pointer("/user/first_name"))
                    .and_then(|v| v.as_str())
                    .map(|first| {
                        let last = root
                            .get("last_name")
                            .or_else(|| root.pointer("/user/last_name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        format!("{first} {last}").trim().to_string()
                    })
                    .filter(|s| !s.is_empty());

                // Connected Wealth accounts surface live balances on Home.
                // Order execution still requires trading_verified (see resolve_trading_mode).
                WealthProfileStatus {
                    ok: true,
                    connected: true,
                    email: settings.wealth_email.clone().or_else(|| {
                        root.get("email")
                            .or_else(|| root.pointer("/user/email"))
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    }),
                    trading_profile: trading_profile.clone(),
                    trading_verified: verified,
                    // "live" = show Wealth portfolio on Home when session is connected.
                    trading_mode: "live".into(),
                    brokerage_balance: brokerage,
                    available_balance: available,
                    current_balance: current,
                    base_url: self.base_url.clone(),
                    message: if verified {
                        "Live trader mode — orders use Wealth brokerage balance.".into()
                    } else {
                        format!(
                            "Wealth connected. Trading profile: {}. Balances from Wealth; live orders require verified trading profile.",
                            trading_profile.as_deref().unwrap_or("unknown")
                        )
                    },
                    display_name,
                }
            }
            Err(e) => {
                tracing::warn!(target: "wealth", error = %e, "wealth profile fetch failed");
                WealthProfileStatus {
                    ok: false,
                    connected: true,
                    email: settings.wealth_email.clone(),
                    trading_profile: None,
                    trading_verified: false,
                    trading_mode: TradingMode::Sandbox.as_str().into(),
                    brokerage_balance: None,
                    available_balance: None,
                    current_balance: None,
                    base_url: self.base_url.clone(),
                    message: format!("Could not load Wealth profile: {e}"),
                    display_name: None,
                }
            }
        }
    }

    /// Live order routing requires a verified trading profile.
    pub async fn resolve_trading_mode(&self, settings: &AppSettings) -> TradingMode {
        if !settings.wealth_connected || !self.has_session() {
            return TradingMode::Sandbox;
        }
        let status = self.profile_status(settings).await;
        if status.ok && status.trading_verified {
            TradingMode::Live
        } else {
            TradingMode::Sandbox
        }
    }

    pub async fn get_wallet(&self) -> Result<WealthWallet> {
        let profile = self.get_profile_raw().await?;
        let wallet = profile
            .get("wallet")
            .or_else(|| profile.pointer("/data/wallet"))
            .ok_or_else(|| anyhow!("Wealth profile missing wallet"))?;
        Ok(WealthWallet {
            brokerage_balance: json_field_f64(wallet, "brokerage_balance").unwrap_or(0.0),
            available_balance: json_field_f64(wallet, "available_balance").unwrap_or(0.0),
            current_balance: json_field_f64(wallet, "current_balance").unwrap_or(0.0),
        })
    }

    pub async fn get_portfolio(&self) -> Result<WealthPortfolioSnapshot> {
        let raw = self
            .request_json(
                reqwest::Method::GET,
                "/portfolio?with[]=stocks.stock",
                None,
            )
            .await?;
        let mut snap = parse_portfolio_snapshot(&raw);
        self.fill_missing_symbols(&mut snap).await;
        Ok(snap)
    }

    async fn fill_missing_symbols(&self, snap: &mut WealthPortfolioSnapshot) {
        for h in &mut snap.holdings {
            if crate::ngx::is_valid_ticker(&h.symbol) {
                continue;
            }
            if h.stock_id <= 0 {
                continue;
            }
            let Ok(raw) = self
                .request_json(
                    reqwest::Method::GET,
                    &format!("/stocks/{}", h.stock_id),
                    None,
                )
                .await
            else {
                continue;
            };
            let root = raw.get("data").unwrap_or(&raw);
            if let Some(sym) = json_symbol(Some(root)) {
                h.symbol = sym;
            }
            if h.price <= 0.0 {
                h.price = json_field_f64(root, "price")
                    .or_else(|| json_field_f64(root, "close_price"))
                    .unwrap_or(0.0);
            }
            if h.buy_price.is_none() {
                h.buy_price = root
                    .pointer("/portfolio/buy_price")
                    .and_then(json_f64);
            }
        }
        snap.holdings.retain(|h| {
            h.quantity > 0.0 && crate::ngx::is_valid_ticker(&h.symbol)
        });
    }

    pub async fn market_is_open(&self) -> Result<bool> {
        let raw = self
            .request_json(reqwest::Method::GET, "/market-status", None)
            .await?;
        Ok(raw
            .get("is_opened")
            .or_else(|| raw.pointer("/data/is_opened"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    pub async fn resolve_stock_id(&self, symbol: &str) -> Result<(i64, f64)> {
        let q = urlencoding_encode(symbol);
        let raw = self
            .request_json(
                reqwest::Method::GET,
                &format!("/stocks?query={q}&per_page=20"),
                None,
            )
            .await?;
        let items = raw
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_else(|| {
                raw.as_array()
                    .cloned()
                    .unwrap_or_default()
            });
        let target = symbol.to_uppercase();
        let stock = items
            .into_iter()
            .find(|s| {
                s.get("symbol")
                    .and_then(|v| v.as_str())
                    .map(|sym| sym.eq_ignore_ascii_case(&target))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow!("Wealth stock not found for {symbol}"))?;
        let id = stock
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow!("stock missing id"))?;
        let price = stock
            .get("price")
            .or_else(|| stock.get("close_price"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        Ok((id, price))
    }

    pub async fn calculate_fee(
        &self,
        stock_id: i64,
        quantity: f64,
        limit_price: f64,
    ) -> Result<WealthFee> {
        let raw = self
            .request_json(
                reqwest::Method::POST,
                "/stocks/fee",
                Some(serde_json::json!({
                    "stock_id": stock_id,
                    "quantity": quantity as i64,
                    "order_type": "market_price",
                    "limit_price": limit_price,
                })),
            )
            .await?;
        Ok(WealthFee {
            accurate_fee: raw
                .get("accurate_fee")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            rounded_fee: raw
                .get("rounded_fee")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
        })
    }

    pub async fn place_market_order(
        &self,
        stock_id: i64,
        side: &str,
        quantity: f64,
        client_order_id: Option<&str>,
    ) -> Result<WealthOrder> {
        let tx = if side.eq_ignore_ascii_case("SELL") {
            "sell"
        } else {
            "buy"
        };
        let mut body = serde_json::json!({
            "stock_id": stock_id,
            "transaction_type": tx,
            "quantity": quantity as i64,
            "type": "market_price",
            "with": ["stock"],
        });
        if let Some(cid) = client_order_id {
            body["client_order_id"] = serde_json::json!(cid);
        }
        let raw = self
            .request_json(reqwest::Method::POST, "/orders", Some(body))
            .await?;
        parse_order(&raw)
    }

    pub async fn get_order(&self, order_id: i64) -> Result<WealthOrder> {
        let raw = self
            .request_json(
                reqwest::Method::GET,
                &format!("/portfolio/orders/{order_id}?with[]=stock"),
                None,
            )
            .await?;
        let root = raw.get("data").unwrap_or(&raw);
        parse_order(root)
    }

    pub async fn place_and_await_fill(
        &self,
        stock_id: i64,
        side: &str,
        quantity: f64,
        client_order_id: Option<&str>,
    ) -> Result<WealthOrder> {
        let mut order = self
            .place_market_order(stock_id, side, quantity, client_order_id)
            .await?;
        if order.status == "executed" || order.status == "rejected" {
            return Ok(order);
        }
        for _ in 0..ORDER_POLL_MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(ORDER_POLL_INTERVAL_MS)).await;
            order = self.get_order(order.id).await?;
            if order.status == "executed" || order.status == "rejected" {
                return Ok(order);
            }
        }
        Ok(order)
    }
}

fn unwrap_auth_body(body: &Value) -> Value {
    // Common API wrappers: { data: { token, ... } } or { data: { auth: {...} } }
    if let Some(data) = body.get("data") {
        if data.get("token").is_some()
            || data.get("access_token").is_some()
            || data.get("accessToken").is_some()
            || data.get("ttl").is_some()
            || data.get("user").is_some()
        {
            return data.clone();
        }
        if let Some(auth) = data.get("auth") {
            return auth.clone();
        }
    }
    if let Some(auth) = body.get("auth") {
        return auth.clone();
    }
    if let Some(result) = body.get("result") {
        return result.clone();
    }
    body.clone()
}

fn extract_ttl_minutes(body: &Value) -> Option<i64> {
    let raw = body
        .get("access_token_ttl")
        .or_else(|| body.get("ttl"))
        .or_else(|| body.get("expires_in"))
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_i64().map(|i| i as f64))
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })?;
    // Heuristic: values >= 120 are likely seconds.
    if body.get("expires_in").is_some() || raw >= 120.0 {
        Some((raw / 60.0).round() as i64)
    } else {
        Some(raw as i64)
    }
}

fn find_trading_profile(value: &Value) -> Option<String> {
    if let Some(s) = value
        .pointer("/profile_verification_state/trading_profile")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    if let Some(s) = value
        .pointer("/user/profile_verification_state/trading_profile")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    if let Some(s) = value
        .get("trading_profile")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    // Depth-limited walk for oddly nested payloads.
    fn walk(v: &Value, depth: usize) -> Option<String> {
        if depth == 0 {
            return None;
        }
        match v {
            Value::Object(map) => {
                if let Some(s) = map.get("trading_profile").and_then(|x| x.as_str()) {
                    return Some(s.to_string());
                }
                for child in map.values() {
                    if let Some(found) = walk(child, depth - 1) {
                        return Some(found);
                    }
                }
                None
            }
            Value::Array(arr) => {
                for child in arr {
                    if let Some(found) = walk(child, depth - 1) {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }
    walk(value, 5)
}

fn extract_token(body: &Value) -> Option<String> {
    const KEYS: &[&str] = &[
        "token",
        "access_token",
        "accessToken",
        "jwt",
        "bearer_token",
        "bearerToken",
        "auth_token",
        "authToken",
    ];
    for key in KEYS {
        if let Some(s) = body.get(*key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            return Some(s.to_string());
        }
    }
    // Nested authorization object
    if let Some(auth) = body.get("authorization").or_else(|| body.get("Authorization")) {
        return extract_token(auth);
    }
    None
}

fn extract_temp_token(body: &Value) -> Option<String> {
    extract_token(body).or_else(|| {
        body.get("temp_token")
            .or_else(|| body.get("temporary_token"))
            .or_else(|| body.get("two_factor_token"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

fn needs_2fa(body: &Value) -> bool {
    body.get("requires_2fa")
        .or_else(|| body.get("two_factor_required"))
        .or_else(|| body.get("needs_2fa"))
        .or_else(|| body.get("require_2fa"))
        .or_else(|| body.get("twoFactorRequired"))
        .or_else(|| body.get("is_2fa_required"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || body
            .get("two_factor")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || body
            .get("message")
            .and_then(|v| v.as_str())
            .map(|m| {
                let lower = m.to_ascii_lowercase();
                lower.contains("2fa")
                    || lower.contains("two-factor")
                    || lower.contains("two factor")
                    || lower.contains("authentication code")
            })
            .unwrap_or(false)
}

fn looks_like_2fa_challenge(body: &Value) -> bool {
    needs_2fa(body)
        || body.get("user").is_some()
            && extract_token(body).is_none()
            && (body.get("qr_code").is_some()
                || body.get("otp_required").is_some()
                || body.get("challenge").is_some())
}

fn extract_error_message(body: &Value) -> Option<String> {
    body.get("message")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            body.get("error")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .or_else(|| {
            body.pointer("/errors/0")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .or_else(|| {
            // Laravel-style { errors: { email: ["..."] } }
            body.get("errors").and_then(|errs| {
                errs.as_object().and_then(|map| {
                    map.values().next().and_then(|v| {
                        v.as_array()
                            .and_then(|a| a.first())
                            .and_then(|x| x.as_str())
                            .map(str::to_string)
                    })
                })
            })
        })
}

fn json_keys_preview(body: &Value) -> String {
    match body {
        Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(|s| s.as_str()).collect();
            keys.sort_unstable();
            keys.join(",")
        }
        Value::Array(arr) => format!("[{}]", arr.len()),
        _ => body.to_string().chars().take(80).collect(),
    }
}

fn parse_order(raw: &Value) -> Result<WealthOrder> {
    Ok(WealthOrder {
        id: raw
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow!("order missing id"))?,
        stock_id: raw.get("stock_id").and_then(|v| v.as_i64()).unwrap_or(0),
        transaction_type: raw
            .get("transaction_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        status: raw
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("open")
            .into(),
        quantity: raw.get("quantity").and_then(|v| v.as_f64()).unwrap_or(0.0),
        quote_price: raw.get("quote_price").and_then(|v| v.as_f64()),
        unit_price: raw.get("unit_price").and_then(|v| v.as_f64()),
        estimated_amount: raw.get("estimated_amount").and_then(|v| v.as_f64()),
        rejection_reason: raw
            .get("rejection_reason")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Persist a live broker order for local UI history.
pub fn insert_broker_order(
    conn: &rusqlite::Connection,
    signal_id: Option<&str>,
    symbol: &str,
    side: &str,
    quantity: f64,
    order: &WealthOrder,
    fill_price: Option<f64>,
    fee: Option<f64>,
) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO broker_orders (
            id, signal_id, symbol, side, quantity, external_order_id, stock_id,
            status, fill_price, fee, rejection_reason, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, datetime('now'), datetime('now'))",
        rusqlite::params![
            id,
            signal_id,
            symbol,
            side,
            quantity,
            order.id,
            order.stock_id,
            order.status,
            fill_price.or(order.unit_price).or(order.quote_price),
            fee,
            order.rejection_reason,
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CachedWealthBook {
    pub brokerage_balance: f64,
    pub stock_value: f64,
    pub profit: f64,
    pub synced_at: String,
    pub holdings: Vec<WealthHolding>,
}

impl CachedWealthBook {
    pub fn from_snapshot(snap: &WealthPortfolioSnapshot, cash: f64, synced_at: String) -> Self {
        let stock_value = if snap.stock_value > 0.0 {
            snap.stock_value
        } else {
            snap.holdings.iter().map(|h| h.current_value).sum()
        };
        Self {
            brokerage_balance: cash,
            stock_value,
            profit: snap.profit,
            synced_at,
            holdings: snap.holdings.clone(),
        }
    }

    pub fn market_value(&self) -> f64 {
        let from_lots: f64 = self
            .holdings
            .iter()
            .filter(|h| h.quantity > 0.0)
            .map(|h| h.current_value)
            .sum();
        if self.holdings.iter().any(|h| h.quantity > 0.0) {
            from_lots
        } else {
            self.stock_value
        }
    }
}

pub struct WealthSyncService;

impl WealthSyncService {
    pub async fn refresh(
        db: &crate::db::Database,
        client: &WealthClient,
    ) -> Result<CachedWealthBook> {
        let previous = Self::load(db).ok().flatten();
        let snap = client.get_portfolio().await?;
        let wallet = client.get_wallet().await.ok();
        let cash = pick_cash(
            wallet.as_ref(),
            previous.as_ref().map(|b| b.brokerage_balance),
        );

        if !snap.stocks_present && snap.holdings.is_empty() {
            if let Some(prev) = previous.filter(|p| !p.holdings.is_empty() || p.stock_value > 0.0) {
                tracing::warn!(
                    target: "wealth",
                    "refusing to overwrite Wealth cache; portfolio response missing stocks"
                );
                return Ok(CachedWealthBook {
                    brokerage_balance: cash,
                    ..prev
                });
            }
        }

        db.with_conn(|conn| persist_snapshot(conn, &snap, cash))?;
        db.with_conn(load_snapshot)?
            .ok_or_else(|| anyhow!("Wealth cache empty after persist"))
    }

    pub fn load(db: &crate::db::Database) -> Result<Option<CachedWealthBook>> {
        db.with_conn(load_snapshot)
    }
}

pub fn persist_snapshot(
    conn: &rusqlite::Connection,
    snap: &WealthPortfolioSnapshot,
    cash: f64,
) -> Result<()> {
    conn.execute("BEGIN IMMEDIATE", [])?;
    let result = (|| {
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let stock_value = if snap.stock_value > 0.0 {
            snap.stock_value
        } else {
            snap.holdings.iter().map(|h| h.current_value).sum()
        };
        conn.execute("DELETE FROM wealth_positions", [])?;
        for h in &snap.holdings {
            if h.quantity <= 0.0 || !crate::ngx::is_valid_ticker(&h.symbol) {
                continue;
            }
            conn.execute(
                "INSERT INTO instruments (symbol, name, sector, is_active)
                 VALUES (?1, ?1, 'Unknown', 1)
                 ON CONFLICT(symbol) DO UPDATE SET is_active = 1",
                [&h.symbol],
            )?;
            conn.execute(
                "INSERT INTO wealth_positions (symbol, stock_id, quantity, avg_cost, last_price, current_value, synced_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    h.symbol,
                    h.stock_id,
                    h.quantity,
                    h.buy_price.unwrap_or(h.price),
                    h.price,
                    h.current_value,
                    now,
                ],
            )?;
        }
        conn.execute(
            "INSERT INTO wealth_account (id, brokerage_balance, stock_value, profit, synced_at)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               brokerage_balance = excluded.brokerage_balance,
               stock_value = excluded.stock_value,
               profit = excluded.profit,
               synced_at = excluded.synced_at",
            rusqlite::params![cash, stock_value, snap.profit, now],
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute("COMMIT", [])?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(e)
        }
    }
}

pub fn load_snapshot(conn: &rusqlite::Connection) -> Result<Option<CachedWealthBook>> {
    let account: Option<(f64, f64, f64, String)> = conn
        .query_row(
            "SELECT brokerage_balance, stock_value, profit, synced_at FROM wealth_account WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((brokerage_balance, stock_value, profit, synced_at)) = account else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT symbol, stock_id, quantity, avg_cost, last_price, current_value
         FROM wealth_positions WHERE quantity > 0 ORDER BY symbol",
    )?;
    let holdings = stmt
        .query_map([], |row| {
            Ok(WealthHolding {
                symbol: row.get(0)?,
                stock_id: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                quantity: row.get(2)?,
                buy_price: Some(row.get(3)?),
                price: row.get(4)?,
                current_value: row.get(5)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(Some(CachedWealthBook {
        brokerage_balance,
        stock_value,
        profit,
        synced_at,
        holdings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_f64_parses_numbers_and_numeric_strings() {
        assert_eq!(json_f64(&json!(125000.5)), Some(125000.5));
        assert_eq!(json_f64(&json!(12)), Some(12.0));
        assert_eq!(json_f64(&json!("1,250,000.25")), Some(1_250_000.25));
        assert_eq!(json_f64(&json!("")), None);
        assert_eq!(json_f64(&json!(null)), None);
    }

    #[test]
    fn pick_cash_uses_brokerage_only() {
        let wallet = WealthWallet {
            brokerage_balance: 80_000.0,
            available_balance: 120_000.0,
            current_balance: 150_000.0,
        };
        assert_eq!(pick_cash(Some(&wallet), Some(500_000.0)), 80_000.0);
        assert_eq!(pick_cash(None, Some(80_000.0)), 80_000.0);
        assert_eq!(
            pick_cash(
                Some(&WealthWallet {
                    brokerage_balance: 0.0,
                    available_balance: 12_000.0,
                    current_balance: 50_000.0,
                }),
                Some(1.0)
            ),
            0.0
        );
    }

    #[test]
    fn parse_portfolio_stocks_ignores_funds() {
        let raw = json!({
            "id": 1,
            "balance": 500000.00,
            "profit": 25000.00,
            "stock_value": 300000.00,
            "fund_value": 200000.00,
            "stocks": [{
                "id": 1,
                "stock_id": 42,
                "quantity": 100,
                "current_value": 150000.00,
                "value_change": 5000.00,
                "stock": {
                    "id": 42,
                    "symbol": "DANGCEM",
                    "close_price": 1500.00,
                    "price": 1500.00
                }
            }],
            "mutual_funds": [{ "id": 9, "current_value": 200000.00 }]
        });
        let snap = parse_portfolio_snapshot(&raw);
        assert!(snap.stocks_present);
        assert_eq!(snap.holdings.len(), 1);
        assert_eq!(snap.holdings[0].symbol, "DANGCEM");
        assert_eq!(snap.holdings[0].quantity, 100.0);
        assert_eq!(snap.holdings[0].stock_id, 42);
        assert_eq!(snap.stock_value, 300000.0);
        assert_eq!(snap.profit, 5000.0);
        assert_eq!(snap.balance, 500000.0);
    }

    #[test]
    fn parse_portfolio_flat_lots_and_string_ids() {
        let raw = json!({
            "data": {
                "stock_value": "0",
                "stocks": [{
                    "symbol": "gtco",
                    "stock_id": "7",
                    "quantity": "50",
                    "price": "45.5",
                    "current_value": "2275"
                }]
            }
        });
        let snap = parse_portfolio_snapshot(&raw);
        assert_eq!(snap.holdings.len(), 1);
        assert_eq!(snap.holdings[0].symbol, "GTCO");
        assert_eq!(snap.holdings[0].stock_id, 7);
        assert_eq!(snap.holdings[0].quantity, 50.0);
        assert_eq!(snap.stock_value, 2275.0);
    }

    #[test]
    fn parse_portfolio_empty_stocks_is_real_empty_book() {
        let raw = json!({
            "balance": 1000,
            "stock_value": 0,
            "fund_value": 8000,
            "stocks": [],
            "mutual_funds": [{ "current_value": 8000 }]
        });
        let snap = parse_portfolio_snapshot(&raw);
        assert!(snap.stocks_present);
        assert!(snap.holdings.is_empty());
        assert_eq!(snap.stock_value, 0.0);
    }

    #[test]
    fn parse_portfolio_lots_without_nested_symbol() {
        let raw = json!({
            "stock_value": 8070.9,
            "stocks": [{
                "id": 1,
                "stock_id": 42,
                "quantity": 10,
                "current_value": 1500,
                "stock": {
                    "id": 42,
                    "close_price": 150,
                    "company_name": "Dangote Cement Plc",
                    "icon_url": "https://example"
                }
            }]
        });
        let snap = parse_portfolio_snapshot(&raw);
        assert_eq!(snap.holdings.len(), 1);
        assert_eq!(snap.holdings[0].stock_id, 42);
        assert_eq!(snap.holdings[0].quantity, 10.0);
        assert_eq!(snap.holdings[0].price, 150.0);
        assert!(snap.holdings[0].symbol.is_empty());
    }

    #[test]
    fn market_value_sums_lot_current_value_when_holdings_exist() {
        let book = CachedWealthBook {
            brokerage_balance: 1501.72,
            stock_value: 8070.9,
            profit: 0.0,
            synced_at: "2026-08-14T15:00:00Z".into(),
            holdings: vec![WealthHolding {
                stock_id: 1,
                symbol: "CWG".into(),
                quantity: 59.0,
                current_value: 1262.6,
                buy_price: Some(21.54),
                price: 21.4,
            }],
        };
        assert_eq!(book.market_value(), 1262.6);
    }
}
