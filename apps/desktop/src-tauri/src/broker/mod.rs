use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::settings::AppSettings;
use crate::wealth::{
    CachedWealthBook, TradingMode, WealthClient, WealthFee, WealthHolding, WealthOrder,
    WealthPortfolioSnapshot, WealthProfileStatus, WealthSyncService, WealthWallet,
};

const BAMBOO_UNAVAILABLE: &str = "Bamboo is not connected yet";

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
            available: false,
            connected: false,
        },
    ]
}

/// Live session for the selected broker. Bamboo is never live this phase.
pub fn open_live_broker(settings: &AppSettings) -> Option<BrokerSession> {
    match BrokerId::parse(&settings.selected_broker) {
        BrokerId::Wealth if settings.wealth_connected => {
            let password = crate::secrets::get_secret(crate::secrets::SECRET_WEALTH_PASSWORD)
                .ok()
                .flatten();
            Some(BrokerSession::Wealth(WealthClient::from_settings(
                settings, password,
            )))
        }
        BrokerId::Bamboo | BrokerId::Wealth => None,
    }
}

pub enum BrokerSession {
    Wealth(WealthClient),
    #[allow(dead_code)]
    Bamboo,
}

impl BrokerSession {
    pub fn id(&self) -> BrokerId {
        match self {
            Self::Wealth(_) => BrokerId::Wealth,
            Self::Bamboo => BrokerId::Bamboo,
        }
    }

    pub fn display_name(&self) -> &'static str {
        self.id().display_name()
    }

    pub async fn resolve_trading_mode(&self, settings: &AppSettings) -> TradingMode {
        match self {
            Self::Wealth(client) => client.resolve_trading_mode(settings).await,
            Self::Bamboo => TradingMode::Sandbox,
        }
    }

    pub async fn profile_status(&self, settings: &AppSettings) -> WealthProfileStatus {
        match self {
            Self::Wealth(client) => client.profile_status(settings).await,
            Self::Bamboo => WealthProfileStatus {
                ok: false,
                connected: false,
                email: None,
                trading_profile: None,
                trading_verified: false,
                trading_mode: TradingMode::Sandbox.as_str().into(),
                brokerage_balance: None,
                available_balance: None,
                current_balance: None,
                base_url: String::new(),
                message: BAMBOO_UNAVAILABLE.into(),
                display_name: None,
            },
        }
    }

    pub async fn refresh_book(&self, db: &Database) -> Result<BrokerBook> {
        match self {
            Self::Wealth(client) => WealthSyncService::refresh(db, client).await,
            Self::Bamboo => Err(anyhow!(BAMBOO_UNAVAILABLE)),
        }
    }

    pub fn load_book(&self, db: &Database) -> Result<Option<BrokerBook>> {
        match self {
            Self::Wealth(_) => WealthSyncService::load(db),
            Self::Bamboo => Ok(None),
        }
    }

    pub async fn market_is_open(&self) -> Result<bool> {
        match self {
            Self::Wealth(client) => client.market_is_open().await,
            Self::Bamboo => Err(anyhow!(BAMBOO_UNAVAILABLE)),
        }
    }

    pub async fn resolve_instrument(&self, symbol: &str) -> Result<(i64, f64)> {
        match self {
            Self::Wealth(client) => client.resolve_stock_id(symbol).await,
            Self::Bamboo => Err(anyhow!(BAMBOO_UNAVAILABLE)),
        }
    }

    pub async fn get_wallet(&self) -> Result<WealthWallet> {
        match self {
            Self::Wealth(client) => client.get_wallet().await,
            Self::Bamboo => Err(anyhow!(BAMBOO_UNAVAILABLE)),
        }
    }

    pub async fn get_portfolio(&self) -> Result<WealthPortfolioSnapshot> {
        match self {
            Self::Wealth(client) => client.get_portfolio().await,
            Self::Bamboo => Err(anyhow!(BAMBOO_UNAVAILABLE)),
        }
    }

    pub async fn calculate_fee(
        &self,
        stock_id: i64,
        quantity: f64,
        price: f64,
    ) -> Result<WealthFee> {
        match self {
            Self::Wealth(client) => client.calculate_fee(stock_id, quantity, price).await,
            Self::Bamboo => Err(anyhow!(BAMBOO_UNAVAILABLE)),
        }
    }

    pub async fn place_and_await_fill(
        &self,
        stock_id: i64,
        side: &str,
        quantity: f64,
        client_order_id: Option<&str>,
    ) -> Result<WealthOrder> {
        match self {
            Self::Wealth(client) => {
                client
                    .place_and_await_fill(stock_id, side, quantity, client_order_id)
                    .await
            }
            Self::Bamboo => Err(anyhow!(BAMBOO_UNAVAILABLE)),
        }
    }

    pub async fn get_order(&self, order_id: i64) -> Result<WealthOrder> {
        match self {
            Self::Wealth(client) => client.get_order(order_id).await,
            Self::Bamboo => Err(anyhow!(BAMBOO_UNAVAILABLE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bamboo_is_never_opened_live() {
        let mut settings = AppSettings::default();
        settings.selected_broker = "bamboo".into();
        settings.wealth_connected = true;
        assert!(open_live_broker(&settings).is_none());
    }

    #[test]
    fn wealth_selected_but_disconnected_is_sandbox() {
        let mut settings = AppSettings::default();
        settings.selected_broker = "wealth".into();
        settings.wealth_connected = false;
        assert!(open_live_broker(&settings).is_none());
    }

    #[test]
    fn catalog_marks_bamboo_unavailable() {
        let settings = AppSettings::default();
        let list = catalog(&settings);
        let bamboo = list.iter().find(|b| b.id == BrokerId::Bamboo).unwrap();
        assert!(!bamboo.available);
        assert!(!bamboo.connected);
    }
}
