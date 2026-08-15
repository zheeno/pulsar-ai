use crate::settings::AppSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketModule {
    Stocks,
    Crypto,
}

impl MarketModule {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "crypto" => Self::Crypto,
            _ => Self::Stocks,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stocks => "stocks",
            Self::Crypto => "crypto",
        }
    }

    pub fn from_settings(settings: &AppSettings) -> Self {
        Self::parse(&settings.active_module)
    }

    pub fn sandbox_portfolio_name(self) -> &'static str {
        match self {
            Self::Stocks => "default-sandbox",
            Self::Crypto => "default-crypto-sandbox",
        }
    }

    /// Equity-curve venue key (kept separate so stocks/crypto books never mix).
    pub fn equity_curve_venue(self) -> &'static str {
        match self {
            Self::Stocks => "sandbox",
            Self::Crypto => "crypto-sandbox",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stocks => "Stocks",
            Self::Crypto => "Crypto",
        }
    }

    pub fn allows_live_broker(self) -> bool {
        matches!(self, Self::Stocks)
    }

    pub fn whole_share_lots(self) -> bool {
        matches!(self, Self::Stocks)
    }
}

pub fn normalize_module(value: &str) -> String {
    MarketModule::parse(value).as_str().to_string()
}
