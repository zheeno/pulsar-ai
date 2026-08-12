use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::rate_limit::RateLimiter;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Session,
    ApiKey,
    Mock,
}

struct SessionTokens {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
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
        let auth_mode = if settings.pulse_supabase_url.is_some()
            && settings.pulse_supabase_anon_key.is_some()
            && settings.pulse_email.is_some()
            && password.is_some()
        {
            AuthMode::Session
        } else if api_key.as_ref().is_some_and(|k| !k.is_empty()) {
            AuthMode::ApiKey
        } else {
            AuthMode::Mock
        };

        Self {
            http: Client::new(),
            base_url: settings.pulse_base_url.clone(),
            auth_mode,
            api_key,
            supabase_url: settings.pulse_supabase_url.clone(),
            anon_key: settings.pulse_supabase_anon_key.clone(),
            email: settings.pulse_email.clone(),
            password,
            tokens: Arc::new(Mutex::new(None)),
        }
    }

    pub fn auth_mode(&self) -> AuthMode {
        self.auth_mode
    }

    pub async fn test_login(&self) -> Result<()> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(());
        }
        let _ = self.get_access_token().await?;
        Ok(())
    }

    pub async fn get_market_status(&self, conn: &rusqlite::Connection) -> Result<NgxMarketStatus> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(NgxMarketStatus {
                status: "Open".into(),
                is_open: true,
            });
        }
        let raw = self.fetch_raw(conn, "/ngxdata/market-status", "market-status").await?;
        let data = unwrap(raw);
        Ok(NgxMarketStatus {
            status: data.get("status").and_then(|v| v.as_str()).unwrap_or("Unknown").into(),
            is_open: data.get("is_open").and_then(|v| v.as_bool()).unwrap_or(false),
        })
    }

    pub async fn get_stocks(&self, conn: &rusqlite::Connection) -> Result<Vec<NgxStock>> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(mock_stocks());
        }
        let raw = self.fetch_raw(conn, "/ngxdata/stocks", "stocks").await?;
        Ok(normalize_stocks(raw))
    }

    pub async fn get_market(&self, conn: &rusqlite::Connection) -> Result<NgxMarketOverview> {
        if self.auth_mode == AuthMode::Mock {
            return Ok(NgxMarketOverview {
                asi: Some(AsiData {
                    value: 98000.0,
                    change_percent: Some(0.5),
                }),
                market_cap: None,
            });
        }
        let raw = self.fetch_raw(conn, "/ngxdata/market", "market").await?;
        Ok(normalize_market(raw))
    }

    pub async fn get_indices(&self, conn: &rusqlite::Connection) -> Result<Vec<NgxIndex>> {
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
        let raw = self.fetch_raw(conn, "/ngxdata/indices", "indices").await?;
        Ok(normalize_indices(raw))
    }

    pub async fn get_symbol_price(
        &self,
        conn: &rusqlite::Connection,
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
        let raw = self.fetch_raw(conn, &path, &format!("prices/{symbol}")).await?;
        Ok(normalize_prices(raw))
    }

    async fn fetch_raw(&self, conn: &rusqlite::Connection, path: &str, endpoint: &str) -> Result<serde_json::Value> {
        if self.auth_mode == AuthMode::ApiKey && !RateLimiter::can_make_request(conn)? {
            return Err(anyhow!("NGX Pulse rate limit exceeded"));
        }

        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let token = self.get_access_token().await?;
        let mut res = self
            .http
            .get(&url)
            .headers(request_headers(&token))
            .send()
            .await
            .context("ngx pulse request")?;

        if res.status().as_u16() == 401 && self.auth_mode == AuthMode::Session {
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

        let auth_str = match self.auth_mode {
            AuthMode::Session => "session",
            AuthMode::ApiKey => "api_key",
            AuthMode::Mock => "mock",
        };
        RateLimiter::record_request(conn, endpoint, auth_str)?;

        if !res.status().is_success() {
            return Err(anyhow!("NGX Pulse error: {}", res.status()));
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
        let supabase_url = self.supabase_url.as_ref().context("supabase url")?;
        let anon_key = self.anon_key.as_ref().context("anon key")?;
        let email = self.email.as_ref().context("email")?;
        let password = self.password.as_ref().context("password")?;

        let url = format!("{}/auth/v1/token?grant_type=password", supabase_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "email": email,
            "password": password,
            "gotrue_meta_security": {}
        });

        let res = self
            .http
            .post(&url)
            .headers(supabase_headers(anon_key))
            .json(&body)
            .send()
            .await
            .context("pulse login")?;

        let tokens = parse_auth_response(res).await?;
        *self.tokens.lock().unwrap() = Some(tokens);
        Ok(())
    }

    async fn refresh_session(&self, refresh_token: &str) -> Result<()> {
        let supabase_url = self.supabase_url.as_ref().context("supabase url")?;
        let anon_key = self.anon_key.as_ref().context("anon key")?;
        let url = format!("{}/auth/v1/token?grant_type=refresh_token", supabase_url.trim_end_matches('/'));
        let body = serde_json::json!({ "refresh_token": refresh_token });

        let res = self
            .http
            .post(&url)
            .headers(supabase_headers(anon_key))
            .json(&body)
            .send()
            .await
            .context("pulse refresh")?;

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

fn normalize_stocks(payload: serde_json::Value) -> Vec<NgxStock> {
    let root = payload.clone();
    let data = unwrap(payload);
    let stocks = data.get("stocks").or(root.get("stocks"));
    let Some(arr) = stocks.and_then(|v| v.as_array()) else {
        return vec![];
    };
    arr.iter()
        .filter_map(|stock| {
            Some(NgxStock {
                symbol: stock.get("symbol")?.as_str()?.into(),
                name: stock.get("name").and_then(|v| v.as_str()).map(String::from),
                price: stock.get("current_price").or(stock.get("price"))?.as_f64()?,
                change_percent: stock
                    .get("change_percent")
                    .or(stock.get("official_change_percent"))
                    .and_then(|v| v.as_f64()),
                volume: stock.get("volume").and_then(|v| v.as_i64()),
                market_cap: stock.get("market_cap").and_then(|v| v.as_f64()),
                pe_ratio: stock.get("pe_ratio").and_then(|v| v.as_f64()),
                sector: stock.get("sector").and_then(|v| v.as_str()).map(String::from),
            })
        })
        .collect()
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
                price: row.get("close_price").or(row.get("price"))?.as_f64()?,
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
