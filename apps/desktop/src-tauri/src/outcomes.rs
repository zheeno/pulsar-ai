//! Fill/close outcome labels. Unknown/partial broker states must not write rows.

use anyhow::Result;
use rusqlite::Connection;
use uuid::Uuid;

use crate::memory;

/// Persist a fill label. Idempotent per (signal_id, 'fill').
/// `avg_cost` on SELL also writes a `close` row with realized PnL (idempotent).
pub fn record_executed_fill(
    conn: &Connection,
    signal_id: &str,
    symbol: &str,
    side: &str,
    quantity: f64,
    fill_price: f64,
    fee: f64,
    venue: &str,
    avg_cost: Option<f64>,
) -> Result<()> {
    if signal_id.is_empty() || quantity <= 0.0 || fill_price <= 0.0 {
        return Ok(());
    }
    let side = if side.eq_ignore_ascii_case("SELL") {
        "SELL"
    } else {
        "BUY"
    };
    let confidence: Option<f64> = conn
        .query_row(
            "SELECT confidence FROM signals WHERE id = ?1",
            [signal_id],
            |row| row.get(0),
        )
        .ok();

    insert_outcome(
        conn,
        signal_id,
        symbol,
        side,
        "fill",
        quantity,
        fill_price,
        fee,
        None,
        None,
        confidence,
        venue,
    )?;

    if side == "SELL" {
        if let Some(cost) = avg_cost.filter(|c| *c > 0.0 && c.is_finite()) {
            let pnl = (fill_price - cost) * quantity - fee;
            let horizon = (fill_price - cost) / cost;
            insert_outcome(
                conn,
                signal_id,
                symbol,
                side,
                "close",
                quantity,
                fill_price,
                fee,
                Some(pnl),
                Some(horizon),
                confidence,
                venue,
            )?;
            let hold_hours = hours_since_last_buy(conn, symbol);
            let exit_model = signal_model(conn, signal_id).unwrap_or_default();
            let entry = last_executed_buy(conn, symbol);
            let pattern = classify_close(horizon, hold_hours, &exit_model);
            let text = close_lesson_text(
                symbol,
                venue,
                quantity,
                fill_price,
                cost,
                horizon,
                pnl,
                hold_hours,
                pattern,
                &exit_model,
                entry.as_deref(),
            );
            let _ = memory::insert_memory(
                conn,
                "symbol_lesson",
                Some(symbol),
                &text,
                "trade_outcome",
                None,
            );
        }
    }
    Ok(())
}

/// Pattern on a closed lot. One loss is not a blacklist — the label is for retrieval.
pub fn classify_close(horizon: f64, hold_hours: Option<f64>, exit_model: &str) -> &'static str {
    let model = exit_model.to_ascii_lowercase();
    if model.contains("stop-loss") {
        return "stop_loss";
    }
    if model.contains("time-stop") {
        return "time_stop";
    }
    if model.contains("take-profit") {
        return "take_profit";
    }
    let quick = hold_hours.map(|h| h < 2.0).unwrap_or(false);
    if horizon > 0.0 {
        return "winner";
    }
    if quick && horizon <= -0.04 {
        return "chase_reversal";
    }
    if horizon < 0.0 {
        "loser"
    } else {
        "flat"
    }
}

fn close_caution(pattern: &str) -> &'static str {
    match pattern {
        "chase_reversal" => {
            "Do not buy a name already green on the day / mid-RSI just above SMA, then dump on the first dip. One-day % is not an edge."
        }
        "stop_loss" => {
            "Hard stop fired. Do not re-enter the same setup on this name until the thesis is new."
        }
        "time_stop" => "Time-stop recycled a stale/losing lot. Do not immediately redeploy into a similar name.",
        "take_profit" => "Winner was realized at the stored target. Repeat the setup, not the ticker.",
        "winner" => "Closed green. Prefer letting winners work over scalp-flips.",
        _ => "Recorded close. One result is not a standing rule and is not a symbol ban.",
    }
}

pub fn close_lesson_text(
    symbol: &str,
    venue: &str,
    quantity: f64,
    fill_price: f64,
    cost: f64,
    horizon: f64,
    pnl: f64,
    hold_hours: Option<f64>,
    pattern: &str,
    exit_model: &str,
    entry_rationale: Option<&str>,
) -> String {
    let result = if horizon > 0.0 { "win" } else { "loss" };
    let hold = hold_hours
        .map(|h| format!("{h:.1}h"))
        .unwrap_or_else(|| "unknown".into());
    let entry = entry_rationale
        .map(|s| truncate_lesson(s, 180))
        .unwrap_or_default();
    format!(
        "LESSON close {sym} venue={venue} result={result} pattern={pattern} pnl={:+.1}% ₦{pnl:.2} hold={hold} exit={exit} qty={quantity:.4} px={fill_price:.4} cost={cost:.4}. {caution} entry: {entry}",
        horizon * 100.0,
        sym = symbol.to_uppercase(),
        exit = if exit_model.is_empty() { "unknown" } else { exit_model },
        caution = close_caution(pattern),
        entry = if entry.is_empty() { "n/a" } else { entry.as_str() },
    )
}

/// Last closed lots for the cycle prompt. Numbers, not a confidence-floor knob.
pub fn desk_lessons(
    conn: &Connection,
    venue: Option<&str>,
    limit: usize,
) -> Result<Vec<serde_json::Value>> {
    let limit = limit.clamp(1, 30) as i64;
    let sql = if venue.is_some() {
        "SELECT symbol, horizon_return_pct, pnl, confidence, venue, created_at, signal_id
         FROM signal_outcomes WHERE event = 'close' AND venue = ?1
         ORDER BY created_at DESC LIMIT ?2"
    } else {
        "SELECT symbol, horizon_return_pct, pnl, confidence, venue, created_at, signal_id
         FROM signal_outcomes WHERE event = 'close'
         ORDER BY created_at DESC LIMIT ?1"
    };
    let mut stmt = conn.prepare(sql)?;
    let mut rows = if let Some(v) = venue {
        stmt.query(rusqlite::params![v, limit])?
    } else {
        stmt.query(rusqlite::params![limit])?
    };
    let mut raw = Vec::new();
    while let Some(row) = rows.next()? {
        let symbol: String = row.get(0)?;
        let horizon: Option<f64> = row.get(1)?;
        let pnl: Option<f64> = row.get(2)?;
        let confidence: Option<f64> = row.get(3)?;
        let v: Option<String> = row.get(4)?;
        let created: Option<String> = row.get(5)?;
        let sid: String = row.get(6)?;
        let exit_model = signal_model(conn, &sid).unwrap_or_default();
        let hold = hours_since_last_buy(conn, &symbol);
        let h = horizon.unwrap_or(0.0);
        let pattern = classify_close(h, hold, &exit_model);
        raw.push(serde_json::json!({
            "symbol": symbol,
            "pattern": pattern,
            "result": if h > 0.0 { "win" } else { "loss" },
            "horizonPct": h * 100.0,
            "pnl": pnl,
            "holdHours": hold,
            "confidence": confidence,
            "venue": v,
            "exitModel": exit_model,
            "caution": close_caution(pattern),
            "closedAt": created,
        }));
    }
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for item in &raw {
        if let Some(p) = item.get("pattern").and_then(|v| v.as_str()) {
            *counts.entry(p.to_string()).or_insert(0) += 1;
        }
    }
    for item in &mut raw {
        if let Some(obj) = item.as_object_mut() {
            let p = obj.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            obj.insert("repeatCount".into(), serde_json::json!(counts.get(p).copied().unwrap_or(0)));
        }
    }
    Ok(raw)
}

pub fn memory_search_query(held: &str, recently_sold: &[String]) -> String {
    let sold = recently_sold.join(" ");
    format!(
        "LESSON close result=loss pattern=chase_reversal stop_loss winner short hold green tape RSI SMA holdings {held} sold {sold}"
    )
}

fn hours_since_last_buy(conn: &Connection, symbol: &str) -> Option<f64> {
    let mut stamps = Vec::new();
    if let Ok(ts) = conn.query_row(
        "SELECT executed_at FROM sandbox_trades
         WHERE UPPER(symbol) = UPPER(?1) AND UPPER(side) = 'BUY'
         ORDER BY executed_at DESC LIMIT 1",
        [symbol],
        |row| row.get::<_, String>(0),
    ) {
        stamps.push(ts);
    }
    if let Ok(ts) = conn.query_row(
        "SELECT created_at FROM broker_orders
         WHERE UPPER(symbol) = UPPER(?1) AND UPPER(side) = 'BUY'
           AND LOWER(status) IN ('executed', 'filled')
         ORDER BY created_at DESC LIMIT 1",
        [symbol],
        |row| row.get::<_, String>(0),
    ) {
        stamps.push(ts);
    }
    if let Ok(ts) = conn.query_row(
        "SELECT created_at FROM signal_outcomes
         WHERE UPPER(symbol) = UPPER(?1) AND UPPER(side) = 'BUY' AND event = 'fill'
         ORDER BY created_at DESC LIMIT 1",
        [symbol],
        |row| row.get::<_, String>(0),
    ) {
        stamps.push(ts);
    }
    let latest = stamps.into_iter().max()?;
    let parsed = chrono::NaiveDateTime::parse_from_str(&latest, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&latest, "%Y-%m-%d %H:%M:%S%.f"))
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&latest).map(|d| d.naive_utc()))
        .ok()?;
    let secs = (chrono::Utc::now().naive_utc() - parsed).num_seconds() as f64;
    if secs.is_finite() && secs >= 0.0 {
        Some(secs / 3600.0)
    } else {
        None
    }
}

fn signal_model(conn: &Connection, signal_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT COALESCE(model_name, '') FROM signals WHERE id = ?1",
        [signal_id],
        |row| row.get(0),
    )
    .ok()
}

fn last_executed_buy(conn: &Connection, symbol: &str) -> Option<String> {
    conn.query_row(
        "SELECT rationale FROM signals
         WHERE UPPER(symbol) = UPPER(?1) AND action = 'BUY' AND executed = 1
         ORDER BY generated_at DESC LIMIT 1",
        [symbol],
        |row| row.get(0),
    )
    .ok()
}

fn truncate_lesson(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| *c == ' ' || !c.is_control())
        .collect();
    let mut chars = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if chars.chars().count() > max {
        chars = chars.chars().take(max.saturating_sub(1)).collect();
        chars.push('…');
    }
    chars
}

fn insert_outcome(
    conn: &Connection,
    signal_id: &str,
    symbol: &str,
    side: &str,
    event: &str,
    quantity: f64,
    fill_price: f64,
    fee: f64,
    pnl: Option<f64>,
    horizon: Option<f64>,
    confidence: Option<f64>,
    venue: &str,
) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO signal_outcomes (
            id, signal_id, symbol, side, event, quantity, fill_price, fee,
            pnl, horizon_return_pct, confidence, venue
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            id,
            signal_id,
            symbol.to_uppercase(),
            side,
            event,
            quantity,
            fill_price,
            fee,
            pnl,
            horizon,
            confidence,
            venue,
        ],
    )?;
    Ok(())
}

/// Confidence buckets → hit rate / average close PnL (closed lots only).
pub fn confidence_journal(conn: &Connection) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT
            CASE
              WHEN COALESCE(confidence, 0) < 0.55 THEN '0.40–0.55'
              WHEN COALESCE(confidence, 0) < 0.70 THEN '0.55–0.70'
              WHEN COALESCE(confidence, 0) < 0.85 THEN '0.70–0.85'
              ELSE '0.85–1.00'
            END AS bucket,
            COUNT(*) AS n,
            AVG(CASE WHEN COALESCE(pnl, 0) > 0 THEN 1.0 ELSE 0.0 END) AS hit_rate,
            AVG(pnl) AS avg_pnl,
            AVG(horizon_return_pct) AS avg_return
         FROM signal_outcomes
         WHERE event = 'close'
         GROUP BY 1
         ORDER BY 1",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "bucket": row.get::<_, String>(0)?,
            "n": row.get::<_, i64>(1)?,
            "hitRate": row.get::<_, f64>(2)?,
            "avgPnl": row.get::<_, Option<f64>>(3)?,
            "avgReturnPct": row.get::<_, Option<f64>>(4)?,
        }))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE signals (
                id TEXT PRIMARY KEY, symbol TEXT, action TEXT, confidence REAL,
                rationale TEXT, technical_snapshot TEXT, model_name TEXT,
                prompt_version TEXT, risk_policy_result TEXT, executed INTEGER
             );
             CREATE TABLE agent_memories (
                id TEXT PRIMARY KEY, kind TEXT, symbol TEXT, text TEXT,
                embedding BLOB, source TEXT, created_at TEXT, updated_at TEXT
             );
             CREATE TABLE signal_outcomes (
                id TEXT PRIMARY KEY, signal_id TEXT, symbol TEXT, side TEXT, event TEXT,
                quantity REAL, fill_price REAL, fee REAL, pnl REAL,
                horizon_return_pct REAL, confidence REAL, venue TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                UNIQUE(signal_id, event)
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES ('s1', 'GTCO', 'SELL', 0.8, 'tp', '{}', 'rules:take-profit', 'v2.4.0', 'APPROVED', 1)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn fill_and_close_written_once() {
        let conn = setup();
        record_executed_fill(&conn, "s1", "GTCO", "SELL", 10.0, 120.0, 5.0, "sandbox", Some(100.0))
            .unwrap();
        record_executed_fill(&conn, "s1", "GTCO", "SELL", 10.0, 120.0, 5.0, "sandbox", Some(100.0))
            .unwrap();
        let fills: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM signal_outcomes WHERE event = 'fill'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let closes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM signal_outcomes WHERE event = 'close'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fills, 1);
        assert_eq!(closes, 1);
        let pnl: f64 = conn
            .query_row(
                "SELECT pnl FROM signal_outcomes WHERE event = 'close'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((pnl - 195.0).abs() < 1e-9);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_memories WHERE source = 'trade_outcome'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(n >= 1);
    }

    #[test]
    fn unknown_path_not_implied_buy_without_price() {
        let conn = setup();
        record_executed_fill(&conn, "s1", "GTCO", "BUY", 0.0, 0.0, 0.0, "bamboo", None).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM signal_outcomes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn journal_buckets_closes() {
        let conn = setup();
        record_executed_fill(&conn, "s1", "GTCO", "SELL", 10.0, 120.0, 0.0, "sandbox", Some(100.0))
            .unwrap();
        let j = confidence_journal(&conn).unwrap();
        assert_eq!(j.len(), 1);
        assert_eq!(j[0]["bucket"], "0.70–0.85");
        assert_eq!(j[0]["n"], 1);
    }

    #[test]
    fn buy_fill_does_not_write_a_lesson() {
        let conn = setup();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES ('b1', 'HOME', 'BUY', 0.58, 'above sma', '{}', 'llm', 'v2.5.0', 'APPROVED', 1)",
            [],
        )
        .unwrap();
        record_executed_fill(&conn, "b1", "HOME", "BUY", 1.0, 9.52, 0.0, "busha", None).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_memories WHERE source = 'trade_outcome'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn home_style_quick_loss_is_chase_reversal() {
        assert_eq!(
            classify_close(-0.126, Some(0.43), "openai:gpt-5.6-luna"),
            "chase_reversal"
        );
        let text = close_lesson_text(
            "HOME",
            "busha",
            1.0,
            8.32,
            9.52,
            -0.126,
            -1.2,
            Some(0.43),
            "chase_reversal",
            "openai:gpt-5.6-luna",
            Some("Price above SMA50, RSI 62.9, daily +3.61%"),
        );
        assert!(text.contains("pattern=chase_reversal"));
        assert!(text.contains("result=loss"));
        assert!(text.contains("One-day % is not an edge"));
        assert!(text.contains("RSI 62.9"));
    }

    #[test]
    fn one_loss_is_not_classified_as_a_ban() {
        assert_eq!(classify_close(-0.02, Some(20.0), "llm"), "loser");
        let text = close_lesson_text(
            "ATOM",
            "busha",
            1.0,
            10.0,
            10.2,
            -0.02,
            -0.2,
            Some(20.0),
            "loser",
            "llm",
            None,
        );
        assert!(text.contains("not a symbol ban"));
    }
}
