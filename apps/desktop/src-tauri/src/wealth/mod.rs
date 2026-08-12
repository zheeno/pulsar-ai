//! Coronation Wealth App REST client for live trading.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
            http: Client::new(),
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
                let err_body = retry_res.text().await.unwrap_or_default();
                tracing::warn!(target: "wealth", %url, %status, body = %err_body.chars().take(200).collect::<String>(), "wealth retry failed");
                return Err(anyhow!("Wealth API error {status}: {err_body}"));
            }
            return retry_res.json().await.context("parse wealth retry");
        }
        if !res.status().is_success() {
            let status = res.status();
            let err_body = res.text().await.unwrap_or_default();
            tracing::warn!(target: "wealth", %url, %status, body = %err_body.chars().take(200).collect::<String>(), "wealth API error");
            return Err(anyhow!("Wealth API error {status}: {err_body}"));
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
                    .and_then(|w| w.get("brokerage_balance"))
                    .and_then(|v| v.as_f64());
                let available = wallet
                    .as_ref()
                    .and_then(|w| w.get("available_balance"))
                    .and_then(|v| v.as_f64());
                let current = wallet
                    .as_ref()
                    .and_then(|w| w.get("current_balance"))
                    .and_then(|v| v.as_f64());
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
            brokerage_balance: wallet
                .get("brokerage_balance")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            available_balance: wallet
                .get("available_balance")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            current_balance: wallet
                .get("current_balance")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
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
        let root = if raw.get("stocks").is_some() {
            &raw
        } else {
            raw.get("data").unwrap_or(&raw)
        };
        let holdings = root
            .get("stocks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                let stock = row.get("stock")?;
                let symbol = stock.get("symbol")?.as_str()?.to_uppercase();
                let stock_id = stock
                    .get("id")
                    .or_else(|| row.get("stock_id"))
                    .and_then(|v| v.as_i64())?;
                let quantity = row.get("quantity").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let price = stock
                    .get("price")
                    .or_else(|| stock.get("close_price"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let current_value = row
                    .get("current_value")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(quantity * price);
                let buy_price = row.get("buy_price").and_then(|v| v.as_f64());
                Some(WealthHolding {
                    stock_id,
                    symbol,
                    quantity,
                    current_value,
                    buy_price,
                    price,
                })
            })
            .collect();

        Ok(WealthPortfolioSnapshot {
            balance: root.get("balance").and_then(|v| v.as_f64()).unwrap_or(0.0),
            profit: root.get("profit").and_then(|v| v.as_f64()).unwrap_or(0.0),
            stock_value: root
                .get("stock_value")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            holdings,
        })
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
    ) -> Result<WealthOrder> {
        let tx = if side.eq_ignore_ascii_case("SELL") {
            "sell"
        } else {
            "buy"
        };
        let raw = self
            .request_json(
                reqwest::Method::POST,
                "/orders",
                Some(serde_json::json!({
                    "stock_id": stock_id,
                    "transaction_type": tx,
                    "quantity": quantity as i64,
                    "type": "market_price",
                    "with": ["stock"],
                })),
            )
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
    ) -> Result<WealthOrder> {
        let mut order = self.place_market_order(stock_id, side, quantity).await?;
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
