use std::sync::Arc;

use crate::agent::AgentBridge;
use crate::cache::PriceCache;
use crate::db::Database;

pub struct AppState {
    pub db: Database,
    pub cache: PriceCache,
    pub agent: AgentBridge,
}

impl AppState {
    pub fn new(db: Database) -> Arc<Self> {
        Arc::new(Self {
            db,
            cache: PriceCache::new(2400),
            agent: AgentBridge::new(crate::agent::resolve_worker_path()),
        })
    }
}
