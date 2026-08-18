use std::collections::HashSet;

const NGX_HOLIDAYS: &[&str] = &[
    "2025-01-01", "2025-04-18", "2025-04-21", "2025-05-01", "2025-06-12",
    "2025-10-01", "2025-12-25", "2025-12-26",
    "2026-01-01", "2026-04-03", "2026-04-06", "2026-05-01", "2026-06-12",
    "2026-10-01", "2026-12-25", "2026-12-26",
    "2027-01-01", "2027-03-26", "2027-03-29", "2027-05-01", "2027-06-12",
    "2027-10-01", "2027-12-25", "2027-12-26",
    "2028-01-01", "2028-04-14", "2028-04-17", "2028-05-01", "2028-06-12",
    "2028-10-01", "2028-12-25", "2028-12-26",
];

pub struct TradingCalendar {
    holidays: HashSet<String>,
}

impl Default for TradingCalendar {
    fn default() -> Self {
        Self {
            holidays: NGX_HOLIDAYS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl TradingCalendar {
    pub fn to_wat(&self, date: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::FixedOffset> {
        let lagos = chrono::FixedOffset::east_opt(3600).unwrap();
        date.with_timezone(&lagos)
    }

    pub fn now_wat(&self) -> chrono::DateTime<chrono::FixedOffset> {
        self.to_wat(chrono::Utc::now())
    }

    pub fn today_wat(&self) -> String {
        self.now_wat().format("%Y-%m-%d").to_string()
    }

    pub fn is_trading_day(&self, date: Option<chrono::DateTime<chrono::Utc>>) -> bool {
        let wat = match date {
            Some(d) => self.to_wat(d),
            None => self.now_wat(),
        };
        let weekday = wat.weekday().num_days_from_monday();
        if weekday >= 5 {
            return false;
        }
        let date_str = wat.format("%Y-%m-%d").to_string();
        !self.holidays.contains(&date_str)
    }

    pub fn is_market_open(&self) -> bool {
        if !self.is_trading_day(None) {
            return false;
        }
        let wat = self.now_wat();
        let minutes = wat.hour() * 60 + wat.minute();
        minutes >= 9 * 60 && minutes < 16 * 60
    }

    pub fn is_post_close_window(&self) -> bool {
        if !self.is_trading_day(None) {
            return false;
        }
        let wat = self.now_wat();
        let minutes = wat.hour() * 60 + wat.minute();
        minutes >= 16 * 60 && minutes < 16 * 60 + 30
    }
}

use chrono::Datelike;
use chrono::Timelike;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holidays_cover_2027_and_2028() {
        let cal = TradingCalendar::default();
        assert!(cal.holidays.contains("2027-01-01"));
        assert!(cal.holidays.contains("2027-12-25"));
        assert!(cal.holidays.contains("2028-10-01"));
        let ny = chrono::DateTime::parse_from_rfc3339("2027-01-01T10:00:00+01:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(!cal.is_trading_day(Some(ny)));
    }
}
