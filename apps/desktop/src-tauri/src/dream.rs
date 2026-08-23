//! Overnight desk review. Compress closed-lot LESSONs into standing pattern
//! cautions. Never places orders, never raises minConfidence, never blacklists
//! tickers, and never calls the LLM.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::Serialize;
use tauri::AppHandle;
use tokio::time::MissedTickBehavior;

use crate::app_state::{AppState, CycleGateGuard};
use crate::calendar::TradingCalendar;
use crate::memory;
use crate::outcomes::{classify_close, hold_hours_as_of, signal_model};
use crate::settings::{get_settings, set_setting};

pub const MIN_CLOSES: usize = 12;
pub const MIN_PATTERN_N: usize = 3;
pub const LOOKBACK: usize = 40;
pub const DREAM_POLL_SECS: u64 = 300;
pub const DREAM_INTERVAL_HOURS_MIN: u32 = 6;
pub const DREAM_INTERVAL_HOURS_MAX: u32 = 48;
pub const DEFAULT_DREAM_INTERVAL_HOURS: u32 = 12;
const DREAM_LAST_AT_KEY: &str = "dream_last_at";
const DREAM_SOURCE: &str = "dream_consolidate";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamReport {
    pub skipped: Option<String>,
    pub wrote: usize,
    pub deleted: usize,
    pub lookback: usize,
    pub lesson_rows: usize,
}

#[derive(Debug, Clone)]
pub struct DreamGate {
    pub enabled: bool,
    pub onboarding_complete: bool,
    pub crypto_mode: bool,
    pub market_open: bool,
    pub interval_hours: u32,
    pub last_at: Option<DateTime<Utc>>,
    pub now: DateTime<Utc>,
    pub cycle_busy: bool,
}

impl DreamReport {
    fn skipped(reason: &str, lookback: usize, lesson_rows: usize) -> Self {
        Self {
            skipped: Some(reason.into()),
            wrote: 0,
            deleted: 0,
            lookback,
            lesson_rows,
        }
    }
}

/// Pure gates. Callers still acquire `CycleGateGuard` before `consolidate`.
pub fn dream_should_run(gate: &DreamGate) -> Result<(), &'static str> {
    if !gate.enabled {
        return Err("disabled");
    }
    if !gate.onboarding_complete {
        return Err("onboarding");
    }
    if gate.cycle_busy {
        return Err("cycle_busy");
    }
    if !gate.crypto_mode && gate.market_open {
        return Err("market_open");
    }
    let interval = i64::from(
        gate.interval_hours
            .clamp(DREAM_INTERVAL_HOURS_MIN, DREAM_INTERVAL_HOURS_MAX),
    );
    if let Some(last) = gate.last_at {
        if gate.now.signed_duration_since(last) < chrono::Duration::hours(interval) {
            return Err("interval");
        }
    }
    Ok(())
}

pub fn clamp_dream_interval_hours(hours: u32) -> u32 {
    hours.clamp(DREAM_INTERVAL_HOURS_MIN, DREAM_INTERVAL_HOURS_MAX)
}

pub fn last_dream_at(conn: &Connection) -> Option<DateTime<Utc>> {
    let raw: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [DREAM_LAST_AT_KEY],
            |row| row.get(0),
        )
        .ok()?;
    DateTime::parse_from_rfc3339(&raw)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

pub fn set_last_dream_at(conn: &Connection, at: DateTime<Utc>) -> Result<()> {
    set_setting(conn, DREAM_LAST_AT_KEY, &at.to_rfc3339())
}

/// Only loss-side patterns become standing rules. Winners/TPs are not a buy-more loop.
fn is_dreamable_pattern(pattern: &str) -> bool {
    matches!(
        pattern,
        "chase_reversal" | "stop_loss" | "time_stop" | "loser"
    )
}

/// Pattern-only copy. No “this name” / “similar name” that the model can join to tradeLessons.
fn dream_caution(pattern: &str) -> &'static str {
    match pattern {
        "chase_reversal" => {
            "Do not buy a name already green on the day / mid-RSI just above SMA, then dump on the first dip. One-day % is not an edge."
        }
        "stop_loss" => {
            "Hard stops have repeated. Do not re-enter the same setup until the thesis is new."
        }
        "time_stop" => {
            "Time-stops have recycled stale or losing lots. Do not immediately redeploy into a similar setup."
        }
        "loser" => {
            "Repeated closed losses of the same pattern. One result is not a standing rule and is not a symbol ban."
        }
        _ => "Standing caution for NEW buys. This is not a ticker ban.",
    }
}

fn dream_rule_text(pattern: &str, n: usize, lookback: usize) -> String {
    format!(
        "DREAM rule pattern={pattern} n={n} lookback={lookback}. Standing caution for NEW buys: {caution} This is not a ticker blacklist. Do not raise minConfidence. Do not treat this as a sell-now order.",
        caution = dream_caution(pattern),
    )
}

fn pattern_from_dream_text(text: &str) -> Option<String> {
    let rest = text.strip_prefix("DREAM rule pattern=")?;
    rest.split_whitespace()
        .next()
        .map(|s| s.trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty())
}

struct CloseRow {
    symbol: String,
    horizon: f64,
    created_at: String,
    signal_id: String,
}

fn load_closes(conn: &Connection, limit: usize) -> Result<Vec<CloseRow>> {
    let mut stmt = conn.prepare(
        "SELECT symbol, horizon_return_pct, created_at, signal_id
         FROM signal_outcomes WHERE event = 'close'
         ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(CloseRow {
            symbol: row.get(0)?,
            horizon: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
            created_at: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            signal_id: row.get(3)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn existing_dreams(conn: &Connection) -> Result<BTreeMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT id, text FROM agent_memories WHERE source = ?1",
    )?;
    let rows = stmt.query_map([DREAM_SOURCE], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = BTreeMap::new();
    for row in rows.flatten() {
        let (id, text) = row;
        if let Some(pattern) = pattern_from_dream_text(&text) {
            out.insert(pattern, id);
        }
    }
    Ok(out)
}

fn lesson_count(conn: &Connection) -> usize {
    conn.query_row(
        "SELECT COUNT(*) FROM agent_memories WHERE source = 'trade_outcome'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0) as usize
}

/// Deterministic consolidate. Does not call the LLM, execution, or strategy writes.
pub fn consolidate(conn: &Connection) -> Result<DreamReport> {
    let lesson_rows = lesson_count(conn);
    let closes = load_closes(conn, LOOKBACK)?;
    if closes.len() < MIN_CLOSES {
        return Ok(DreamReport::skipped(
            "few_closes",
            closes.len(),
            lesson_rows,
        ));
    }

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for row in &closes {
        let hold = hold_hours_as_of(conn, &row.symbol, &row.created_at);
        let exit_model = signal_model(conn, &row.signal_id).unwrap_or_default();
        let pattern = classify_close(row.horizon, hold, &exit_model);
        *counts.entry(pattern.to_string()).or_insert(0) += 1;
    }

    let lookback = closes.len();
    let qualified: BTreeSet<String> = counts
        .iter()
        .filter(|(p, n)| **n >= MIN_PATTERN_N && is_dreamable_pattern(p))
        .map(|(p, _)| p.clone())
        .collect();

    let existing = existing_dreams(conn)?;
    let mut deleted = 0usize;
    let mut wrote = 0usize;

    for (pattern, id) in &existing {
        if !qualified.contains(pattern) {
            conn.execute("DELETE FROM agent_memories WHERE id = ?1", [id])?;
            deleted += 1;
        }
    }

    for pattern in &qualified {
        let n = counts.get(pattern).copied().unwrap_or(0);
        let text = dream_rule_text(pattern, n, lookback);
        if text.to_ascii_lowercase().contains("blacklist")
            && !text.contains("not a ticker blacklist")
        {
            anyhow::bail!("dream rule must not ban tickers");
        }
        if let Some(id) = existing.get(pattern) {
            conn.execute("DELETE FROM agent_memories WHERE id = ?1", [id])?;
        }
        memory::insert_memory(conn, "freeform", None, &text, DREAM_SOURCE, None)?;
        wrote += 1;
    }

    debug_assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM agent_memories WHERE source = 'trade_outcome'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize,
        lesson_rows,
        "consolidate must not delete raw LESSON rows"
    );

    Ok(DreamReport {
        skipped: None,
        wrote,
        deleted,
        lookback,
        lesson_rows,
    })
}

pub fn desk_dream_rules(conn: &Connection) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT text, created_at FROM agent_memories
         WHERE source = ?1 ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([DREAM_SOURCE], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (text, created) = row;
        let pattern = pattern_from_dream_text(&text).unwrap_or_default();
        let n = text
            .split_whitespace()
            .find_map(|p| p.strip_prefix("n=")?.trim_end_matches('.').parse::<i64>().ok())
            .unwrap_or(0);
        out.push(serde_json::json!({
            "pattern": pattern,
            "n": n,
            "caution": dream_caution(&pattern),
            "text": text,
            "reviewedAt": created,
        }));
    }
    Ok(out)
}

pub fn start_dream_scheduler(app: AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(DREAM_POLL_SECS));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let app2 = app.clone();
            let state2 = state.clone();
            match tokio::task::spawn_blocking(move || tick_dream(&app2, &state2)).await {
                Ok(Ok(report)) => {
                    if let Some(reason) = report.skipped.as_deref() {
                        tracing::debug!(target: "dream", reason, "dream tick skipped");
                    } else {
                        tracing::info!(
                            target: "dream",
                            wrote = report.wrote,
                            deleted = report.deleted,
                            lookback = report.lookback,
                            "dream consolidate finished"
                        );
                    }
                }
                Ok(Err(e)) => tracing::warn!(target: "dream", error = %e, "dream tick failed"),
                Err(e) => tracing::warn!(target: "dream", error = %e, "dream tick join failed"),
            }
        }
    });
}

fn tick_dream(_app: &AppHandle, state: &Arc<AppState>) -> Result<DreamReport> {
    let settings = state.db.with_conn(get_settings)?;
    let last_at = state.db.with_conn(|conn| Ok(last_dream_at(conn)))?;
    let crypto_mode = crate::broker::crypto_mode();
    let market_open = TradingCalendar::default().is_market_open();
    let cycle_busy = state.is_cycle_running();
    let gate = DreamGate {
        enabled: settings.dream_enabled,
        onboarding_complete: settings.onboarding_complete,
        crypto_mode,
        market_open,
        interval_hours: settings.dream_interval_hours,
        last_at,
        now: Utc::now(),
        cycle_busy,
    };
    if let Err(reason) = dream_should_run(&gate) {
        return Ok(DreamReport::skipped(reason, 0, 0));
    }

    let Some(_lock) = CycleGateGuard::acquire(state.clone()) else {
        return Ok(DreamReport::skipped("cycle_busy", 0, 0));
    };

    let report = state.db.with_conn(consolidate)?;
    if report.skipped.is_none() {
        state.db.with_conn(|conn| set_last_dream_at(conn, Utc::now()))?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE signals (
                id TEXT PRIMARY KEY, symbol TEXT, action TEXT, confidence REAL,
                rationale TEXT, technical_snapshot TEXT, model_name TEXT,
                prompt_version TEXT, risk_policy_result TEXT, executed INTEGER
             );
             CREATE TABLE signal_outcomes (
                id TEXT PRIMARY KEY, signal_id TEXT, symbol TEXT, side TEXT, event TEXT,
                quantity REAL, fill_price REAL, fee REAL, pnl REAL,
                horizon_return_pct REAL, confidence REAL, venue TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                UNIQUE(signal_id, event)
             );
             CREATE TABLE sandbox_trades (
                id TEXT PRIMARY KEY, symbol TEXT, side TEXT, executed_at TEXT
             );
             CREATE TABLE broker_orders (
                id TEXT PRIMARY KEY, symbol TEXT, side TEXT, status TEXT, created_at TEXT
             );
             CREATE TABLE strategy_param_sets (
                id TEXT PRIMARY KEY, min_confidence_to_trade REAL NOT NULL
             );
             CREATE TABLE blotter (
                id TEXT PRIMARY KEY, symbol TEXT, side TEXT, created_at TEXT
             );",
        )
        .unwrap();
        conn.execute_batch(include_str!("../migrations/008_agent_memories.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/015_dream_consolidate.sql"))
            .unwrap();
        conn.execute(
            "INSERT INTO strategy_param_sets (id, min_confidence_to_trade) VALUES ('s1', 0.65)",
            [],
        )
        .unwrap();
        conn
    }

    fn insert_close(
        conn: &Connection,
        id: &str,
        symbol: &str,
        horizon: f64,
        closed_at: &str,
        bought_at: &str,
        model: &str,
    ) {
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES (?1, ?2, 'SELL', 0.6, 'close', '{}', ?3, 'v2.5.2', 'APPROVED', 1)",
            rusqlite::params![id, symbol, model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signal_outcomes (
                id, signal_id, symbol, side, event, quantity, fill_price, fee,
                pnl, horizon_return_pct, confidence, venue, created_at
             ) VALUES (?1, ?1, ?2, 'SELL', 'close', 1, 8.0, 0, ?3, ?4, 0.6, 'busha', ?5)",
            rusqlite::params![id, symbol, horizon * 100.0, horizon, closed_at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sandbox_trades (id, symbol, side, executed_at) VALUES (?1, ?2, 'BUY', ?3)",
            rusqlite::params![format!("b-{id}"), symbol, bought_at],
        )
        .unwrap();
    }

    fn seed_chase_book(conn: &Connection, chase_n: usize, pad_to: usize) {
        for i in 0..chase_n {
            insert_close(
                conn,
                &format!("c{i}"),
                "HOME",
                -0.12,
                &format!("2026-08-01T10:{i:02}:00Z"),
                &format!("2026-08-01T09:{i:02}:00Z"),
                "openai:gpt-5.6-luna",
            );
        }
        // Pad with mixed patterns so only chase_reversal can reach n>=3.
        let fillers = [
            (-0.06, "rules:stop-loss"),
            (-0.03, "rules:time-stop"),
            (0.10, "rules:take-profit"),
            (0.04, "llm"),
            (-0.02, "llm"),
            (0.0, "llm"),
        ];
        let need = pad_to.saturating_sub(chase_n);
        for i in 0..need {
            let (horizon, model) = fillers[i % fillers.len()];
            insert_close(
                conn,
                &format!("p{i}"),
                &format!("PAD{i}"),
                horizon,
                &format!("2026-08-02T10:{i:02}:00Z"),
                &format!("2026-08-01T06:{i:02}:00Z"),
                model,
            );
        }
    }

    fn dream_texts(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT text FROM agent_memories WHERE source = 'dream_consolidate'")
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    #[test]
    fn skips_when_closes_below_twelve() {
        let conn = setup();
        seed_chase_book(&conn, 3, 11);
        let report = consolidate(&conn).unwrap();
        assert_eq!(report.skipped.as_deref(), Some("few_closes"));
        assert_eq!(report.wrote, 0);
        assert!(dream_texts(&conn).is_empty());
    }

    #[test]
    fn skips_write_when_no_pattern_reaches_three() {
        let conn = setup();
        // 12 closes, two of each of six patterns — none reach n=3.
        let rows = [
            ("c0", "HOME", -0.12, "llm", "2026-08-01T10:00:00Z", "2026-08-01T09:40:00Z"),
            ("c1", "ONE", -0.12, "llm", "2026-08-01T11:00:00Z", "2026-08-01T10:40:00Z"),
            ("s0", "TWO", -0.06, "rules:stop-loss", "2026-08-01T12:00:00Z", "2026-08-01T06:00:00Z"),
            ("s1", "TRE", -0.06, "rules:stop-loss", "2026-08-01T13:00:00Z", "2026-08-01T07:00:00Z"),
            ("t0", "FOR", -0.03, "rules:time-stop", "2026-08-01T14:00:00Z", "2026-07-30T14:00:00Z"),
            ("t1", "FIV", -0.03, "rules:time-stop", "2026-08-01T15:00:00Z", "2026-07-30T15:00:00Z"),
            ("p0", "SIX", 0.10, "rules:take-profit", "2026-08-01T16:00:00Z", "2026-07-31T16:00:00Z"),
            ("p1", "SEV", 0.10, "rules:take-profit", "2026-08-01T17:00:00Z", "2026-07-31T17:00:00Z"),
            ("w0", "EGT", 0.04, "llm", "2026-08-01T18:00:00Z", "2026-07-31T18:00:00Z"),
            ("w1", "NIN", 0.04, "llm", "2026-08-01T19:00:00Z", "2026-07-31T19:00:00Z"),
            ("l0", "TEN", -0.02, "llm", "2026-08-01T20:00:00Z", "2026-07-31T20:00:00Z"),
            ("l1", "ELV", -0.02, "llm", "2026-08-01T21:00:00Z", "2026-07-31T21:00:00Z"),
        ];
        for (id, sym, horizon, model, closed, bought) in rows {
            insert_close(&conn, id, sym, horizon, closed, bought, model);
        }
        let report = consolidate(&conn).unwrap();
        assert!(report.skipped.is_none());
        assert_eq!(report.wrote, 0);
        assert!(dream_texts(&conn).is_empty());
    }

    #[test]
    fn writes_chase_reversal_when_three_of_twelve() {
        let conn = setup();
        seed_chase_book(&conn, 3, 12);
        memory::insert_memory(
            &conn,
            "symbol_lesson",
            Some("HOME"),
            "LESSON close HOME venue=busha result=loss pattern=chase_reversal pnl=-12.6%",
            "trade_outcome",
            None,
        )
        .unwrap();
        let before_lessons: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_memories WHERE source = 'trade_outcome'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let report = consolidate(&conn).unwrap();
        assert!(report.skipped.is_none());
        assert_eq!(report.wrote, 1);
        let texts = dream_texts(&conn);
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("pattern=chase_reversal"));
        assert!(texts[0].contains("n=3"));
        assert!(texts[0].contains("not a ticker blacklist"));
        assert!(texts[0].contains("Do not raise minConfidence"));
        assert!(!texts[0].to_ascii_lowercase().contains("this name"));
        assert!(!texts[0].to_ascii_lowercase().contains("similar name"));
        assert!(!texts[0].contains("HOME"));
        assert!(!texts[0].contains("ATOM"));
        let after_lessons: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_memories WHERE source = 'trade_outcome'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before_lessons, after_lessons);
    }

    #[test]
    fn three_take_profits_do_not_write_a_buy_more_rule() {
        let conn = setup();
        for i in 0..3 {
            insert_close(
                &conn,
                &format!("tp{i}"),
                "WIN",
                0.10,
                &format!("2026-08-01T12:{i:02}:00Z"),
                &format!("2026-07-31T12:{i:02}:00Z"),
                "rules:take-profit",
            );
        }
        for i in 0..9 {
            insert_close(
                &conn,
                &format!("w{i}"),
                &format!("W{i}"),
                0.04,
                &format!("2026-08-02T12:{i:02}:00Z"),
                &format!("2026-08-01T12:{i:02}:00Z"),
                "llm",
            );
        }
        let report = consolidate(&conn).unwrap();
        assert!(report.skipped.is_none());
        assert_eq!(report.wrote, 0);
        assert!(dream_texts(&conn).is_empty());
    }

    #[test]
    fn consolidate_does_not_evict_lesson_rows_at_cap() {
        let conn = setup();
        seed_chase_book(&conn, 3, 12);
        for i in 0..memory::MEMORY_CAP {
            conn.execute(
                "INSERT INTO agent_memories (id, kind, symbol, text, source, created_at, updated_at)
                 VALUES (?1, 'symbol_lesson', 'X', ?2, 'trade_outcome', datetime('now'), datetime('now'))",
                rusqlite::params![format!("m{i}"), format!("LESSON close pad {i}")],
            )
            .unwrap();
        }
        let before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_memories WHERE source = 'trade_outcome'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let report = consolidate(&conn).unwrap();
        assert_eq!(report.wrote, 1);
        let after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_memories WHERE source = 'trade_outcome'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(after, memory::MEMORY_CAP as i64);
    }

    #[test]
    fn replace_keeps_one_rule_per_pattern() {
        let conn = setup();
        seed_chase_book(&conn, 3, 12);
        consolidate(&conn).unwrap();
        consolidate(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_memories WHERE source = 'dream_consolidate'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn does_not_mutate_strategy_or_execution() {
        let conn = setup();
        seed_chase_book(&conn, 3, 12);
        consolidate(&conn).unwrap();
        let conf: f64 = conn
            .query_row(
                "SELECT min_confidence_to_trade FROM strategy_param_sets WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((conf - 0.65).abs() < 1e-12);
        let blotter: i64 = conn
            .query_row("SELECT COUNT(*) FROM blotter", [], |r| r.get(0))
            .unwrap();
        let orders: i64 = conn
            .query_row("SELECT COUNT(*) FROM broker_orders", [], |r| r.get(0))
            .unwrap();
        assert_eq!(blotter, 0);
        assert_eq!(orders, 0);
    }

    #[test]
    fn gates_are_fail_closed() {
        let now = DateTime::parse_from_rfc3339("2026-08-23T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let last = Some(
            DateTime::parse_from_rfc3339("2026-08-23T06:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let mut gate = DreamGate {
            enabled: false,
            onboarding_complete: true,
            crypto_mode: true,
            market_open: false,
            interval_hours: 12,
            last_at: None,
            now,
            cycle_busy: false,
        };
        assert_eq!(dream_should_run(&gate), Err("disabled"));
        gate.enabled = true;
        gate.onboarding_complete = false;
        assert_eq!(dream_should_run(&gate), Err("onboarding"));
        gate.onboarding_complete = true;
        gate.cycle_busy = true;
        assert_eq!(dream_should_run(&gate), Err("cycle_busy"));
        gate.cycle_busy = false;
        gate.crypto_mode = false;
        gate.market_open = true;
        assert_eq!(dream_should_run(&gate), Err("market_open"));
        gate.market_open = false;
        gate.last_at = last;
        gate.interval_hours = 12;
        assert_eq!(dream_should_run(&gate), Err("interval"));
        gate.last_at = Some(
            DateTime::parse_from_rfc3339("2026-08-22T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert!(dream_should_run(&gate).is_ok());
        gate.crypto_mode = true;
        gate.market_open = true;
        assert!(dream_should_run(&gate).is_ok());
    }

    #[test]
    fn historical_hold_classifies_old_chase() {
        let conn = setup();
        insert_close(
            &conn,
            "old",
            "HOME",
            -0.126,
            "2026-08-01T10:26:00Z",
            "2026-08-01T10:00:00Z",
            "openai:gpt-5.6-luna",
        );
        let hold = hold_hours_as_of(&conn, "HOME", "2026-08-01T10:26:00Z").unwrap();
        assert!((hold - 0.4333).abs() < 0.05);
        assert_eq!(
            classify_close(-0.126, Some(hold), "openai:gpt-5.6-luna"),
            "chase_reversal"
        );
    }
}
