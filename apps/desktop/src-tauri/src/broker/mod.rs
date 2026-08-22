use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::bamboo::{BambooClient, BambooFeeQuote, BambooSyncService};
use crate::busha::{BushaClient, BushaFeeQuote, BushaSyncService};
use crate::db::Database;
use crate::settings::AppSettings;
use crate::wealth::{
    CachedWealthBook, TradingMode, WealthClient, WealthHolding, WealthOrder, WealthPortfolioSnapshot,
    WealthProfileStatus, WealthSyncService, WealthWallet,
};

pub type BrokerHolding = WealthHolding;
pub type BrokerBook = CachedWealthBook;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrokerId {
    Wealth,
    Bamboo,
    Busha,
}

impl BrokerId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wealth => "wealth",
            Self::Bamboo => "bamboo",
            Self::Busha => "busha",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Wealth => "Coronation Wealth",
            Self::Bamboo => "Bamboo",
            Self::Busha => "Busha",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Self::Wealth => "Wealth",
            Self::Bamboo => "Bamboo",
            Self::Busha => "Busha",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "bamboo" => Self::Bamboo,
            "busha" => Self::Busha,
            _ => Self::Wealth,
        }
    }

    pub fn whole_shares(self) -> bool {
        !matches!(self, Self::Busha)
    }
}

pub fn whole_shares_for_venue(venue: &str) -> bool {
    !venue.eq_ignore_ascii_case("busha")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssetClass {
    Stocks,
    Crypto,
}

impl AssetClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stocks => "stocks",
            Self::Crypto => "crypto",
        }
    }
}

/// Soft session for Busha: expiry must not silently demote the app to NGX sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CryptoSession {
    /// Fresh JWT — live Busha trading allowed.
    Connected,
    /// Cred still stored but JWT expired / marked dead — crypto UI, reconnect required.
    ExpiredNeedsReconnect,
    /// User disconnected or never connected.
    Disconnected,
}

impl CryptoSession {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::ExpiredNeedsReconnect => "expiredNeedsReconnect",
            Self::Disconnected => "disconnected",
        }
    }

    pub fn is_crypto_mode(self) -> bool {
        !matches!(self, Self::Disconnected)
    }

    pub fn can_trade(self) -> bool {
        matches!(self, Self::Connected)
    }
}

/// Derive crypto session from Auth Bridge Busha credential freshness.
pub fn crypto_session() -> CryptoSession {
    match crate::auth_bridge::store::get("busha") {
        Ok(Some(cred))
            if crate::secrets::token_is_fresh(
                cred.expires_at,
                crate::secrets::TOKEN_EXPIRY_SKEW_SECS,
            ) =>
        {
            CryptoSession::Connected
        }
        Ok(Some(_)) => CryptoSession::ExpiredNeedsReconnect,
        _ => CryptoSession::Disconnected,
    }
}

fn busha_token_fresh() -> bool {
    matches!(crypto_session(), CryptoSession::Connected)
}

/// Fresh Busha JWT present — live HTTP / order routing allowed.
pub fn busha_connected() -> bool {
    busha_token_fresh()
}

/// Crypto product mode (connected or soft-expired). Used for asset class / 24h market gates.
pub fn crypto_mode() -> bool {
    crypto_session().is_crypto_mode()
}

pub fn crypto_reconnect_required() -> bool {
    matches!(crypto_session(), CryptoSession::ExpiredNeedsReconnect)
}

pub fn asset_class() -> AssetClass {
    if crypto_mode() {
        AssetClass::Crypto
    } else {
        AssetClass::Stocks
    }
}

pub fn is_valid_trade_symbol(venue: &str, symbol: &str) -> bool {
    if venue.eq_ignore_ascii_case("busha") {
        let s = symbol.trim().to_uppercase();
        (2..=16).contains(&s.len())
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    } else {
        crate::ngx::is_valid_ticker(symbol)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerListItem {
    pub id: BrokerId,
    pub name: String,
    pub available: bool,
    pub connected: bool,
}

#[derive(Debug, Clone)]
pub struct BrokerInstrument {
    pub symbol: String,
    pub quote: f64,
    pub stock_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct BrokerFeeQuote {
    pub fee: f64,
    pub quantity: f64,
    pub price_per_share: f64,
    pub total_price: f64,
    pub available_quantity: f64,
    pub stock_id: Option<i64>,
    pub symbol: String,
    pub bamboo: Option<BambooFeeQuote>,
    pub busha: Option<BushaFeeQuote>,
}

#[derive(Debug, Clone)]
pub struct BrokerOrder {
    pub id: String,
    pub numeric_id: Option<i64>,
    pub stock_id: Option<i64>,
    pub status: String,
    pub quantity: f64,
    pub quote_price: Option<f64>,
    pub unit_price: Option<f64>,
    pub rejection_reason: Option<String>,
}

impl BrokerOrder {
    fn from_wealth(order: WealthOrder) -> Self {
        Self {
            id: order.id.to_string(),
            numeric_id: Some(order.id),
            stock_id: Some(order.stock_id),
            status: order.status,
            quantity: order.quantity,
            quote_price: order.quote_price,
            unit_price: order.unit_price,
            rejection_reason: order.rejection_reason,
        }
    }

    fn from_busha(transfer: &crate::busha::BushaTransfer, quote: &BrokerFeeQuote) -> Self {
        let status = if transfer.status.eq_ignore_ascii_case("failed")
            || transfer.status.eq_ignore_ascii_case("rejected")
        {
            "rejected".into()
        } else {
            "executed".into()
        };
        Self {
            id: transfer.id.clone(),
            numeric_id: None,
            stock_id: None,
            status,
            quantity: quote.quantity,
            quote_price: Some(quote.price_per_share),
            unit_price: Some(quote.price_per_share),
            rejection_reason: None,
        }
    }
}

pub fn catalog(settings: &AppSettings) -> Vec<BrokerListItem> {
    vec![
        BrokerListItem {
            id: BrokerId::Wealth,
            name: BrokerId::Wealth.display_name().into(),
            available: true,
            connected: settings.wealth_connected
                && crate::secrets::secret_present(crate::secrets::SECRET_WEALTH_TOKEN),
        },
        BrokerListItem {
            id: BrokerId::Bamboo,
            name: BrokerId::Bamboo.display_name().into(),
            available: true,
            connected: settings.bamboo_connected
                && crate::secrets::secret_present(crate::secrets::SECRET_BAMBOO_TOKEN),
        },
        // Busha is Auth Bridge only — never selected_broker.
    ]
}

/// Which live broker would be opened (does not construct HTTP clients).
pub fn live_broker_id(settings: &AppSettings) -> Option<BrokerId> {
    match crypto_session() {
        CryptoSession::Connected => Some(BrokerId::Busha),
        // Soft-expired: stay in crypto mode, but do not open NGX brokers underneath.
        CryptoSession::ExpiredNeedsReconnect => None,
        CryptoSession::Disconnected => match BrokerId::parse(&settings.selected_broker) {
            BrokerId::Wealth if settings.wealth_connected => Some(BrokerId::Wealth),
            BrokerId::Bamboo if settings.bamboo_connected => Some(BrokerId::Bamboo),
            BrokerId::Busha => None,
            _ => None,
        },
    }
}

pub fn open_live_broker(settings: &AppSettings) -> Option<BrokerSession> {
    match live_broker_id(settings)? {
        BrokerId::Wealth => {
            let password = crate::secrets::get_secret(crate::secrets::SECRET_WEALTH_PASSWORD)
                .ok()
                .flatten();
            Some(BrokerSession::Wealth(WealthClient::from_settings(
                settings, password,
            )))
        }
        BrokerId::Bamboo => Some(BrokerSession::Bamboo(BambooClient::from_settings(settings))),
        BrokerId::Busha => BushaClient::from_store().ok().map(BrokerSession::Busha),
    }
}

pub enum BrokerSession {
    Wealth(WealthClient),
    Bamboo(BambooClient),
    Busha(BushaClient),
}

impl BrokerSession {
    pub fn id(&self) -> BrokerId {
        match self {
            Self::Wealth(_) => BrokerId::Wealth,
            Self::Bamboo(_) => BrokerId::Bamboo,
            Self::Busha(_) => BrokerId::Busha,
        }
    }

    pub fn display_name(&self) -> &'static str {
        self.id().display_name()
    }

    pub async fn resolve_trading_mode(&self, settings: &AppSettings) -> TradingMode {
        match self {
            Self::Wealth(client) => client.resolve_trading_mode(settings).await,
            Self::Bamboo(client) => client.resolve_trading_mode(settings).await,
            Self::Busha(client) => client.resolve_trading_mode(settings).await,
        }
    }

    pub async fn profile_status(&self, settings: &AppSettings) -> WealthProfileStatus {
        match self {
            Self::Wealth(client) => client.profile_status(settings).await,
            Self::Bamboo(client) => client.profile_status(settings).await,
            Self::Busha(client) => client.profile_status(settings).await,
        }
    }

    pub async fn refresh_book(&self, db: &Database) -> Result<BrokerBook> {
        match self {
            Self::Wealth(client) => WealthSyncService::refresh(db, client).await,
            Self::Bamboo(client) => BambooSyncService::refresh(db, client).await,
            Self::Busha(client) => BushaSyncService::refresh(db, client).await,
        }
    }

    pub fn load_book(&self, db: &Database) -> Result<Option<BrokerBook>> {
        match self {
            Self::Wealth(_) => WealthSyncService::load(db),
            Self::Bamboo(_) => BambooSyncService::load(db),
            Self::Busha(_) => BushaSyncService::load(db),
        }
    }

    pub async fn market_is_open(&self) -> Result<bool> {
        match self {
            Self::Wealth(client) => client.market_is_open().await,
            Self::Bamboo(client) => client.market_is_open().await,
            Self::Busha(client) => Ok(client.market_is_open()?),
        }
    }

    pub async fn resolve_instrument(&self, symbol: &str) -> Result<BrokerInstrument> {
        match self {
            Self::Wealth(client) => {
                let (stock_id, quote) = client.resolve_stock_id(symbol).await?;
                Ok(BrokerInstrument {
                    symbol: symbol.to_string(),
                    quote,
                    stock_id: Some(stock_id),
                })
            }
            Self::Bamboo(client) => {
                let quote = client.resolve_quote(symbol).await?;
                Ok(BrokerInstrument {
                    symbol: symbol.to_uppercase(),
                    quote,
                    stock_id: None,
                })
            }
            Self::Busha(client) => {
                let quote = client.resolve_quote(symbol).await?;
                Ok(BrokerInstrument {
                    symbol: symbol.to_uppercase(),
                    quote,
                    stock_id: None,
                })
            }
        }
    }

    pub async fn resolve_instrument_for_side(
        &self,
        symbol: &str,
        side: &str,
    ) -> Result<BrokerInstrument> {
        match self {
            Self::Busha(client) => {
                let quote = client.resolve_quote_for(symbol, side).await?;
                Ok(BrokerInstrument {
                    symbol: symbol.to_uppercase(),
                    quote,
                    stock_id: None,
                })
            }
            _ => self.resolve_instrument(symbol).await,
        }
    }

    pub async fn get_wallet(&self) -> Result<WealthWallet> {
        match self {
            Self::Wealth(client) => client.get_wallet().await,
            Self::Bamboo(client) => client.get_wallet().await,
            Self::Busha(client) => client.get_wallet().await,
        }
    }

    pub async fn get_portfolio(&self) -> Result<WealthPortfolioSnapshot> {
        match self {
            Self::Wealth(client) => client.get_portfolio().await,
            Self::Bamboo(client) => client.get_portfolio().await,
            Self::Busha(client) => client.get_portfolio().await,
        }
    }

    pub async fn calculate_fee(
        &self,
        instrument: &BrokerInstrument,
        side: &str,
        quantity: f64,
        price: f64,
        sell_full_lot: bool,
    ) -> Result<BrokerFeeQuote> {
        match self {
            Self::Wealth(client) => {
                let stock_id = instrument
                    .stock_id
                    .ok_or_else(|| anyhow!("Wealth instrument missing stock_id"))?;
                let fee = client.calculate_fee(stock_id, quantity, price).await?;
                Ok(BrokerFeeQuote {
                    fee: fee.rounded_fee,
                    quantity,
                    price_per_share: price,
                    total_price: price * quantity + fee.rounded_fee,
                    available_quantity: f64::MAX,
                    stock_id: Some(stock_id),
                    symbol: instrument.symbol.clone(),
                    bamboo: None,
                    busha: None,
                })
            }
            Self::Bamboo(client) => {
                let calc = client
                    .calculate_fee(&instrument.symbol, side, quantity, price)
                    .await?;
                Ok(BrokerFeeQuote {
                    fee: calc.fee,
                    quantity: calc.quantity,
                    price_per_share: calc.price_per_share,
                    total_price: calc.total_price,
                    available_quantity: calc.available_quantity,
                    stock_id: None,
                    symbol: calc.symbol.clone(),
                    bamboo: Some(calc),
                    busha: None,
                })
            }
            Self::Busha(client) => {
                let (fee, busha) = client
                    .calculate_fee(&instrument.symbol, side, quantity, price, sell_full_lot)
                    .await?;
                Ok(BrokerFeeQuote {
                    fee: fee.fee,
                    quantity: fee.quantity,
                    price_per_share: fee.price_per_share,
                    total_price: fee.total_price,
                    available_quantity: fee.available_quantity,
                    stock_id: None,
                    symbol: instrument.symbol.clone(),
                    bamboo: None,
                    busha: Some(busha),
                })
            }
        }
    }

    pub async fn place_and_await_fill(
        &self,
        instrument: &BrokerInstrument,
        side: &str,
        quote: &BrokerFeeQuote,
        client_order_id: Option<&str>,
    ) -> Result<BrokerOrder> {
        match self {
            Self::Wealth(client) => {
                let stock_id = instrument
                    .stock_id
                    .or(quote.stock_id)
                    .ok_or_else(|| anyhow!("Wealth instrument missing stock_id"))?;
                let order = client
                    .place_and_await_fill(stock_id, side, quote.quantity, client_order_id)
                    .await?;
                Ok(BrokerOrder::from_wealth(order))
            }
            Self::Bamboo(client) => {
                let calc = quote
                    .bamboo
                    .as_ref()
                    .ok_or_else(|| anyhow!("Bamboo place requires calculate quote"))?;
                client.place_and_await_fill(calc).await
            }
            Self::Busha(client) => {
                let calc = quote
                    .busha
                    .as_ref()
                    .ok_or_else(|| anyhow!("Busha place requires a live quote"))?;
                let transfer = client.place_and_await_fill(calc, &instrument.symbol).await?;
                Ok(BrokerOrder::from_busha(&transfer, quote))
            }
        }
    }

    /// After a full-lot crypto exit, sell any tradable remainder left in the wallet.
    pub async fn try_sweep_crypto_remainder(&self, symbol: &str, price: f64) -> Result<()> {
        match self {
            Self::Busha(client) => {
                let _ = client.try_sweep_crypto_remainder(symbol, price).await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub async fn get_order(&self, order_id: &str) -> Result<BrokerOrder> {
        match self {
            Self::Wealth(client) => {
                let id = order_id
                    .parse::<i64>()
                    .map_err(|_| anyhow!("Wealth order id must be numeric"))?;
                Ok(BrokerOrder::from_wealth(client.get_order(id).await?))
            }
            Self::Bamboo(client) => client.get_order(order_id).await,
            Self::Busha(_) => Ok(BrokerOrder {
                id: order_id.to_string(),
                numeric_id: None,
                stock_id: None,
                status: "executed".into(),
                quantity: 0.0,
                quote_price: None,
                unit_price: None,
                rejection_reason: None,
            }),
        }
    }
}

pub fn insert_broker_order(
    conn: &rusqlite::Connection,
    signal_id: Option<&str>,
    symbol: &str,
    side: &str,
    quantity: f64,
    order: &BrokerOrder,
    fill_price: Option<f64>,
    fee: Option<f64>,
    venue: &str,
) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO broker_orders (
            id, signal_id, symbol, side, quantity, external_order_id, external_order_ref, stock_id,
            status, fill_price, fee, rejection_reason, created_at, updated_at, venue
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, datetime('now'), datetime('now'), ?13)",
        rusqlite::params![
            id,
            signal_id,
            symbol,
            side,
            quantity,
            order.numeric_id,
            order.id,
            order.stock_id,
            order.status,
            fill_price.or(order.unit_price).or(order.quote_price),
            fee,
            order.rejection_reason,
            venue,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bamboo_opens_when_selected_and_connected() {
        let mut settings = AppSettings::default();
        settings.selected_broker = "bamboo".into();
        settings.bamboo_connected = true;
        settings.wealth_connected = true;
        assert_eq!(live_broker_id(&settings), Some(BrokerId::Bamboo));
    }

    #[test]
    fn bamboo_disconnected_is_sandbox() {
        let mut settings = AppSettings::default();
        settings.selected_broker = "bamboo".into();
        settings.bamboo_connected = false;
        assert!(live_broker_id(&settings).is_none());
        assert!(open_live_broker(&settings).is_none());
    }

    #[test]
    fn wealth_selected_but_disconnected_is_sandbox() {
        let mut settings = AppSettings::default();
        settings.selected_broker = "wealth".into();
        settings.wealth_connected = false;
        assert!(live_broker_id(&settings).is_none());
    }

    #[test]
    fn catalog_marks_bamboo_available() {
        let _g = crate::secrets::vault_test_guard();
        crate::secrets::seed_vault(&[(crate::secrets::SECRET_BAMBOO_TOKEN, "live-jwt")]);
        let mut settings = AppSettings::default();
        settings.bamboo_connected = true;
        let list = catalog(&settings);
        let bamboo = list.iter().find(|b| b.id == BrokerId::Bamboo).unwrap();
        assert!(bamboo.available);
        assert!(bamboo.connected);
    }

    #[test]
    fn catalog_connected_requires_vault_token() {
        let _g = crate::secrets::vault_test_guard();
        crate::secrets::seed_vault(&[]);
        let mut settings = AppSettings::default();
        settings.bamboo_connected = true;
        settings.wealth_connected = true;
        let list = catalog(&settings);
        let bamboo = list.iter().find(|b| b.id == BrokerId::Bamboo).unwrap();
        let wealth = list.iter().find(|b| b.id == BrokerId::Wealth).unwrap();
        assert!(bamboo.available);
        assert!(!bamboo.connected);
        assert!(!wealth.connected);
    }

    #[test]
    fn busha_session_selects_crypto_over_ngx() {
        let _ = crate::auth_bridge::store::delete("busha");
        let mut settings = AppSettings::default();
        settings.selected_broker = "bamboo".into();
        settings.bamboo_connected = true;
        settings.wealth_connected = true;
        assert_eq!(live_broker_id(&settings), Some(BrokerId::Bamboo));
        assert_eq!(asset_class(), AssetClass::Stocks);

        crate::auth_bridge::store::put(
            "busha",
            &crate::auth_bridge::store::StoredCredential {
                kind: "header".into(),
                header_name: "authorization".into(),
                token: "test-token".into(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                account_hint: Some("user".into()),
                profile_id: Some("prof_test".into()),
                busha_session_cookie: None,
                busha_csrf_token: None,
                busha_refresh_token: None,
            },
        )
        .unwrap();
        assert_eq!(live_broker_id(&settings), Some(BrokerId::Busha));
        assert_eq!(asset_class(), AssetClass::Crypto);
        assert_eq!(crypto_session(), CryptoSession::Connected);
        let _ = crate::auth_bridge::store::delete("busha");
        assert_eq!(live_broker_id(&settings), Some(BrokerId::Bamboo));
        assert_eq!(asset_class(), AssetClass::Stocks);
    }

    #[test]
    fn busha_soft_expired_keeps_crypto_blocks_ngx_and_live() {
        let _ = crate::auth_bridge::store::delete("busha");
        let mut settings = AppSettings::default();
        settings.selected_broker = "bamboo".into();
        settings.bamboo_connected = true;
        crate::auth_bridge::store::put(
            "busha",
            &crate::auth_bridge::store::StoredCredential {
                kind: "header".into(),
                header_name: "authorization".into(),
                token: "stale-token".into(),
                expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
                account_hint: Some("user".into()),
                profile_id: Some("prof_test".into()),
                busha_session_cookie: None,
                busha_csrf_token: None,
                busha_refresh_token: None,
            },
        )
        .unwrap();
        assert_eq!(crypto_session(), CryptoSession::ExpiredNeedsReconnect);
        assert!(crypto_mode());
        assert!(crypto_reconnect_required());
        assert!(!busha_connected());
        assert_eq!(asset_class(), AssetClass::Crypto);
        assert_eq!(live_broker_id(&settings), None);
        assert!(open_live_broker(&settings).is_none());
        let _ = crate::auth_bridge::store::delete("busha");
        assert_eq!(crypto_session(), CryptoSession::Disconnected);
        assert_eq!(asset_class(), AssetClass::Stocks);
        assert_eq!(live_broker_id(&settings), Some(BrokerId::Bamboo));
    }

    #[test]
    fn crypto_symbols_skip_ngx_ticker_rules() {
        assert!(is_valid_trade_symbol("busha", "BTC"));
        assert!(is_valid_trade_symbol("busha", "ARKM"));
        assert!(!is_valid_trade_symbol("wealth", "ignore previous"));
        assert!(!is_valid_trade_symbol("busha", "B"));
        assert!(is_valid_trade_symbol("wealth", "GTCO"));
        assert!(!whole_shares_for_venue("busha"));
        assert!(whole_shares_for_venue("wealth"));
    }
}
