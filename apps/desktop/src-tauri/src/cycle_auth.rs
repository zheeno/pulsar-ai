use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use uuid::Uuid;

const TOKEN_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct LiveConfirmation {
    pub token: String,
    pub cycle_id: String,
    pub venue: String,
    pub strategy_hash: String,
    pub signal_ids: Vec<String>,
    pub action_count: usize,
    pub allow_bulk_liquidation: bool,
    pub expires_at: Instant,
}

#[derive(Default)]
pub struct LiveIntentStore {
    inner: Mutex<HashMap<String, LiveConfirmation>>,
}

impl LiveIntentStore {
    pub fn issue(
        &self,
        cycle_id: String,
        venue: String,
        strategy_hash: String,
        signal_ids: Vec<String>,
        allow_bulk_liquidation: bool,
    ) -> LiveConfirmation {
        self.purge_expired();
        let token = Uuid::new_v4().to_string();
        let confirm = LiveConfirmation {
            token: token.clone(),
            cycle_id,
            venue,
            strategy_hash,
            action_count: signal_ids.len(),
            signal_ids,
            allow_bulk_liquidation,
            expires_at: Instant::now() + TOKEN_TTL,
        };
        self.inner.lock().insert(token, confirm.clone());
        confirm
    }

    pub fn consume(
        &self,
        token: &str,
        venue: &str,
        strategy_hash: &str,
        allow_bulk_liquidation: bool,
    ) -> Result<LiveConfirmation> {
        self.purge_expired();
        let mut guard = self.inner.lock();
        let confirm = guard
            .remove(token)
            .ok_or_else(|| anyhow!("Live confirmation token is invalid or already used"))?;
        if Instant::now() > confirm.expires_at {
            return Err(anyhow!("Live confirmation token expired"));
        }
        if confirm.venue != venue {
            return Err(anyhow!("Live confirmation venue mismatch"));
        }
        if confirm.strategy_hash != strategy_hash {
            return Err(anyhow!("Live confirmation strategy mismatch"));
        }
        if confirm.allow_bulk_liquidation != allow_bulk_liquidation {
            return Err(anyhow!("Live confirmation bulk-liquidation flag mismatch"));
        }
        Ok(confirm)
    }

    fn purge_expired(&self) {
        let now = Instant::now();
        self.inner.lock().retain(|_, c| c.expires_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_replay_and_expiry() {
        let store = LiveIntentStore::default();
        let issued = store.issue(
            "c1".into(),
            "wealth".into(),
            "hash".into(),
            vec!["s1".into()],
            false,
        );
        store
            .consume(&issued.token, "wealth", "hash", false)
            .unwrap();
        assert!(store
            .consume(&issued.token, "wealth", "hash", false)
            .is_err());
    }

    #[test]
    fn rejects_venue_mismatch() {
        let store = LiveIntentStore::default();
        let issued = store.issue("c1".into(), "wealth".into(), "hash".into(), vec![], false);
        assert!(store
            .consume(&issued.token, "sandbox", "hash", false)
            .is_err());
    }
}
