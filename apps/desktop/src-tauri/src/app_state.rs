use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::path::PathBuf;

use crate::agent::AgentBridge;
use crate::cache::PriceCache;
use crate::db::Database;

pub struct AppState {
    pub db: Database,
    pub cache: PriceCache,
    pub agent: AgentBridge,
    cycle_running: AtomicBool,
}

impl AppState {
    pub fn new(db: Database, worker_path: PathBuf, node_bin: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            db,
            cache: PriceCache::new(2400),
            agent: AgentBridge::new(worker_path, node_bin),
            cycle_running: AtomicBool::new(false),
        })
    }

    pub fn try_begin_cycle(&self) -> bool {
        self.cycle_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn end_cycle(&self) {
        self.cycle_running.store(false, Ordering::SeqCst);
    }

    pub fn is_cycle_running(&self) -> bool {
        self.cycle_running.load(Ordering::SeqCst)
    }
}

/// Clears the cycle gate when dropped (panic-safe).
pub struct CycleGateGuard {
    state: Arc<AppState>,
}

impl CycleGateGuard {
    pub fn acquire(state: Arc<AppState>) -> Option<Self> {
        if state.try_begin_cycle() {
            Some(Self { state })
        } else {
            None
        }
    }
}

impl Drop for CycleGateGuard {
    fn drop(&mut self) {
        self.state.end_cycle();
    }
}
