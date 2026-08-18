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
            let horizon_pct = horizon * 100.0;
            let text = format!(
                "Closed {quantity:.0} {symbol} @ {fill_price:.2} vs cost {cost:.2} ({horizon_pct:+.1}%) pnl ₦{pnl:.2} venue={venue}"
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
    } else {
        let text = format!("Filled BUY {quantity:.0} {symbol} @ {fill_price:.2} venue={venue}");
        let _ = memory::insert_memory(
            conn,
            "symbol_lesson",
            Some(symbol),
            &text,
            "trade_outcome",
            None,
        );
    }
    Ok(())
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
            AVG(CASE WHEN COALESCE(horizon_return_pct, 0) > 0 THEN 1.0 ELSE 0.0 END) AS hit_rate,
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
}
