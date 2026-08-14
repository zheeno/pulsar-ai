use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::path::PathBuf;
use std::time::Instant;

use parking_lot::Mutex;

use crate::agent::AgentBridge;
use crate::cache::PriceCache;
use crate::cycle_auth::LiveIntentStore;
use crate::db::Database;

pub struct AppState {
    pub db: Database,
    pub cache: PriceCache,
    pub agent: AgentBridge,
    pub live_intents: LiveIntentStore,
    cycle_running: AtomicBool,
    backtest_running: AtomicBool,
    last_backtest_at: Mutex<Option<Instant>>,
}

impl AppState {
    pub fn new(db: Database, worker_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            db,
            cache: PriceCache::new(2400),
            agent: AgentBridge::new(worker_path),
            live_intents: LiveIntentStore::default(),
            cycle_running: AtomicBool::new(false),
            backtest_running: AtomicBool::new(false),
            last_backtest_at: Mutex::new(None),
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

    pub fn try_begin_backtest(&self) -> bool {
        self.backtest_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn end_backtest(&self) {
        self.backtest_running.store(false, Ordering::SeqCst);
        *self.last_backtest_at.lock() = Some(Instant::now());
    }

    pub fn last_backtest_at(&self) -> Option<Instant> {
        *self.last_backtest_at.lock()
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
