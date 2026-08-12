use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPrice {
    pub symbol: String,
    pub price: f64,
    pub trade_date: String,
    pub updated_at: String,
}

struct CacheEntry {
    value: String,
    expires: Instant,
}

pub struct PriceCache {
    inner: Mutex<HashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl PriceCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub fn set_price(&self, entry: &CachedPrice) {
        let json = serde_json::to_string(entry).unwrap_or_default();
        let mut map = self.inner.lock();
        map.insert(
            format!("price:{}", entry.symbol),
            CacheEntry {
                value: json,
                expires: Instant::now() + self.ttl,
            },
        );
    }

    pub fn get_price(&self, symbol: &str) -> Option<CachedPrice> {
        let key = format!("price:{symbol}");
        let mut map = self.inner.lock();
        if let Some(entry) = map.get(&key) {
            if entry.expires > Instant::now() {
                return serde_json::from_str(&entry.value).ok();
            }
            map.remove(&key);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_cache_ttl() {
        let cache = PriceCache::new(1);
        cache.set_price(&CachedPrice {
            symbol: "GTCO".into(),
            price: 46.0,
            trade_date: "2025-01-01".into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        });
        assert!(cache.get_price("GTCO").is_some());
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(cache.get_price("GTCO").is_none());
    }
}
