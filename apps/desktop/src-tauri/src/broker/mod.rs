use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::bamboo::{BambooClient, BambooFeeQuote, BambooSyncService};
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
}

impl BrokerId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wealth => "wealth",
            Self::Bamboo => "bamboo",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Wealth => "Coronation Wealth",
            Self::Bamboo => "Bamboo",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Self::Wealth => "Wealth",
            Self::Bamboo => "Bamboo",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "bamboo" => Self::Bamboo,
            _ => Self::Wealth,
        }
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
}

pub fn catalog(settings: &AppSettings) -> Vec<BrokerListItem> {
    vec![
        BrokerListItem {
            id: BrokerId::Wealth,
            name: BrokerId::Wealth.display_name().into(),
            available: true,
            connected: settings.wealth_connected,
        },
        BrokerListItem {
            id: BrokerId::Bamboo,
            name: BrokerId::Bamboo.display_name().into(),
            available: true,
            connected: settings.bamboo_connected,
        },
    ]
}

/// Which live broker would be opened (does not construct HTTP clients).
pub fn live_broker_id(settings: &AppSettings) -> Option<BrokerId> {
    match BrokerId::parse(&settings.selected_broker) {
        BrokerId::Wealth if settings.wealth_connected => Some(BrokerId::Wealth),
        BrokerId::Bamboo if settings.bamboo_connected => Some(BrokerId::Bamboo),
        _ => None,
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
    }
}

pub enum BrokerSession {
    Wealth(WealthClient),
    Bamboo(BambooClient),
}

impl BrokerSession {
    pub fn id(&self) -> BrokerId {
        match self {
            Self::Wealth(_) => BrokerId::Wealth,
            Self::Bamboo(_) => BrokerId::Bamboo,
        }
    }

    pub fn display_name(&self) -> &'static str {
        self.id().display_name()
    }

    pub async fn resolve_trading_mode(&self, settings: &AppSettings) -> TradingMode {
        match self {
            Self::Wealth(client) => client.resolve_trading_mode(settings).await,
            Self::Bamboo(client) => client.resolve_trading_mode(settings).await,
        }
    }

    pub async fn profile_status(&self, settings: &AppSettings) -> WealthProfileStatus {
        match self {
            Self::Wealth(client) => client.profile_status(settings).await,
            Self::Bamboo(client) => client.profile_status(settings).await,
        }
    }

    pub async fn refresh_book(&self, db: &Database) -> Result<BrokerBook> {
        match self {
            Self::Wealth(client) => WealthSyncService::refresh(db, client).await,
            Self::Bamboo(client) => BambooSyncService::refresh(db, client).await,
        }
    }

    pub fn load_book(&self, db: &Database) -> Result<Option<BrokerBook>> {
        match self {
            Self::Wealth(_) => WealthSyncService::load(db),
            Self::Bamboo(_) => BambooSyncService::load(db),
        }
    }

    pub async fn market_is_open(&self) -> Result<bool> {
        match self {
            Self::Wealth(client) => client.market_is_open().await,
            Self::Bamboo(client) => client.market_is_open().await,
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
        }
    }

    pub async fn get_wallet(&self) -> Result<WealthWallet> {
        match self {
            Self::Wealth(client) => client.get_wallet().await,
            Self::Bamboo(client) => client.get_wallet().await,
        }
    }

    pub async fn get_portfolio(&self) -> Result<WealthPortfolioSnapshot> {
        match self {
            Self::Wealth(client) => client.get_portfolio().await,
            Self::Bamboo(client) => client.get_portfolio().await,
        }
    }

    pub async fn calculate_fee(
        &self,
        instrument: &BrokerInstrument,
        side: &str,
        quantity: f64,
        price: f64,
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
) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO broker_orders (
            id, signal_id, symbol, side, quantity, external_order_id, external_order_ref, stock_id,
            status, fill_price, fee, rejection_reason, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, datetime('now'), datetime('now'))",
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
        let mut settings = AppSettings::default();
        settings.bamboo_connected = true;
        let list = catalog(&settings);
        let bamboo = list.iter().find(|b| b.id == BrokerId::Bamboo).unwrap();
        assert!(bamboo.available);
        assert!(bamboo.connected);
    }
}
