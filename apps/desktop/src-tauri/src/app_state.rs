use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::agent::AgentBridge;
use crate::cache::PriceCache;
use crate::db::Database;

pub struct AppState {
    pub db: Database,
    pub cache: PriceCache,
    pub agent: AgentBridge,
    /// True only while a user/scheduler trading cycle holds `CycleGateGuard`.
    cycle_running: AtomicBool,
    /// Shared exclusion for cycle + dream + risk so they do not mutate the book together.
    book_busy: AtomicBool,
}

impl AppState {
    pub fn new(db: Database, worker_path: PathBuf, node_bin: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            db,
            cache: PriceCache::new(2400),
            agent: AgentBridge::new(worker_path, node_bin),
            cycle_running: AtomicBool::new(false),
            book_busy: AtomicBool::new(false),
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

    pub fn try_begin_book_busy(&self) -> bool {
        self.book_busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn end_book_busy(&self) {
        self.book_busy.store(false, Ordering::SeqCst);
    }

    pub fn is_book_busy(&self) -> bool {
        self.book_busy.load(Ordering::SeqCst)
    }
}

/// Mutual exclusion for dream / risk / trading cycle book writes.
pub struct BookBusyGuard {
    state: Arc<AppState>,
}

impl BookBusyGuard {
    pub fn acquire(state: Arc<AppState>) -> Option<Self> {
        if state.try_begin_book_busy() {
            Some(Self { state })
        } else {
            None
        }
    }
}

impl Drop for BookBusyGuard {
    fn drop(&mut self) {
        self.state.end_book_busy();
    }
}

/// Trading cycle gate: holds the book lock and the UI `cycle_running` flag.
pub struct CycleGateGuard {
    _book: BookBusyGuard,
    state: Arc<AppState>,
}

impl CycleGateGuard {
    pub fn acquire(state: Arc<AppState>) -> Option<Self> {
        let book = BookBusyGuard::acquire(state.clone())?;
        if !state.try_begin_cycle() {
            return None;
        }
        Some(Self { _book: book, state })
    }
}

impl Drop for CycleGateGuard {
    fn drop(&mut self) {
        self.state.end_cycle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("pulsar-gate-{}", uuid::Uuid::new_v4()));
        let db = Database::open(&dir).expect("db");
        AppState::new(db, PathBuf::from("worker"), PathBuf::from("node"))
    }

    #[test]
    fn risk_lock_does_not_look_like_a_trading_cycle() {
        let s = state();
        let _risk = BookBusyGuard::acquire(s.clone()).unwrap();
        assert!(s.is_book_busy());
        assert!(!s.is_cycle_running());
        assert!(CycleGateGuard::acquire(s.clone()).is_none());
    }

    #[test]
    fn trading_cycle_sets_ui_flag_and_blocks_book() {
        let s = state();
        let cycle = CycleGateGuard::acquire(s.clone()).unwrap();
        assert!(s.is_cycle_running());
        assert!(s.is_book_busy());
        assert!(BookBusyGuard::acquire(s.clone()).is_none());
        drop(cycle);
        assert!(!s.is_cycle_running());
        assert!(!s.is_book_busy());
    }
}
