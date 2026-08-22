use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::Database;
use crate::wealth::{
    CachedWealthBook, TradingMode, WealthHolding, WealthPortfolioSnapshot, WealthProfileStatus,
    WealthWallet,
};

const BASE_URL: &str = "https://api.busha.io";
const ORIGIN: &str = "https://app.busha.io";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct BushaPair {
    pub id: String,
    pub base: String,
    pub counter: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub is_buy_supported: bool,
    pub is_sell_supported: bool,
    pub min_buy_ngn: f64,
    pub min_buy_base: f64,
    pub min_sell_ngn: f64,
    pub min_sell_base: f64,
    pub max_buy_ngn: f64,
    pub max_buy_base: f64,
    pub max_sell_ngn: f64,
    pub max_sell_base: f64,
    pub base_decimal: usize,
    pub counter_decimal: usize,
    pub percentage_change: f64,
}

#[derive(Debug, Clone)]
pub struct BushaQuote {
    pub id: String,
    pub source_currency: String,
    pub target_currency: String,
    pub source_amount: f64,
    pub target_amount: f64,
    pub rate: f64,
    pub side: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct BushaTransfer {
    pub id: String,
    pub quote_id: String,
    pub reference: String,
    pub status: String,
    pub source_amount: f64,
    pub target_amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BushaFeeQuote {
    pub quote_id: String,
    pub expires_at: DateTime<Utc>,
    pub source_amount: f64,
    pub target_amount: f64,
    pub rate: f64,
    pub side: String,
    pub source_currency: String,
    pub target_currency: String,
}

pub struct BushaSizedFee {
    pub fee: f64,
    pub quantity: f64,
    pub price_per_share: f64,
    pub total_price: f64,
    pub available_quantity: f64,
}

pub struct BushaClient {
    http: reqwest::Client,
    token: Mutex<String>,
    profile_id: String,
}

impl BushaClient {
    pub fn from_store() -> Result<Self> {
        let token = crate::auth_bridge::get_token("busha")?
            .ok_or_else(|| anyhow!("Busha is not connected"))?;
        let profile_id = crate::auth_bridge::get_profile_id("busha")?
            .ok_or_else(|| anyhow!("Busha profile id is missing; reconnect Busha"))?;
        Ok(Self {
            http: crate::http_client::http_client()?,
            token: Mutex::new(token),
            profile_id,
        })
    }

    fn reload_token(&self) -> Result<()> {
        let fresh = crate::auth_bridge::get_token("busha")?
            .ok_or_else(|| anyhow!("Busha is not connected"))?;
        *self.token.lock() = fresh;
        Ok(())
    }

    fn bearer_token(&self) -> String {
        self.token.lock().clone()
    }

    pub fn market_is_open(&self) -> Result<bool> {
        Ok(true)
    }

    pub async fn resolve_trading_mode(&self, _settings: &crate::settings::AppSettings) -> TradingMode {
        TradingMode::Live
    }

    pub async fn profile_status(&self, _settings: &crate::settings::AppSettings) -> WealthProfileStatus {
        let hint = crate::auth_bridge::config::get("busha")
            .ok()
            .and_then(|c| crate::auth_bridge::session::status_for(&c).account_hint);
        WealthProfileStatus {
            ok: true,
            connected: true,
            email: hint.clone(),
            trading_profile: Some("crypto".into()),
            trading_verified: true,
            trading_mode: "live".into(),
            brokerage_balance: None,
            available_balance: None,
            current_balance: None,
            base_url: BASE_URL.into(),
            message: "Busha session active.".into(),
            display_name: hint,
            has_session: true,
        }
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        let token = self.bearer_token();
        let auth = if token.to_ascii_lowercase().starts_with("bearer ") {
            token
        } else {
            format!("Bearer {token}")
        };
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth).map_err(|_| anyhow!("invalid Busha authorization"))?,
        );
        headers.insert(
            "x-bu-profile-id",
            HeaderValue::from_str(&self.profile_id).map_err(|_| anyhow!("invalid Busha profile id"))?,
        );
        headers.insert("origin", HeaderValue::from_static(ORIGIN));
        headers.insert("referer", HeaderValue::from_static("https://app.busha.io/"));
        headers.insert("accept", HeaderValue::from_static("*/*"));
        headers.insert("accept-language", HeaderValue::from_static("en-US,en;q=0.9"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        headers.insert(
            "sec-ch-ua",
            HeaderValue::from_static("\"Not=A?Brand\";v=\"99\", \"Google Chrome\";v=\"151\", \"Chromium\";v=\"151\""),
        );
        headers.insert("sec-ch-ua-mobile", HeaderValue::from_static("?0"));
        headers.insert("sec-ch-ua-platform", HeaderValue::from_static("\"macOS\""));
        headers.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
        headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-site"));
        Ok(headers)
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{BASE_URL}{path}");
        let response = self
            .http
            .get(&url)
            .headers(self.headers()?)
            .send()
            .await
            .context("Busha GET")?;
        if response.status().as_u16() == 401 {
            if crate::auth_bridge::refresh_busha_session().await.is_ok() {
                let _ = self.reload_token();
                let response = self
                    .http
                    .get(&url)
                    .headers(self.headers()?)
                    .send()
                    .await
                    .context("Busha GET retry")?;
                return self.read_json(response).await;
            }
            let _ = crate::auth_bridge::session::mark_expired("busha");
            return Err(anyhow!("Busha session expired. Sign in again."));
        }
        self.read_json(response).await
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{BASE_URL}{path}");
        let response = self
            .http
            .post(&url)
            .headers(self.headers()?)
            .json(body)
            .send()
            .await
            .context("Busha POST")?;
        if response.status().as_u16() == 401 {
            if crate::auth_bridge::refresh_busha_session().await.is_ok() {
                let _ = self.reload_token();
                let response = self
                    .http
                    .post(&url)
                    .headers(self.headers()?)
                    .json(body)
                    .send()
                    .await
                    .context("Busha POST retry")?;
                return self.read_json(response).await;
            }
            let _ = crate::auth_bridge::session::mark_expired("busha");
            return Err(anyhow!("Busha session expired. Sign in again."));
        }
        self.read_json(response).await
    }

    async fn read_json(&self, response: reqwest::Response) -> Result<Value> {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(busha_http_error(status.as_u16(), &text)));
        }
        serde_json::from_str(&text).context("Busha JSON")
    }

    pub async fn pairs_ngn(&self) -> Result<Vec<BushaPair>> {
        let v = self
            .get_json("/v1/pairs?counter=NGN&sort_by=market_cap&sort=desc")
            .await?;
        parse_pairs(&v)
    }

    pub async fn balances(&self) -> Result<(f64, Vec<WealthHolding>)> {
        let v = self.get_json("/v1/balances").await?;
        parse_balances(&v)
    }

    pub async fn overview_change(&self) -> Result<f64> {
        let v = self.get_json("/v1/balances/overview").await?;
        Ok(walk_f64(&v, "data.total.change_24h.amount").unwrap_or(0.0))
    }

    pub async fn create_quote(
        &self,
        source_currency: &str,
        target_currency: &str,
        source_amount: f64,
        decimals: Option<usize>,
        floor_amount: bool,
    ) -> Result<BushaQuote> {
        let body = quote_body_with_decimals(
            source_currency,
            target_currency,
            source_amount,
            decimals,
            floor_amount,
        );
        tracing::debug!(
            target: "busha",
            source = source_currency,
            target = target_currency,
            amount = body["source_amount"].as_str().unwrap_or(""),
            floor_amount,
            "creating Busha quote"
        );
        let v = self.post_json("/v1/quotes", &body).await?;
        parse_quote(&v)
    }

    pub async fn create_transfer(&self, quote: &BushaQuote) -> Result<BushaTransfer> {
        if quote_expired(quote, Utc::now()) {
            return Err(anyhow!("Busha quote expired; request a new quote"));
        }
        let body = json!({ "quote_id": quote.id });
        let v = self.post_json("/v1/transfers", &body).await?;
        parse_transfer(&v)
    }

    pub async fn transactions(&self, currency: &str) -> Result<Value> {
        self.get_json(&format!(
            "/v1/transactions?currency={}",
            urlencoding_lite(currency)
        ))
        .await
    }

    pub async fn get_wallet(&self) -> Result<WealthWallet> {
        let (cash, _) = self.balances().await?;
        Ok(WealthWallet {
            brokerage_balance: cash,
            available_balance: cash,
            current_balance: cash,
        })
    }

    pub async fn get_portfolio(&self) -> Result<WealthPortfolioSnapshot> {
        let (cash, holdings) = self.balances().await?;
        let mv: f64 = holdings.iter().map(|h| h.current_value).sum();
        let profit = self.overview_change().await.unwrap_or(0.0);
        Ok(WealthPortfolioSnapshot {
            balance: cash,
            profit,
            stock_value: mv,
            holdings,
            stocks_present: true,
        })
    }

    pub async fn resolve_quote(&self, symbol: &str) -> Result<f64> {
        self.resolve_quote_for(symbol, "BUY").await
    }

    pub async fn resolve_quote_for(&self, symbol: &str, side: &str) -> Result<f64> {
        let pairs = self.pairs_ngn().await?;
        let p = pair_for(&pairs, symbol).ok_or_else(|| anyhow!("No Busha NGN pair for {symbol}"))?;
        if side.eq_ignore_ascii_case("SELL") {
            Ok(p.sell_price)
        } else {
            Ok(p.buy_price)
        }
    }

    pub async fn ingest_universe(&self, db: &Database) -> Result<usize> {
        let pairs = self.pairs_ngn().await?;
        db.with_conn(|conn| persist_pairs(conn, &pairs))?;
        Ok(pairs.len())
    }

    pub async fn calculate_fee(
        &self,
        symbol: &str,
        side: &str,
        quantity: f64,
        price: f64,
    ) -> Result<(BushaSizedFee, BushaFeeQuote)> {
        let base = symbol.trim().to_uppercase();
        let buy = side.eq_ignore_ascii_case("BUY");
        let mut source_amount = if buy {
            quantity * price
        } else {
            quantity
        };
        if source_amount <= 0.0 {
            return Err(anyhow!("Busha quote amount must be positive"));
        }
        let mut decimals: Option<usize> = None;
        let mut floor_amount = false;
        if let Ok(pairs) = self.pairs_ngn().await {
            if let Some(p) = pair_for(&pairs, &base) {
                if buy {
                    source_amount = apply_pair_quote_limits(true, source_amount, price, p)?;
                    decimals = Some(p.counter_decimal);
                } else {
                    // Never raise a SELL above what the wallet can spend — meet_floor used to
                    // bump dust bags past `available` and Busha returned insufficient balance.
                    let available = self.available_crypto(&base).await.unwrap_or(source_amount);
                    source_amount = apply_sell_quote_limits(source_amount, price, p, available)?;
                    decimals = Some(p.base_decimal);
                    floor_amount = true;
                }
            }
        }
        let (source, target) = if buy {
            ("NGN", base.as_str())
        } else {
            (base.as_str(), "NGN")
        };
        let quote = self
            .create_quote(source, target, source_amount, decimals, floor_amount)
            .await?;
        let fee = BushaFeeQuote {
            quote_id: quote.id.clone(),
            expires_at: quote.expires_at,
            source_amount: quote.source_amount,
            target_amount: quote.target_amount,
            rate: quote.rate,
            side: quote.side.clone(),
            source_currency: quote.source_currency.clone(),
            target_currency: quote.target_currency.clone(),
        };
        let qty = if buy {
            quote.target_amount
        } else {
            quote.source_amount
        };
        let total = if buy {
            quote.source_amount
        } else {
            quote.target_amount
        };
        let pps = if quote.rate > 0.0 {
            quote.rate
        } else if qty > 0.0 {
            total / qty
        } else {
            price
        };
        Ok((
            BushaSizedFee {
                fee: 0.0,
                quantity: qty,
                price_per_share: pps,
                total_price: total,
                available_quantity: f64::MAX,
            },
            fee,
        ))
    }

    async fn available_crypto(&self, currency: &str) -> Result<f64> {
        let (cash, holdings) = self.balances().await?;
        let _ = cash;
        let code = currency.trim().to_uppercase();
        Ok(holdings
            .iter()
            .find(|h| h.symbol.eq_ignore_ascii_case(&code))
            .map(|h| h.quantity)
            .unwrap_or(0.0))
    }

    pub async fn place_and_await_fill(
        &self,
        fee: &BushaFeeQuote,
        symbol: &str,
    ) -> Result<BushaTransfer> {
        let quote = BushaQuote {
            id: fee.quote_id.clone(),
            source_currency: fee.source_currency.clone(),
            target_currency: fee.target_currency.clone(),
            source_amount: fee.source_amount,
            target_amount: fee.target_amount,
            rate: fee.rate,
            side: fee.side.clone(),
            expires_at: fee.expires_at,
        };
        let transfer = self.create_transfer(&quote).await?;
        let currency = symbol.trim().to_uppercase();
        for _ in 0..6 {
            if let Ok(tx) = self.transactions(&currency).await {
                if transfer_matches_tx(&transfer, &tx) {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
        Ok(transfer)
    }
}

fn urlencoding_lite(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

pub fn quote_body(source: &str, target: &str, source_amount: f64) -> Value {
    quote_body_with_decimals(source, target, source_amount, None, false)
}

pub fn quote_body_with_decimals(
    source: &str,
    target: &str,
    source_amount: f64,
    decimals: Option<usize>,
    floor_amount: bool,
) -> Value {
    let places = decimals.unwrap_or_else(|| {
        if source.eq_ignore_ascii_case("NGN") {
            2
        } else {
            8
        }
    });
    let amount = if floor_amount {
        format_amount_floor(source_amount, places)
    } else {
        format_amount_decimals(source_amount, places)
    };
    json!({
        "source_currency": source,
        "target_currency": target,
        "source_amount": amount,
    })
}

/// Raise `amount` to the pair's posted min, using that pair's decimal places.
pub fn meet_floor(amount: f64, min: f64, decimals: usize) -> f64 {
    if !min.is_finite() || min <= 0.0 {
        return amount;
    }
    if amount + 1e-12 >= min {
        return amount;
    }
    let d = decimals.min(12);
    let factor = 10_f64.powi(d as i32);
    let bumped = (min * factor).ceil() / factor;
    if bumped <= min + 1e-12 {
        min + 1.0 / factor
    } else {
        bumped
    }
}

pub fn floor_amount_decimals(n: f64, decimals: usize) -> f64 {
    if !n.is_finite() || n <= 0.0 {
        return 0.0;
    }
    let d = decimals.min(12);
    let factor = 10_f64.powi(d as i32);
    (n * factor).floor() / factor
}

/// Size a SELL without ever quoting more than `available` (floored to pair decimals).
pub fn apply_sell_quote_limits(
    requested: f64,
    price: f64,
    pair: &BushaPair,
    available: f64,
) -> Result<f64> {
    let px = if pair.sell_price > 0.0 {
        pair.sell_price
    } else {
        price
    };
    // Leave one quantum so float / lock dust does not trip insufficient balance.
    let eps = 10_f64.powi(-(pair.base_decimal.min(12) as i32));
    let mut qty = floor_amount_decimals(requested.min(available) - eps, pair.base_decimal);
    if qty <= 0.0 {
        return Err(anyhow!(
            "Busha {} sell size is zero after flooring available {:.8}",
            pair.base,
            available
        ));
    }
    if pair.max_sell_base > 0.0 && qty > pair.max_sell_base + 1e-12 {
        qty = floor_amount_decimals(pair.max_sell_base, pair.base_decimal);
    }
    if px > 0.0 && pair.max_sell_ngn > 0.0 && qty * px > pair.max_sell_ngn + 1e-9 {
        qty = floor_amount_decimals(pair.max_sell_ngn / px, pair.base_decimal);
    }
    if pair.min_sell_base > 0.0 && qty + 1e-12 < pair.min_sell_base {
        return Err(anyhow!(
            "Busha {} position {:.8} is below min sell {:.8}",
            pair.base,
            available,
            pair.min_sell_base
        ));
    }
    if px > 0.0 && pair.min_sell_ngn > 0.0 && qty * px + 1e-12 < pair.min_sell_ngn {
        return Err(anyhow!(
            "Busha {} sell ₦{:.2} is below min ₦{:.2} (held {:.8})",
            pair.base,
            qty * px,
            pair.min_sell_ngn,
            available
        ));
    }
    Ok(qty)
}

pub fn apply_pair_quote_limits(
    buy: bool,
    source_amount: f64,
    price: f64,
    pair: &BushaPair,
) -> Result<f64> {
    if buy {
        let mut ngn = meet_floor(source_amount, pair.min_buy_ngn, pair.counter_decimal);
        let px = if pair.buy_price > 0.0 {
            pair.buy_price
        } else {
            price
        };
        if px > 0.0 && pair.min_buy_base > 0.0 && ngn / px + 1e-12 < pair.min_buy_base {
            ngn = meet_floor(
                pair.min_buy_base * px,
                pair.min_buy_ngn,
                pair.counter_decimal,
            );
        }
        if pair.max_buy_ngn > 0.0 && ngn > pair.max_buy_ngn + 1e-9 {
            return Err(anyhow!(
                "Busha {} buy ₦{:.2} exceeds max ₦{:.2}",
                pair.base,
                ngn,
                pair.max_buy_ngn
            ));
        }
        Ok(ngn)
    } else {
        // Legacy path without wallet clamp — prefer `apply_sell_quote_limits`.
        let mut qty = meet_floor(source_amount, pair.min_sell_base, pair.base_decimal);
        let px = if pair.sell_price > 0.0 {
            pair.sell_price
        } else {
            price
        };
        if px > 0.0 && pair.min_sell_ngn > 0.0 && qty * px + 1e-12 < pair.min_sell_ngn {
            qty = meet_floor(
                pair.min_sell_ngn / px,
                pair.min_sell_base,
                pair.base_decimal,
            );
        }
        if pair.max_sell_base > 0.0 && qty > pair.max_sell_base + 1e-12 {
            qty = pair.max_sell_base;
        }
        if px > 0.0 && pair.max_sell_ngn > 0.0 && qty * px > pair.max_sell_ngn + 1e-9 {
            qty = pair.max_sell_ngn / px;
        }
        Ok(qty)
    }
}

pub fn format_amount_for(currency: &str, n: f64) -> String {
    let decimals = if currency.eq_ignore_ascii_case("NGN") {
        2
    } else {
        8
    };
    format_amount_decimals(n, decimals)
}

pub fn format_amount_decimals(n: f64, decimals: usize) -> String {
    if !n.is_finite() || n <= 0.0 {
        return "0".into();
    }
    let d = decimals.min(12);
    let factor = 10_f64.powi(d as i32);
    let rounded = (n * factor).round() / factor;
    let s = format!("{rounded:.d$}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Format for SELL source amounts — never round up past the wallet balance.
pub fn format_amount_floor(n: f64, decimals: usize) -> String {
    if !n.is_finite() || n <= 0.0 {
        return "0".into();
    }
    let floored = floor_amount_decimals(n, decimals);
    if floored <= 0.0 {
        return "0".into();
    }
    let d = decimals.min(12);
    let s = format!("{floored:.d$}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn busha_http_error(status: u16, body: &str) -> String {
    let detail = busha_error_detail(body);
    if detail.is_empty() {
        format!("Busha HTTP {status}")
    } else {
        format!("Busha HTTP {status}: {detail}")
    }
}

fn busha_error_detail(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        for path in ["message", "error", "error.message", "data.message"] {
            if let Some(s) = walk(&v, path).and_then(|x| x.as_str()) {
                let s = s.trim();
                if !s.is_empty() {
                    return s.chars().take(240).collect();
                }
            }
        }
        if let Some(arr) = v.get("errors").and_then(|x| x.as_array()) {
            let joined: Vec<&str> = arr.iter().filter_map(|x| x.as_str()).take(3).collect();
            if !joined.is_empty() {
                return joined.join("; ").chars().take(240).collect();
            }
        }
    }
    trimmed.chars().take(160).collect()
}

pub fn quote_expired(quote: &BushaQuote, now: DateTime<Utc>) -> bool {
    quote.expires_at <= now
}

pub fn parse_amount_str(raw: &str) -> f64 {
    raw.trim().parse::<f64>().unwrap_or(0.0)
}

fn money(v: &Value) -> f64 {
    if let Some(s) = v.as_str() {
        return parse_amount_str(s);
    }
    if let Some(n) = v.as_f64() {
        return n;
    }
    if let Some(s) = v.get("amount").and_then(|x| x.as_str()) {
        return parse_amount_str(s);
    }
    v.get("amount").and_then(|x| x.as_f64()).unwrap_or(0.0)
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

fn walk_f64(value: &Value, path: &str) -> Option<f64> {
    walk(value, path).map(money)
}

pub fn parse_pairs(root: &Value) -> Result<Vec<BushaPair>> {
    let arr = root
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("pairs missing data"))?;
    let mut out = Vec::new();
    for item in arr {
        let base = item.get("base").and_then(|v| v.as_str()).unwrap_or("").to_uppercase();
        let counter = item
            .get("counter")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        if counter != "NGN" || base.is_empty() {
            continue;
        }
        let buy = item.get("is_buy_supported").and_then(|v| v.as_bool()).unwrap_or(false);
        let sell = item.get("is_sell_supported").and_then(|v| v.as_bool()).unwrap_or(false);
        if !buy && !sell {
            continue;
        }
        let pct = item
            .get("percentage_change")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        out.push(BushaPair {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&format!("{base}NGN"))
                .to_string(),
            base,
            counter,
            buy_price: item.get("buy_price").map(money).unwrap_or(0.0),
            sell_price: item.get("sell_price").map(money).unwrap_or(0.0),
            is_buy_supported: buy,
            is_sell_supported: sell,
            min_buy_ngn: item
                .pointer("/min_buy_amount/counter")
                .map(money)
                .unwrap_or(0.0),
            min_buy_base: item
                .pointer("/min_buy_amount")
                .map(money)
                .unwrap_or(0.0),
            min_sell_ngn: item
                .pointer("/min_sell_amount/counter")
                .map(money)
                .unwrap_or(0.0),
            min_sell_base: item
                .pointer("/min_sell_amount")
                .map(money)
                .unwrap_or(0.0),
            max_buy_ngn: item
                .pointer("/max_buy_amount/counter")
                .map(money)
                .unwrap_or(0.0),
            max_buy_base: item
                .pointer("/max_buy_amount")
                .map(money)
                .unwrap_or(0.0),
            max_sell_ngn: item
                .pointer("/max_sell_amount/counter")
                .map(money)
                .unwrap_or(0.0),
            max_sell_base: item
                .pointer("/max_sell_amount")
                .map(money)
                .unwrap_or(0.0),
            base_decimal: item
                .get("base_decimal")
                .map(|v| parse_decimal_places(Some(v)))
                .unwrap_or(8),
            counter_decimal: item
                .get("counter_decimal")
                .map(|v| parse_decimal_places(Some(v)))
                .unwrap_or(2),
            percentage_change: pct,
        });
    }
    Ok(out)
}

/// Fiat wallets that are never portfolio coins (NGN is cash; KES/USD are ignored).
fn is_omitted_fiat_currency(code: &str) -> bool {
    matches!(code, "NGN" | "KES" | "USD")
}

pub fn parse_balances(root: &Value) -> Result<(f64, Vec<WealthHolding>)> {
    let arr = root
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("balances missing data"))?;
    let mut cash = 0.0;
    let mut holdings = Vec::new();
    for item in arr {
        let code = item
            .get("currency")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let available = item.get("available").map(money).unwrap_or(0.0);
        if code == "NGN" && kind == "fiat" {
            cash = available;
            continue;
        }
        // Live Busha sets trade=false on held coins; do not gate portfolio on trade.
        if is_omitted_fiat_currency(&code) || kind != "crypto" || available <= 0.0 {
            continue;
        }
        let fiat = item
            .pointer("/available/fiat")
            .map(money)
            .unwrap_or(0.0);
        let px = if available > 0.0 { fiat / available } else { 0.0 };
        holdings.push(WealthHolding {
            stock_id: 0,
            symbol: code,
            quantity: available,
            current_value: fiat,
            buy_price: None,
            price: px,
        });
    }
    Ok((cash, holdings))
}

pub fn parse_quote(root: &Value) -> Result<BushaQuote> {
    let data = root.get("data").cloned().unwrap_or(root.clone());
    let id = data
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("quote missing id"))?
        .to_string();
    let expires = data
        .get("expires_at")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .ok_or_else(|| anyhow!("quote missing expires_at"))?;
    let rate = data.pointer("/rate/rate").map(money).unwrap_or(0.0);
    let side = data
        .pointer("/rate/side")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(BushaQuote {
        id,
        source_currency: data
            .get("source_currency")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        target_currency: data
            .get("target_currency")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        source_amount: data.get("source_amount").map(money).unwrap_or(0.0),
        target_amount: data.get("target_amount").map(money).unwrap_or(0.0),
        rate,
        side,
        expires_at: expires,
    })
}

pub fn parse_transfer(root: &Value) -> Result<BushaTransfer> {
    let data = root.get("data").cloned().unwrap_or(root.clone());
    Ok(BushaTransfer {
        id: data.get("id").and_then(|v| v.as_str()).unwrap_or("").into(),
        quote_id: data
            .get("quote_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        reference: data
            .get("reference")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        status: data
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        source_amount: data.get("source_amount").map(money).unwrap_or(0.0),
        target_amount: data.get("target_amount").map(money).unwrap_or(0.0),
    })
}

pub fn transfer_matches_tx(transfer: &BushaTransfer, tx: &Value) -> bool {
    let Some(arr) = tx.get("data").and_then(|v| v.as_array()) else {
        return false;
    };
    arr.iter().any(|item| {
        let reference = item.get("reference").and_then(|v| v.as_str()).unwrap_or("");
        reference == transfer.id
            || reference == transfer.quote_id
            || reference == transfer.reference
            || reference.replace("CNV_", "TRF_") == transfer.id
            || item.get("id").and_then(|v| v.as_str()) == Some(transfer.id.as_str())
    })
}

fn parse_decimal_places(v: Option<&Value>) -> usize {
    match v {
        Some(Value::String(s)) => s.parse().ok().unwrap_or(8),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(8) as usize,
        _ => 8,
    }
    .min(12)
}

fn pair_for<'a>(pairs: &'a [BushaPair], symbol: &str) -> Option<&'a BushaPair> {
    let s = symbol.trim().to_uppercase();
    pairs.iter().find(|p| p.base == s || p.id.eq_ignore_ascii_case(&s))
}

/// Exit marks (crypto→NGN) for held symbols from `/v1/pairs`.
pub fn sell_marks_for_symbols(
    pairs: &[BushaPair],
    symbols: &[String],
) -> std::collections::HashMap<String, f64> {
    let mut out = std::collections::HashMap::new();
    for sym in symbols {
        let key = sym.trim().to_uppercase();
        if key.is_empty() {
            continue;
        }
        if let Some(p) = pair_for(pairs, &key) {
            let px = if p.sell_price > 0.0 {
                p.sell_price
            } else {
                p.buy_price
            };
            if px > 0.0 {
                out.insert(key, px);
            }
        }
    }
    out
}

/// Persist sell marks into `price_history` + cache for risk / UI fallbacks.
pub fn apply_sell_marks(
    conn: &Connection,
    cache: &crate::cache::PriceCache,
    pairs: &[BushaPair],
    symbols: &[String],
    trade_date: &str,
) -> Result<std::collections::HashMap<String, f64>> {
    use crate::cache::CachedPrice;
    let marks = sell_marks_for_symbols(pairs, symbols);
    for (symbol, price) in &marks {
        let pct = pair_for(pairs, symbol)
            .map(|p| p.percentage_change)
            .unwrap_or(0.0);
        conn.execute(
            "INSERT INTO instruments (symbol, name, sector, is_active) VALUES (?1, ?1, 'CRYPTO', 1)
             ON CONFLICT(symbol) DO UPDATE SET sector = 'CRYPTO', is_active = 1",
            [symbol],
        )?;
        conn.execute(
            "INSERT INTO price_history (symbol, trade_date, price, change_percent, ingested_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(symbol, trade_date) DO UPDATE SET
               price = excluded.price, change_percent = excluded.change_percent, ingested_at = datetime('now')",
            rusqlite::params![symbol, trade_date, price, pct],
        )?;
        cache.set_price(&CachedPrice {
            symbol: symbol.clone(),
            price: *price,
            trade_date: trade_date.to_string(),
            updated_at: Utc::now().to_rfc3339(),
        });
    }
    Ok(marks)
}

pub fn min_buy_ngn_for(conn: &Connection, symbol: &str) -> Option<f64> {
    conn.query_row(
        "SELECT min_buy_ngn FROM busha_pairs WHERE symbol = ?1",
        [symbol.trim().to_uppercase()],
        |row| row.get::<_, f64>(0),
    )
    .ok()
    .filter(|n| n.is_finite() && *n > 0.0)
}

pub fn persist_pairs(conn: &Connection, pairs: &[BushaPair]) -> Result<()> {
    conn.execute(
        "UPDATE instruments SET is_active = 0 WHERE sector = 'CRYPTO'",
        [],
    )?;
    conn.execute("DELETE FROM busha_pairs", [])?;
    let today = Utc::now().date_naive().to_string();
    for p in pairs {
        conn.execute(
            "INSERT INTO instruments (symbol, name, sector, is_active) VALUES (?1, ?2, 'CRYPTO', 1)
             ON CONFLICT(symbol) DO UPDATE SET name = excluded.name, sector = 'CRYPTO', is_active = 1",
            rusqlite::params![p.base, p.id],
        )?;
        conn.execute(
            "INSERT INTO price_history (symbol, trade_date, price, change_percent, volume)
             VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT(symbol, trade_date) DO UPDATE SET price = excluded.price, change_percent = excluded.change_percent",
            rusqlite::params![p.base, today, p.buy_price, p.percentage_change],
        )?;
        conn.execute(
            "INSERT INTO busha_pairs (
                symbol, pair_id, buy_price, sell_price,
                min_buy_ngn, min_buy_base, min_sell_ngn, min_sell_base,
                max_buy_ngn, max_buy_base, max_sell_ngn, max_sell_base,
                base_decimal, counter_decimal, synced_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, datetime('now'))",
            rusqlite::params![
                p.base,
                p.id,
                p.buy_price,
                p.sell_price,
                p.min_buy_ngn,
                p.min_buy_base,
                p.min_sell_ngn,
                p.min_sell_base,
                p.max_buy_ngn,
                p.max_buy_base,
                p.max_sell_ngn,
                p.max_sell_base,
                p.base_decimal as i64,
                p.counter_decimal as i64,
            ],
        )?;
    }
    Ok(())
}

pub struct BushaSyncService;

impl BushaSyncService {
    pub async fn refresh(db: &Database, client: &BushaClient) -> Result<CachedWealthBook> {
        let snap = client.get_portfolio().await?;
        let cash = snap.balance;
        db.with_conn(|conn| persist_book(conn, &snap, cash))?;
        db.with_conn(load_book)?
            .ok_or_else(|| anyhow!("Busha cache empty after persist"))
    }

    pub fn load(db: &Database) -> Result<Option<CachedWealthBook>> {
        db.with_conn(load_book)
    }
}

pub fn load_snapshot(conn: &Connection) -> Result<Option<CachedWealthBook>> {
    load_book(conn)
}

fn persist_book(conn: &Connection, snap: &WealthPortfolioSnapshot, cash: f64) -> Result<()> {
    conn.execute(
        "INSERT INTO busha_account (id, brokerage_balance, stock_value, profit, synced_at)
         VALUES (1, ?1, ?2, ?3, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET
            brokerage_balance = excluded.brokerage_balance,
            stock_value = excluded.stock_value,
            profit = excluded.profit,
            synced_at = excluded.synced_at",
        rusqlite::params![cash, snap.stock_value, snap.profit],
    )?;
    conn.execute("DELETE FROM busha_positions", [])?;
    for h in &snap.holdings {
        conn.execute(
            "INSERT INTO busha_positions (symbol, stock_id, quantity, avg_cost, last_price, current_value, synced_at)
             VALUES (?1, 0, ?2, ?3, ?4, ?5, datetime('now'))",
            rusqlite::params![
                h.symbol,
                h.quantity,
                h.buy_price.unwrap_or(h.price),
                h.price,
                h.current_value
            ],
        )?;
    }
    Ok(())
}

fn load_book(conn: &Connection) -> Result<Option<CachedWealthBook>> {
    let Ok((cash, mv, profit, synced)) = conn.query_row(
        "SELECT brokerage_balance, stock_value, profit, synced_at FROM busha_account WHERE id = 1",
        [],
        |row| Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?, row.get::<_, f64>(2)?, row.get::<_, String>(3)?)),
    ) else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT symbol, quantity, avg_cost, last_price, current_value FROM busha_positions WHERE quantity > 0",
    )?;
    let holdings = stmt
        .query_map([], |row| {
            Ok(WealthHolding {
                stock_id: 0,
                symbol: row.get(0)?,
                quantity: row.get(1)?,
                buy_price: row.get(2)?,
                price: row.get(3)?,
                current_value: row.get(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(Some(CachedWealthBook {
        brokerage_balance: cash,
        stock_value: mv,
        profit,
        synced_at: synced,
        holdings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BALANCES: &str = r#"{
      "status":"success",
      "data":[
        {"currency":"KES","type":"fiat","trade":false,"available":{"amount":"0","currency":"KES"}},
        {"currency":"NGN","type":"fiat","trade":false,"available":{"amount":"125000.50","currency":"NGN"}},
        {"currency":"USD","type":"fiat","trade":false,"available":{"amount":"0","currency":"USD"}},
        {"currency":"BTC","type":"crypto","trade":false,"available":{"amount":"0.01","fiat":{"amount":"935016.95","currency":"NGN"},"currency":"BTC"}},
        {"currency":"USDT","type":"crypto","trade":false,"available":{"amount":"10","fiat":{"amount":"16000","currency":"NGN"},"currency":"USDT"}},
        {"currency":"ETH","type":"crypto","trade":false,"available":{"amount":"0","fiat":{"amount":"0","currency":"NGN"},"currency":"ETH"}}
      ]
    }"#;

    /// Live-shaped capture: held coins have trade=false; zero rows and fiat wallets must not inflate MV.
    const BALANCES_LIVE_BOOK: &str = r#"{
      "status":"success",
      "message":"Fetched balances successfully",
      "data":[
        {"currency":"KES","type":"fiat","trade":false,"available":{"amount":"0","fiat":{"amount":"0","currency":"NGN"},"currency":"KES"}},
        {"currency":"NGN","type":"fiat","trade":false,"available":{"amount":"2869.01","fiat":{"amount":"2869.01","currency":"NGN"},"currency":"NGN"}},
        {"currency":"USD","type":"fiat","trade":false,"available":{"amount":"0","fiat":{"amount":"0","currency":"NGN"},"currency":"USD"}},
        {"currency":"ADA","type":"crypto","trade":false,"available":{"amount":"1.1","fiat":{"amount":"312.28","currency":"NGN"},"currency":"ADA"}},
        {"currency":"ALICE","type":"crypto","trade":false,"available":{"amount":"1.417886","fiat":{"amount":"294.54","currency":"NGN"},"currency":"ALICE"}},
        {"currency":"ARKM","type":"crypto","trade":false,"available":{"amount":"3.1","fiat":{"amount":"416.08","currency":"NGN"},"currency":"ARKM"}},
        {"currency":"BTC","type":"crypto","trade":false,"available":{"amount":"0","fiat":{"amount":"0","currency":"NGN"},"currency":"BTC"}},
        {"currency":"CHZ","type":"crypto","trade":false,"available":{"amount":"16.291202","fiat":{"amount":"295.52","currency":"NGN"},"currency":"CHZ"}},
        {"currency":"ETH","type":"crypto","trade":false,"available":{"amount":"0","fiat":{"amount":"0","currency":"NGN"},"currency":"ETH"}},
        {"currency":"FIL","type":"crypto","trade":false,"available":{"amount":"0.34154033","fiat":{"amount":"350","currency":"NGN"},"currency":"FIL"}},
        {"currency":"MET","type":"crypto","trade":false,"available":{"amount":"1","fiat":{"amount":"302.14","currency":"NGN"},"currency":"MET"}},
        {"currency":"TRX","type":"crypto","trade":false,"available":{"amount":"0.6","fiat":{"amount":"275.51","currency":"NGN"},"currency":"TRX"}},
        {"currency":"USDC","type":"crypto","trade":false,"available":{"amount":"0","fiat":{"amount":"0","currency":"NGN"},"currency":"USDC"}},
        {"currency":"USDT","type":"crypto","trade":false,"available":{"amount":"0","fiat":{"amount":"0","currency":"NGN"},"currency":"USDT"}}
      ]
    }"#;

    const PAIRS: &str = r#"{
      "data":[{
        "id":"BTCNGN","base":"BTC","counter":"NGN",
        "buy_price":{"amount":"93501695.36","currency":"NGN"},
        "sell_price":{"amount":"88919802.32","currency":"NGN"},
        "is_buy_supported":true,"is_sell_supported":true,
        "min_buy_amount":{"amount":"0.000005","counter":{"amount":"467.51","currency":"NGN"},"currency":"BTC"},
        "min_sell_amount":{"amount":"0.000005","counter":{"amount":"444.60","currency":"NGN"},"currency":"BTC"},
        "max_buy_amount":{"amount":"1","counter":{"amount":"93501695.36","currency":"NGN"},"currency":"BTC"},
        "max_sell_amount":{"amount":"1","counter":{"amount":"88919802.32","currency":"NGN"},"currency":"BTC"},
        "base_decimal":"8","counter_decimal":"2",
        "percentage_change":"1.1"
      }]
    }"#;

    const BUY_QUOTE: &str = r#"{
      "status":"success",
      "data":{
        "id":"QUO_PffLl5DprKTr",
        "source_currency":"NGN","target_currency":"ARKM",
        "source_amount":"650","target_amount":"5.2",
        "rate":{"product":"ARKMNGN","rate":"124.78","side":"buy","type":"FIXED"},
        "expires_at":"2026-08-19T15:12:12.476035361Z"
      }
    }"#;

    const SELL_QUOTE: &str = r#"{
      "status":"success",
      "data":{
        "id":"QUO_wU0f1mFOD0JM",
        "source_currency":"ARKM","target_currency":"NGN",
        "source_amount":"2.1","target_amount":"248.07",
        "rate":{"product":"ARKMNGN","rate":"118.13","side":"sell","type":"FIXED"},
        "expires_at":"2026-08-19T15:17:53.959425884Z"
      }
    }"#;

    const TRANSFER: &str = r#"{
      "status":"success",
      "data":{
        "id":"TRF_ZDymeHnRJ4VM",
        "quote_id":"QUO_PffLl5DprKTr",
        "reference":"QUO_PffLl5DprKTr",
        "status":"pending",
        "source_amount":"650",
        "target_amount":"5.2"
      }
    }"#;

    const TXS: &str = r#"{
      "data":[{
        "status":"completed","type":"buys","reference":"CNV_ZDymeHnRJ4VM","is_credit":true
      }]
    }"#;

    #[test]
    fn parses_ngn_cash_and_held_crypto_ignoring_trade_flag() {
        let v: Value = serde_json::from_str(BALANCES).unwrap();
        let (cash, holds) = parse_balances(&v).unwrap();
        assert!((cash - 125000.50).abs() < 0.01);
        assert_eq!(holds.len(), 2);
        assert_eq!(holds[0].symbol, "BTC");
        assert!((holds[0].quantity - 0.01).abs() < 1e-9);
        assert_eq!(holds[1].symbol, "USDT");
        assert!((holds[1].current_value - 16_000.0).abs() < 0.01);
        let mv: f64 = holds.iter().map(|h| h.current_value).sum();
        assert!((mv - 951_016.95).abs() < 0.01);
    }

    #[test]
    fn live_balances_book_matches_equity_bar() {
        let v: Value = serde_json::from_str(BALANCES_LIVE_BOOK).unwrap();
        let (cash, holds) = parse_balances(&v).unwrap();
        assert!((cash - 2869.01).abs() < 0.01);
        assert_eq!(holds.len(), 7);
        let symbols: Vec<&str> = holds.iter().map(|h| h.symbol.as_str()).collect();
        assert_eq!(
            symbols,
            vec!["ADA", "ALICE", "ARKM", "CHZ", "FIL", "MET", "TRX"]
        );
        assert!(!symbols.iter().any(|s| matches!(*s, "NGN" | "KES" | "USD" | "BTC" | "ETH" | "USDC" | "USDT")));
        let mv: f64 = holds.iter().map(|h| h.current_value).sum();
        assert!((mv - 2246.07).abs() < 0.02);
        assert!((cash + mv - 5115.08).abs() < 0.02);
    }

    #[test]
    fn parses_ngn_pairs() {
        let v: Value = serde_json::from_str(PAIRS).unwrap();
        let pairs = parse_pairs(&v).unwrap();
        assert_eq!(pairs[0].base, "BTC");
        assert!((pairs[0].buy_price - 93501695.36).abs() < 0.01);
        assert!((pairs[0].min_buy_ngn - 467.51).abs() < 0.01);
        assert!((pairs[0].min_sell_ngn - 444.60).abs() < 0.01);
        assert!((pairs[0].max_buy_ngn - 93_501_695.36).abs() < 0.01);
        assert_eq!(pairs[0].base_decimal, 8);
        assert_eq!(pairs[0].counter_decimal, 2);
    }

    #[test]
    fn sell_marks_use_sell_price_not_buy_for_held() {
        let pairs = parse_pairs(&serde_json::from_str(PAIRS).unwrap()).unwrap();
        let marks = sell_marks_for_symbols(&pairs, &["BTC".into(), "ADA".into()]);
        assert_eq!(marks.len(), 1);
        assert!((marks["BTC"] - 88_919_802.32).abs() < 0.01);
        assert!(marks["BTC"] < pairs[0].buy_price);
        assert!(!marks.contains_key("ADA"));
    }

    #[test]
    fn pair_limits_are_per_asset_not_a_global_250() {
        let btc = parse_pairs(&serde_json::from_str(PAIRS).unwrap()).unwrap();
        let p = &btc[0];
        let bumped = apply_pair_quote_limits(true, 80.0, p.buy_price, p).unwrap();
        assert!(bumped + 1e-9 >= 467.51);
        let cngn = BushaPair {
            id: "CNGNNGN".into(),
            base: "CNGN".into(),
            counter: "NGN".into(),
            buy_price: 1.001,
            sell_price: 0.999,
            is_buy_supported: true,
            is_sell_supported: true,
            min_buy_ngn: 1.001,
            min_buy_base: 1.0,
            min_sell_ngn: 0.999,
            min_sell_base: 1.0,
            max_buy_ngn: 50_050_000.0,
            max_buy_base: 50_000_000.0,
            max_sell_ngn: 49_950_000.0,
            max_sell_base: 50_000_000.0,
            base_decimal: 2,
            counter_decimal: 3,
            percentage_change: 0.0,
        };
        let tiny = apply_pair_quote_limits(true, 80.0, 1.001, &cngn).unwrap();
        assert!((tiny - 80.0).abs() < 1e-9);
        let usdt = BushaPair {
            id: "USDTNGN".into(),
            base: "USDT".into(),
            counter: "NGN".into(),
            buy_price: 1400.94,
            sell_price: 1385.34,
            is_buy_supported: true,
            is_sell_supported: true,
            min_buy_ngn: 2619.76,
            min_buy_base: 1.87,
            min_sell_ngn: 2853.8,
            min_sell_base: 2.06,
            max_buy_ngn: 700_470_000.0,
            max_buy_base: 500_000.0,
            max_sell_ngn: 692_670_000.0,
            max_sell_base: 500_000.0,
            base_decimal: 2,
            counter_decimal: 2,
            percentage_change: 0.0,
        };
        let usdt_q = apply_pair_quote_limits(true, 950.0, 1400.94, &usdt).unwrap();
        assert!(usdt_q + 1e-9 >= 2619.76);
        let sell = apply_pair_quote_limits(false, 1.0, 1385.34, &usdt).unwrap();
        assert!(sell + 1e-9 >= 2.06);
        let clamped = apply_sell_quote_limits(1.0, 1385.34, &usdt, 1.0);
        assert!(clamped.is_err(), "must not bump sell above available wallet qty");
        let ok = apply_sell_quote_limits(5.0, 1385.34, &usdt, 5.0).unwrap();
        assert!(ok + 1e-9 >= 2.06);
        assert!(ok <= 5.0);
    }

    #[test]
    fn ngn_floor_uses_posted_pair_min() {
        assert!(meet_floor(80.0, 250.0, 2) >= 250.01);
        assert!(meet_floor(80.0, 250.000001, 2) > 250.0);
        assert!((meet_floor(650.0, 250.0, 2) - 650.0).abs() < 1e-9);
        assert!(meet_floor(0.5, 1.001, 3) >= 1.001);
        assert!((meet_floor(80.0, 0.0, 2) - 80.0).abs() < 1e-9);
    }

    #[test]
    fn buy_and_sell_quote_bodies_and_sides() {
        let buy = quote_body("NGN", "ARKM", 650.0);
        assert_eq!(buy["source_currency"], "NGN");
        assert_eq!(buy["target_currency"], "ARKM");
        assert_eq!(buy["source_amount"], "650");
        let ngn = quote_body("NGN", "BTC", 19_999.847_291_63);
        assert_eq!(ngn["source_amount"], "19999.85");
        let err = busha_http_error(
            400,
            r#"{"status":"error","message":"source_amount is below the minimum"}"#,
        );
        assert!(err.contains("below the minimum"));
        let q = parse_quote(&serde_json::from_str(BUY_QUOTE).unwrap()).unwrap();
        assert_eq!(q.id, "QUO_PffLl5DprKTr");
        assert_eq!(q.side, "buy");
        let sell_body = quote_body("ARKM", "NGN", 2.1);
        assert_eq!(sell_body["source_currency"], "ARKM");
        let s = parse_quote(&serde_json::from_str(SELL_QUOTE).unwrap()).unwrap();
        assert_eq!(s.side, "sell");
        assert!((s.source_amount - 2.1).abs() < 1e-9);
    }

    #[test]
    fn expired_quote_is_rejected() {
        let mut q = parse_quote(&serde_json::from_str(BUY_QUOTE).unwrap()).unwrap();
        q.expires_at = Utc::now() - chrono::Duration::seconds(1);
        assert!(quote_expired(&q, Utc::now()));
        let client_err = quote_expired(&q, Utc::now());
        assert!(client_err);
    }

    #[test]
    fn expired_quote_must_not_transfer() {
        let mut q = parse_quote(&serde_json::from_str(BUY_QUOTE).unwrap()).unwrap();
        q.expires_at = Utc::now() - chrono::Duration::seconds(5);
        assert!(quote_expired(&q, Utc::now()));
    }

    #[test]
    fn transfer_matches_cnv_prefix_swap() {
        let t = parse_transfer(&serde_json::from_str(TRANSFER).unwrap()).unwrap();
        assert_eq!(t.id, "TRF_ZDymeHnRJ4VM");
        let tx: Value = serde_json::from_str(TXS).unwrap();
        assert!(transfer_matches_tx(&t, &tx));
    }
}
