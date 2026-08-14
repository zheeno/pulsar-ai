use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http_client::http_client;
use crate::secrets::{
    delete_secret, get_secret, set_secret, SECRET_BAMBOO_NGN_WALLET_ID, SECRET_BAMBOO_PASSWORD,
    SECRET_BAMBOO_TOKEN, SECRET_BAMBOO_TOKEN_EXPIRES, SECRET_BAMBOO_TRANSACTION_PIN,
    SECRET_BAMBOO_USER_ID,
};
use crate::settings::AppSettings;
use crate::wealth::{
    CachedWealthBook, TradingMode, WealthHolding, WealthPortfolioSnapshot, WealthProfileStatus,
    WealthWallet,
};

pub const BAMBOO_BASE_URL: &str = "https://api.investbamboo.com";

const ORDER_POLL_INTERVAL_MS: u64 = 2_000;
const ORDER_POLL_MAX_ATTEMPTS: u32 = 30;

struct SessionToken {
    access_token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BambooLoginResult {
    pub ok: bool,
    pub connected: bool,
    pub message: String,
    pub phone: Option<String>,
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct BambooFeeQuote {
    pub fee: f64,
    pub quantity: f64,
    pub price_per_share: f64,
    pub total_price: f64,
    pub available_quantity: f64,
    pub symbol: String,
    pub side: String,
    pub price: f64,
    pub order_price: Option<f64>,
    pub order_value: Option<String>,
}

pub struct BambooClient {
    http: Client,
    base_url: String,
    phone: Option<String>,
    password: Option<String>,
    tokens: Arc<Mutex<Option<SessionToken>>>,
}

impl BambooClient {
    pub fn from_settings(settings: &AppSettings) -> Self {
        let token = get_secret(SECRET_BAMBOO_TOKEN).ok().flatten();
        let expires = get_secret(SECRET_BAMBOO_TOKEN_EXPIRES)
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
            base_url: BAMBOO_BASE_URL.to_string(),
            phone: settings.bamboo_phone.clone(),
            password: get_secret(SECRET_BAMBOO_PASSWORD).ok().flatten(),
            tokens: Arc::new(Mutex::new(session)),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn has_session(&self) -> bool {
        self.tokens.lock().unwrap().is_some()
            || get_secret(SECRET_BAMBOO_TOKEN)
                .ok()
                .flatten()
                .is_some_and(|t| !t.is_empty())
    }

    pub fn clear_local_secrets() {
        let _ = delete_secret(SECRET_BAMBOO_TOKEN);
        let _ = delete_secret(SECRET_BAMBOO_TOKEN_EXPIRES);
        let _ = delete_secret(SECRET_BAMBOO_PASSWORD);
        let _ = delete_secret(SECRET_BAMBOO_USER_ID);
        let _ = delete_secret(SECRET_BAMBOO_NGN_WALLET_ID);
        let _ = delete_secret(SECRET_BAMBOO_TRANSACTION_PIN);
    }

    pub async fn login(
        &self,
        phone: &str,
        password: &str,
        transaction_pin: Option<&str>,
    ) -> Result<BambooLoginResult> {
        match self.login_inner(phone, password).await {
            Ok(()) => {
                if let Some(pin) = transaction_pin.map(str::trim).filter(|s| !s.is_empty()) {
                    let _ = set_secret(SECRET_BAMBOO_TRANSACTION_PIN, pin);
                }
                Ok(BambooLoginResult {
                    ok: true,
                    connected: true,
                    message: "Bamboo account connected.".into(),
                    phone: Some(phone.to_string()),
                    base_url: self.base_url.clone(),
                })
            }
            Err(e) => Ok(BambooLoginResult {
                ok: false,
                connected: false,
                message: e.to_string(),
                phone: Some(phone.to_string()),
                base_url: self.base_url.clone(),
            }),
        }
    }

    async fn login_inner(&self, phone: &str, password: &str) -> Result<()> {
        let url = format!("{}/api/login", self.base_url.trim_end_matches('/'));
        tracing::info!(target: "bamboo", %url, "bamboo login request");
        let res = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "phone_number": phone,
                "password": password,
            }))
            .send()
            .await
            .context("bamboo login request")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| extract_error_message(&v))
                .unwrap_or_else(|| format!("Bamboo login failed ({status})"));
            return Err(anyhow!(msg));
        }
        let token = extract_login_token(&text).ok_or_else(|| anyhow!("Bamboo login missing token"))?;
        self.persist_token(&token, phone, Some(password))?;
        Ok(())
    }

    fn persist_token(&self, token: &str, phone: &str, password: Option<&str>) -> Result<()> {
        let claims = decode_jwt_payload(token).unwrap_or(Value::Null);
        let exp = claims
            .get("exp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| Utc::now().timestamp() + 3600);
        let expires_at = Utc
            .timestamp_opt(exp, 0)
            .single()
            .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));
        if let Some(sub) = claims.get("sub").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            let _ = set_secret(SECRET_BAMBOO_USER_ID, sub);
        } else if let Some(sub) = claims.get("sub").and_then(|v| v.as_i64()) {
            let _ = set_secret(SECRET_BAMBOO_USER_ID, &sub.to_string());
        }
        set_secret(SECRET_BAMBOO_TOKEN, token)?;
        set_secret(SECRET_BAMBOO_TOKEN_EXPIRES, &expires_at.to_rfc3339())?;
        if let Some(pw) = password {
            set_secret(SECRET_BAMBOO_PASSWORD, pw)?;
        }
        *self.tokens.lock().unwrap() = Some(SessionToken {
            access_token: token.to_string(),
            expires_at,
        });
        tracing::info!(
            target: "bamboo",
            phone = %phone,
            expires_at = %expires_at.to_rfc3339(),
            "bamboo auth persisted"
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
        if let Ok(Some(stored)) = get_secret(SECRET_BAMBOO_TOKEN) {
            if !stored.is_empty() {
                let expires = get_secret(SECRET_BAMBOO_TOKEN_EXPIRES)
                    .ok()
                    .flatten()
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc));
                if let Some(exp) = expires {
                    if exp > Utc::now() + chrono::Duration::seconds(30) {
                        *self.tokens.lock().unwrap() = Some(SessionToken {
                            access_token: stored.clone(),
                            expires_at: exp,
                        });
                        return Ok(stored);
                    }
                }
            }
        }
        self.relogin().await?;
        self.tokens
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.access_token.clone())
            .ok_or_else(|| anyhow!("Bamboo session expired — reconnect in Settings"))
    }

    async fn relogin(&self) -> Result<()> {
        let phone = self
            .phone
            .clone()
            .ok_or_else(|| anyhow!("Bamboo session expired — reconnect in Settings"))?;
        let password = self
            .password
            .clone()
            .or_else(|| get_secret(SECRET_BAMBOO_PASSWORD).ok().flatten())
            .ok_or_else(|| anyhow!("Bamboo session expired — reconnect in Settings"))?;
        self.login_inner(&phone, &password).await
    }

    async fn request_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        extra_headers: &[(&str, &str)],
    ) -> Result<Value> {
        let token = self.access_token().await?;
        let res = self
            .send_authed(&token, method.clone(), path, body.as_ref(), extra_headers)
            .await?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if status.as_u16() == 401 || is_auth_error_body(&text) {
            tracing::warn!(target: "bamboo", path, "unauthenticated — re-login");
            self.relogin().await?;
            let token = self.access_token().await?;
            let retry = self
                .send_authed(&token, method, path, body.as_ref(), extra_headers)
                .await?;
            let retry_status = retry.status();
            let retry_text = retry.text().await.unwrap_or_default();
            if !retry_status.is_success() {
                tracing::warn!(target: "bamboo", path, %retry_status, "bamboo retry failed");
                return Err(anyhow!(
                    extract_error_message(&serde_json::from_str(&retry_text).unwrap_or(Value::Null))
                        .unwrap_or_else(|| format!("Bamboo API error {retry_status}"))
                ));
            }
            return parse_json_body(&retry_text);
        }
        if !status.is_success() {
            tracing::warn!(target: "bamboo", path, %status, "bamboo API error");
            return Err(anyhow!(
                extract_error_message(&serde_json::from_str(&text).unwrap_or(Value::Null))
                    .unwrap_or_else(|| format!("Bamboo API error {status}"))
            ));
        }
        parse_json_body(&text)
    }

    async fn send_authed(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        extra_headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let mut req = self
            .http
            .request(method, &url)
            .header("Authorization", format!("Bearer {token}"))
            .header("x-subject-type", "standard")
            .header("Accept", "application/json")
            .header("Content-Type", "application/json");
        for (k, v) in extra_headers {
            req = req.header(*k, *v);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        req.send().await.context("bamboo request")
    }

    pub async fn profile_status(&self, settings: &AppSettings) -> WealthProfileStatus {
        let base = WealthProfileStatus {
            ok: false,
            connected: settings.bamboo_connected && self.has_session(),
            email: settings.bamboo_phone.clone(),
            trading_profile: None,
            trading_verified: false,
            trading_mode: TradingMode::Sandbox.as_str().into(),
            brokerage_balance: None,
            available_balance: None,
            current_balance: None,
            base_url: self.base_url.clone(),
            message: "Bamboo account not connected.".into(),
            display_name: None,
        };
        if !settings.bamboo_connected || !self.has_session() {
            return base;
        }

        let profile = match self
            .request_json(reqwest::Method::GET, "/api/profile", None, &[])
            .await
        {
            Ok(v) => v,
            Err(e) => {
                return WealthProfileStatus {
                    connected: true,
                    message: format!("Could not load Bamboo profile: {e}"),
                    ..base
                };
            }
        };
        let root = profile.get("data").cloned().unwrap_or(profile);
        let restricted = root
            .pointer("/account_restriction/restricted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let email = root
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| settings.bamboo_phone.clone());
        let display_name = {
            let first = root.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let last = root.get("surname").and_then(|v| v.as_str()).unwrap_or("");
            let n = format!("{first} {last}").trim().to_string();
            if n.is_empty() {
                None
            } else {
                Some(n)
            }
        };

        let cscs = self
            .request_json(
                reqwest::Method::GET,
                "/api/lsx/ng/cscs/account/status",
                None,
                &[],
            )
            .await
            .ok();
        let ready = cscs
            .as_ref()
            .and_then(|v| {
                v.get("ready_for_trading")
                    .or_else(|| v.pointer("/data/ready_for_trading"))
            })
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let cash = self.ngn_cash().await.ok();

        if restricted {
            return WealthProfileStatus {
                ok: false,
                connected: true,
                email,
                trading_profile: Some("restricted".into()),
                trading_verified: false,
                trading_mode: TradingMode::Sandbox.as_str().into(),
                brokerage_balance: cash,
                available_balance: cash,
                current_balance: cash,
                base_url: self.base_url.clone(),
                message: "Bamboo account is restricted. Live orders are blocked.".into(),
                display_name,
            };
        }

        WealthProfileStatus {
            ok: true,
            connected: true,
            email,
            trading_profile: Some(if ready {
                "verified".into()
            } else {
                "cscs_pending".into()
            }),
            trading_verified: ready,
            trading_mode: if ready {
                "live".into()
            } else {
                TradingMode::Sandbox.as_str().into()
            },
            brokerage_balance: cash,
            available_balance: cash,
            current_balance: cash,
            base_url: self.base_url.clone(),
            message: if ready {
                "Live trader mode — orders use Bamboo NGN cash.".into()
            } else {
                "Bamboo connected. NGX trading requires CSCS ready_for_trading.".into()
            },
            display_name,
        }
    }

    pub async fn resolve_trading_mode(&self, settings: &AppSettings) -> TradingMode {
        if !settings.bamboo_connected || !self.has_session() {
            return TradingMode::Sandbox;
        }
        let status = self.profile_status(settings).await;
        if status.ok && status.trading_verified {
            TradingMode::Live
        } else {
            TradingMode::Sandbox
        }
    }

    async fn wallet_balance_raw(&self) -> Result<Value> {
        self.request_json(reqwest::Method::GET, "/api/wallet_balance", None, &[])
            .await
    }

    pub async fn ngn_cash(&self) -> Result<f64> {
        let raw = self.wallet_balance_raw().await?;
        if let Some(id) = parse_ngn_wallet_id(&raw) {
            let _ = set_secret(SECRET_BAMBOO_NGN_WALLET_ID, &id.to_string());
        }
        parse_ngn_cash(&raw).ok_or_else(|| anyhow!("Bamboo NGN wallet_balance missing"))
    }

    pub async fn get_wallet(&self) -> Result<WealthWallet> {
        let cash = self.ngn_cash().await?;
        Ok(WealthWallet {
            brokerage_balance: cash,
            available_balance: cash,
            current_balance: cash,
        })
    }

    pub async fn get_portfolio(&self) -> Result<WealthPortfolioSnapshot> {
        let raw = self
            .request_json(reqwest::Method::GET, "/api/lsx/ng/my_stocks", None, &[])
            .await?;
        Ok(parse_my_stocks(&raw))
    }

    pub async fn ensure_ngn_wallet_id(&self) -> Result<i64> {
        if let Some(id) = get_secret(SECRET_BAMBOO_NGN_WALLET_ID)
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
        {
            return Ok(id);
        }
        let raw = self.wallet_balance_raw().await?;
        let id = parse_ngn_wallet_id(&raw).ok_or_else(|| anyhow!("Bamboo NGN wallet_id missing"))?;
        let _ = set_secret(SECRET_BAMBOO_NGN_WALLET_ID, &id.to_string());
        Ok(id)
    }

    pub async fn market_is_open(&self) -> Result<bool> {
        let raw = self
            .request_json(
                reqwest::Method::GET,
                "/api/market/open_date?market=NGX",
                None,
                &[],
            )
            .await?;
        Ok(raw
            .pointer("/market_session/core_market")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    pub async fn resolve_quote(&self, symbol: &str) -> Result<f64> {
        let target = symbol.to_uppercase();
        let q = urlencoding_encode(&target);
        let _ = self
            .request_json(
                reqwest::Method::GET,
                &format!("/api/lsx/ng/stocks?query={q}"),
                None,
                &[],
            )
            .await;
        let raw = self
            .request_json(
                reqwest::Method::GET,
                &format!("/api/lsx/ng/stocks/{target}"),
                None,
                &[],
            )
            .await?;
        let root = raw.get("data").unwrap_or(&raw);
        json_f64_field(root, "market_price")
            .or_else(|| json_f64_field(root, "price"))
            .or_else(|| json_f64_field(root, "close_price"))
            .filter(|p| *p > 0.0)
            .ok_or_else(|| anyhow!("Bamboo quote missing for {symbol}"))
    }

    pub async fn calculate_fee(
        &self,
        symbol: &str,
        side: &str,
        quantity: f64,
        price: f64,
    ) -> Result<BambooFeeQuote> {
        let side = if side.eq_ignore_ascii_case("SELL") {
            "SELL"
        } else {
            "BUY"
        };
        let raw = self
            .request_json(
                reqwest::Method::POST,
                "/api/lsx/ng/order/calculate",
                Some(serde_json::json!({
                    "type": "MARKET",
                    "symbol": symbol.to_uppercase(),
                    "side": side,
                    "quantity": quantity,
                    "price": price,
                    "currency": "NGN",
                })),
                &[("currency", "NGN")],
            )
            .await?;
        Ok(parse_fee_quote(&raw, symbol, side, quantity, price))
    }

    pub async fn get_order(&self, order_id: &str) -> Result<crate::broker::BrokerOrder> {
        let encoded = urlencoding_encode(order_id);
        let raw = self
            .request_json(
                reqwest::Method::GET,
                &format!("/api/lsx/ng/order/{encoded}/status"),
                None,
                &[],
            )
            .await?;
        Ok(parse_broker_order(&raw, order_id))
    }

    pub async fn place_and_await_fill(
        &self,
        calc: &BambooFeeQuote,
    ) -> Result<crate::broker::BrokerOrder> {
        if calc.available_quantity <= 0.0 {
            anyhow::bail!("Bamboo available_quantity is 0");
        }
        let cash = self.ngn_cash().await.unwrap_or(0.0);
        if calc.side.eq_ignore_ascii_case("BUY") && calc.total_price > cash {
            anyhow::bail!(
                "Insufficient Bamboo cash (need ₦{:.2}, have ₦{:.2})",
                calc.total_price,
                cash
            );
        }
        let wallet_id = self.ensure_ngn_wallet_id().await?;
        let mut body = serde_json::json!({
            "symbol": calc.symbol.to_uppercase(),
            "side": calc.side,
            "order_type": "MARKET",
            "quantity": calc.quantity,
            "price": calc.price,
            "price_per_share": calc.price_per_share,
            "fee": calc.fee,
            "total_price": calc.total_price,
            "source_wallet_id": wallet_id,
            "currency": "NGN",
            "type": "MARKET",
        });
        if let Some(op) = calc.order_price {
            body["order_price"] = serde_json::json!(op);
        }
        if let Some(v) = &calc.order_value {
            body["order_value"] = Value::String(v.clone());
        } else {
            body["order_value"] = Value::String(format!("{:.2}", calc.total_price));
        }
        if let Some(pin) = get_secret(SECRET_BAMBOO_TRANSACTION_PIN)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
        {
            body["transaction_pin"] = Value::String(pin);
        }

        let placed = self
            .request_json(
                reqwest::Method::POST,
                "/api/lsx/ng/order",
                Some(body),
                &[("currency", "NGN")],
            )
            .await?;
        let order_id = placed
            .get("order_id")
            .or_else(|| placed.get("id"))
            .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string())))
            .ok_or_else(|| anyhow!("Bamboo place missing order_id"))?;

        let mut order = self.get_order(&order_id).await.unwrap_or_else(|_| {
            crate::broker::BrokerOrder {
                id: order_id.clone(),
                numeric_id: None,
                stock_id: None,
                status: "pending".into(),
                quantity: calc.quantity,
                quote_price: Some(calc.price_per_share),
                unit_price: Some(calc.price_per_share),
                rejection_reason: None,
            }
        });
        if order.status == "executed" || order.status == "rejected" {
            return Ok(order);
        }
        for _ in 0..ORDER_POLL_MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(ORDER_POLL_INTERVAL_MS)).await;
            order = self.get_order(&order_id).await?;
            if order.status == "executed" || order.status == "rejected" {
                return Ok(order);
            }
        }
        Ok(order)
    }
}

pub struct BambooSyncService;

impl BambooSyncService {
    pub async fn refresh(
        db: &crate::db::Database,
        client: &BambooClient,
    ) -> Result<CachedWealthBook> {
        let snap = client.get_portfolio().await?;
        let cash = client.ngn_cash().await.unwrap_or(0.0);
        db.with_conn(|conn| persist_snapshot(conn, &snap, cash))?;
        db.with_conn(load_snapshot)?
            .ok_or_else(|| anyhow!("Bamboo cache empty after persist"))
    }

    pub fn load(db: &crate::db::Database) -> Result<Option<CachedWealthBook>> {
        db.with_conn(load_snapshot)
    }
}

fn persist_snapshot(
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
        conn.execute("DELETE FROM bamboo_positions", [])?;
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
                "INSERT INTO bamboo_positions (symbol, stock_id, quantity, avg_cost, last_price, current_value, synced_at)
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
            "INSERT INTO bamboo_account (id, brokerage_balance, stock_value, profit, synced_at)
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

fn load_snapshot(conn: &rusqlite::Connection) -> Result<Option<CachedWealthBook>> {
    let account: Option<(f64, f64, f64, String)> = conn
        .query_row(
            "SELECT brokerage_balance, stock_value, profit, synced_at FROM bamboo_account WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((brokerage_balance, stock_value, profit, synced_at)) = account else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT symbol, stock_id, quantity, avg_cost, last_price, current_value
         FROM bamboo_positions WHERE quantity > 0 ORDER BY symbol",
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

pub fn parse_my_stocks(raw: &Value) -> WealthPortfolioSnapshot {
    let root = raw.get("data").unwrap_or(raw);
    let stocks = root
        .get("stocks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let holdings: Vec<WealthHolding> = stocks.iter().filter_map(parse_lot).collect();
    let stock_value = holdings.iter().map(|h| h.current_value).sum();
    WealthPortfolioSnapshot {
        balance: 0.0,
        profit: json_f64_field(root, "profit").unwrap_or(0.0),
        stock_value,
        holdings,
        stocks_present: true,
    }
}

fn parse_lot(row: &Value) -> Option<WealthHolding> {
    let symbol = row
        .get("symbol")
        .and_then(|v| v.as_str())
        .map(|s| s.to_uppercase())
        .filter(|s| crate::ngx::is_valid_ticker(s))?;
    let quantity = json_f64_field(row, "quantity").unwrap_or(0.0);
    if quantity <= 0.0 {
        return None;
    }
    let price = json_f64_field(row, "market_price")
        .or_else(|| json_f64_field(row, "price"))
        .unwrap_or(0.0);
    let avg = json_f64_field(row, "average_cost").or_else(|| json_f64_field(row, "cost_basis"));
    Some(WealthHolding {
        stock_id: 0,
        symbol,
        quantity,
        current_value: quantity * price,
        buy_price: avg,
        price,
    })
}

fn ngn_wallet_row(raw: &Value) -> Option<&Value> {
    let root = raw.get("data").unwrap_or(raw);
    let wallets = root
        .get("wallet_balance")
        .or_else(|| root.get("wallets"))
        .and_then(|v| v.as_array())?;
    wallets.iter().find(|row| {
        let currency = row
            .get("currency")
            .or_else(|| row.get("currency_code"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let symbol = row
            .get("currency_symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        currency.eq_ignore_ascii_case("NGN") || symbol.contains('₦')
    })
}

pub fn parse_ngn_cash(raw: &Value) -> Option<f64> {
    let row = ngn_wallet_row(raw)?;
    json_f64_field(row, "wallet_balance").or_else(|| json_f64_field(row, "balance"))
}

pub fn map_order_status(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "successful" | "success" | "filled" | "executed" | "complete" | "completed" => "executed",
        "rejected" | "failed" | "fail" | "error" => "rejected",
        "cancelled" | "canceled" => "cancelled",
        _ => "pending",
    }
}

fn parse_fee_quote(
    raw: &Value,
    symbol: &str,
    side: &str,
    quantity: f64,
    price: f64,
) -> BambooFeeQuote {
    let root = raw.get("data").unwrap_or(raw);
    let qty = json_f64_field(root, "quantity").unwrap_or(quantity);
    let pps = json_f64_field(root, "price_per_share")
        .or_else(|| json_f64_field(root, "price"))
        .unwrap_or(price);
    let fee = json_f64_field(root, "fee").unwrap_or(0.0);
    let total = json_f64_field(root, "total_price").unwrap_or(qty * pps + fee);
    BambooFeeQuote {
        fee,
        quantity: qty,
        price_per_share: pps,
        total_price: total,
        available_quantity: json_f64_field(root, "available_quantity").unwrap_or(0.0),
        symbol: root
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or(symbol)
            .to_string(),
        side: root
            .get("side")
            .and_then(|v| v.as_str())
            .unwrap_or(side)
            .to_string(),
        price: json_f64_field(root, "price").unwrap_or(price),
        order_price: json_f64_field(root, "order_price"),
        order_value: root
            .get("order_value")
            .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_f64().map(|n| n.to_string()))),
    }
}

fn parse_broker_order(raw: &Value, fallback_id: &str) -> crate::broker::BrokerOrder {
    let root = raw.get("data").unwrap_or(raw);
    let id = root
        .get("id")
        .or_else(|| root.get("order_id"))
        .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string())))
        .unwrap_or_else(|| fallback_id.to_string());
    let status_raw = root
        .get("status")
        .or_else(|| root.get("order_status"))
        .and_then(|v| v.as_str())
        .unwrap_or("Pending");
    crate::broker::BrokerOrder {
        id,
        numeric_id: None,
        stock_id: None,
        status: map_order_status(status_raw).into(),
        quantity: json_f64_field(root, "quantity")
            .or_else(|| json_f64_field(root, "filled_quantity"))
            .unwrap_or(0.0),
        quote_price: json_f64_field(root, "naira_price").or_else(|| json_f64_field(root, "price")),
        unit_price: json_f64_field(root, "price").or_else(|| json_f64_field(root, "naira_price")),
        rejection_reason: root
            .get("message")
            .or_else(|| root.get("reason"))
            .or_else(|| root.get("rejection_reason"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

fn parse_ngn_wallet_id(raw: &Value) -> Option<i64> {
    let row = ngn_wallet_row(raw)?;
    row.get("wallet_id")
        .or_else(|| row.get("id"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

fn extract_login_token(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_matches('"');
    if trimmed.starts_with("eyJ") && trimmed.contains('.') {
        return Some(trimmed.to_string());
    }
    let body: Value = serde_json::from_str(text).ok()?;
    if let Some(s) = body.as_str().filter(|s| s.starts_with("eyJ")) {
        return Some(s.to_string());
    }
    const KEYS: &[&str] = &["access_token", "token", "jwt", "data"];
    for key in KEYS {
        if let Some(v) = body.get(*key) {
            if let Some(s) = v.as_str().filter(|s| !s.is_empty()) {
                return Some(s.to_string());
            }
            if let Some(s) = v.get("access_token").or_else(|| v.get("token")).and_then(|x| x.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let mut padded = payload.replace('-', "+").replace('_', "/");
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn is_auth_error_body(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("unauthenticated") || lower.contains("invalid_token")
}

fn extract_error_message(body: &Value) -> Option<String> {
    body.get("message")
        .or_else(|| body.get("error"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn json_f64_field(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|n| n as f64))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

fn parse_json_body(text: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text).context("parse bamboo json")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ngx_lots_and_mark_value() {
        let raw = serde_json::json!({
            "stocks": [{
                "symbol": "DANGCEM",
                "quantity": 10,
                "average_cost": 300.0,
                "market_price": 330.5
            }]
        });
        let snap = parse_my_stocks(&raw);
        assert_eq!(snap.holdings.len(), 1);
        assert_eq!(snap.holdings[0].symbol, "DANGCEM");
        assert_eq!(snap.holdings[0].quantity, 10.0);
        assert!((snap.holdings[0].current_value - 3305.0).abs() < 0.01);
        assert_eq!(snap.holdings[0].buy_price, Some(300.0));
    }

    #[test]
    fn parses_ngn_wallet_balance_not_usd() {
        let raw = serde_json::json!({
            "wallet_balance": [
                {
                    "name": "Bamboo Base Wallet",
                    "currency": "USD",
                    "wallet_id": 351836,
                    "currency_symbol": "$",
                    "wallet_balance": 0.0
                },
                {
                    "name": "Naira Wallet",
                    "currency": "NGN",
                    "wallet_id": 675485,
                    "currency_symbol": "₦",
                    "wallet_balance": 4.9e3
                }
            ]
        });
        assert_eq!(parse_ngn_cash(&raw), Some(4900.0));
        assert_eq!(parse_ngn_wallet_id(&raw), Some(675485));
    }

    #[test]
    fn maps_order_status_strings() {
        assert_eq!(map_order_status("Successful"), "executed");
        assert_eq!(map_order_status("Filled"), "executed");
        assert_eq!(map_order_status("Rejected"), "rejected");
        assert_eq!(map_order_status("Pending"), "pending");
        assert_eq!(map_order_status("New"), "pending");
        assert_eq!(map_order_status("Cancelled"), "cancelled");
    }

    #[test]
    fn cost_basis_fallback() {
        let raw = serde_json::json!({
            "stocks": [{
                "symbol": "GTCO",
                "quantity": 2,
                "cost_basis": 40.0,
                "market_price": 46.0
            }]
        });
        let snap = parse_my_stocks(&raw);
        assert_eq!(snap.holdings[0].buy_price, Some(40.0));
        assert_eq!(snap.holdings[0].current_value, 92.0);
    }
}
