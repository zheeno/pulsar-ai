use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct OrderIntent {
    pub id: String,
    pub client_order_id: String,
    pub external_order_id: Option<i64>,
    pub external_order_ref: Option<String>,
    pub signal_id: Option<String>,
    pub symbol: String,
    pub side: String,
    pub requested_qty: f64,
    pub requested_quote: Option<f64>,
    pub requested_notional: Option<f64>,
    pub state: String,
}

pub fn insert_intent(
    conn: &Connection,
    signal_id: Option<&str>,
    cycle_id: Option<&str>,
    symbol: &str,
    side: &str,
    qty: f64,
    quote: f64,
    venue: &str,
) -> Result<OrderIntent> {
    let id = Uuid::new_v4().to_string();
    let client_order_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO order_intents (
            id, client_order_id, cycle_id, signal_id, symbol, side,
            requested_qty, requested_quote, requested_notional, venue, state
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'created')",
        rusqlite::params![
            id,
            client_order_id,
            cycle_id,
            signal_id,
            symbol,
            side,
            qty,
            quote,
            qty * quote,
            venue,
        ],
    )?;
    Ok(OrderIntent {
        id,
        client_order_id,
        external_order_id: None,
        external_order_ref: None,
        signal_id: signal_id.map(str::to_string),
        symbol: symbol.into(),
        side: side.into(),
        requested_qty: qty,
        requested_quote: Some(quote),
        requested_notional: Some(qty * quote),
        state: "created".into(),
    })
}

pub fn mark_submitted(conn: &Connection, id: &str, external_order_ref: Option<&str>) -> Result<()> {
    let numeric = external_order_ref.and_then(|s| s.parse::<i64>().ok());
    conn.execute(
        "UPDATE order_intents SET state = 'submitted',
            external_order_ref = COALESCE(?2, external_order_ref),
            external_order_id = COALESCE(?3, external_order_id),
            updated_at = datetime('now') WHERE id = ?1",
        rusqlite::params![id, external_order_ref, numeric],
    )?;
    Ok(())
}

pub fn mark_terminal(
    conn: &Connection,
    id: &str,
    state: &str,
    external_order_ref: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    let numeric = external_order_ref.and_then(|s| s.parse::<i64>().ok());
    conn.execute(
        "UPDATE order_intents SET state = ?2,
            external_order_ref = COALESCE(?3, external_order_ref),
            external_order_id = COALESCE(?4, external_order_id),
            last_error = ?5, updated_at = datetime('now') WHERE id = ?1",
        rusqlite::params![id, state, external_order_ref, numeric, error],
    )?;
    Ok(())
}

pub fn pending_buy_notional(conn: &Connection) -> Result<f64> {
    pending_buy_notional_on(conn, None)
}

pub fn pending_buy_notional_on(conn: &Connection, venue: Option<&str>) -> Result<f64> {
    let v: f64 = match venue {
        Some(v) => conn.query_row(
            "SELECT COALESCE(SUM(requested_notional), 0) FROM order_intents
             WHERE side = 'BUY' AND venue = ?1 AND state IN ('created', 'submitted', 'unknown')",
            [v],
            |row| row.get(0),
        )?,
        None => conn.query_row(
            "SELECT COALESCE(SUM(requested_notional), 0) FROM order_intents
             WHERE side = 'BUY' AND state IN ('created', 'submitted', 'unknown')",
            [],
            |row| row.get(0),
        )?,
    };
    Ok(v)
}

pub fn pending_sell_qty(conn: &Connection, symbol: &str) -> Result<f64> {
    pending_sell_qty_on(conn, symbol, None)
}

pub fn pending_sell_qty_on(conn: &Connection, symbol: &str, venue: Option<&str>) -> Result<f64> {
    let v: f64 = match venue {
        Some(venue) => conn.query_row(
            "SELECT COALESCE(SUM(requested_qty), 0) FROM order_intents
             WHERE UPPER(symbol) = UPPER(?1) AND side = 'SELL' AND venue = ?2
               AND state IN ('created', 'submitted', 'unknown')",
            rusqlite::params![symbol, venue],
            |row| row.get(0),
        )?,
        None => conn.query_row(
            "SELECT COALESCE(SUM(requested_qty), 0) FROM order_intents
             WHERE UPPER(symbol) = UPPER(?1) AND side = 'SELL'
               AND state IN ('created', 'submitted', 'unknown')",
            [symbol],
            |row| row.get(0),
        )?,
    };
    Ok(v)
}

pub fn pending_action_count(conn: &Connection) -> Result<i64> {
    let v: i64 = conn.query_row(
        "SELECT COUNT(*) FROM order_intents
         WHERE date(created_at) = date('now') AND state IN ('created', 'submitted', 'unknown', 'filled')
           AND side = 'BUY'",
        [],
        |row| row.get(0),
    )?;
    Ok(v)
}

pub fn ambiguous_pending(conn: &Connection) -> Result<bool> {
    ambiguous_pending_on(conn, None)
}

/// Unknown intents on another broker must not freeze the active venue.
pub fn ambiguous_pending_on(conn: &Connection, venue: Option<&str>) -> Result<bool> {
    let n: i64 = match venue {
        Some(v) => conn.query_row(
            "SELECT COUNT(*) FROM order_intents WHERE state = 'unknown' AND venue = ?1",
            [v],
            |row| row.get(0),
        )?,
        None => conn.query_row(
            "SELECT COUNT(*) FROM order_intents WHERE state = 'unknown'",
            [],
            |row| row.get(0),
        )?,
    };
    Ok(n > 0)
}

pub fn load_open_intents(conn: &Connection) -> Result<Vec<OrderIntent>> {
    load_open_intents_on(conn, None)
}

fn map_open_intent(row: &rusqlite::Row<'_>) -> rusqlite::Result<OrderIntent> {
    let numeric: Option<i64> = row.get(2)?;
    let pref: Option<String> = row.get(3)?;
    Ok(OrderIntent {
        id: row.get(0)?,
        client_order_id: row.get(1)?,
        external_order_id: numeric,
        external_order_ref: pref.or_else(|| numeric.map(|n| n.to_string())),
        signal_id: row.get(4)?,
        symbol: row.get(5)?,
        side: row.get(6)?,
        requested_qty: row.get(7)?,
        requested_quote: row.get(8)?,
        requested_notional: row.get(9)?,
        state: row.get(10)?,
    })
}

pub fn load_open_intents_on(conn: &Connection, venue: Option<&str>) -> Result<Vec<OrderIntent>> {
    let sql = "SELECT id, client_order_id, external_order_id, external_order_ref, signal_id, symbol, side, requested_qty, requested_quote, requested_notional, state
         FROM order_intents WHERE state IN ('created', 'submitted', 'unknown')";
    let mut intents = Vec::new();
    if let Some(v) = venue {
        let mut stmt = conn.prepare(&format!("{sql} AND venue = ?1"))?;
        let rows = stmt.query_map([v], map_open_intent)?;
        intents.extend(rows.filter_map(|r| r.ok()));
    } else {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], map_open_intent)?;
        intents.extend(rows.filter_map(|r| r.ok()));
    }
    Ok(intents)
}

/// Place/HTTP failures must not stay `unknown` (that freezes all live trading
/// via `ambiguous_pending`). Only intents that were actually submitted to the
/// broker (have an order id) belong in `unknown`.
pub fn reject_unsubmitted_unknown(conn: &Connection) -> Result<usize> {
    let n = conn.execute(
        "UPDATE order_intents SET state = 'rejected',
            last_error = COALESCE(last_error, 'never submitted to broker'),
            updated_at = datetime('now')
         WHERE state = 'unknown'
           AND external_order_ref IS NULL
           AND external_order_id IS NULL",
        [],
    )?;
    Ok(n)
}

/// Intents left in `created` never reached the broker (fee/quote failure after insert).
/// Drop them so they do not reserve cash or block a retry of the same signal.
pub fn reject_abandoned_created(conn: &Connection, venue: Option<&str>) -> Result<usize> {
    let n = match venue {
        Some(v) => conn.execute(
            "UPDATE order_intents SET state = 'rejected',
                last_error = COALESCE(last_error, 'abandoned before broker submit'),
                updated_at = datetime('now')
             WHERE state = 'created' AND venue = ?1
               AND external_order_ref IS NULL
               AND external_order_id IS NULL",
            [v],
        )?,
        None => conn.execute(
            "UPDATE order_intents SET state = 'rejected',
                last_error = COALESCE(last_error, 'abandoned before broker submit'),
                updated_at = datetime('now')
             WHERE state = 'created'
               AND external_order_ref IS NULL
               AND external_order_id IS NULL",
            [],
        )?,
    };
    Ok(n)
}

pub fn find_duplicate(
    conn: &Connection,
    symbol: &str,
    side: &str,
    qty: f64,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM order_intents
         WHERE UPPER(symbol) = UPPER(?1) AND side = ?2 AND requested_qty = ?3
           AND created_at >= datetime('now', '-10 minutes')
           AND state IN ('submitted', 'filled', 'unknown')
         LIMIT 1",
        rusqlite::params![symbol, side, qty],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE order_intents (
                id TEXT PRIMARY KEY,
                client_order_id TEXT,
                cycle_id TEXT,
                signal_id TEXT,
                symbol TEXT,
                side TEXT,
                requested_qty REAL,
                requested_quote REAL,
                requested_notional REAL,
                venue TEXT,
                state TEXT,
                external_order_ref TEXT,
                external_order_id INTEGER,
                last_error TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn reject_unsubmitted_unknown_clears_ambiguous_gate() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO order_intents (id, client_order_id, symbol, side, requested_qty, venue, state, last_error)
             VALUES ('a', 'c1', 'NIDF', 'SELL', 6, 'wealth', 'unknown', 'HTTP timeout')",
            [],
        )
        .unwrap();
        assert!(ambiguous_pending(&conn).unwrap());
        let n = reject_unsubmitted_unknown(&conn).unwrap();
        assert_eq!(n, 1);
        assert!(!ambiguous_pending(&conn).unwrap());
    }

    #[test]
    fn keeps_unknown_when_external_ref_exists() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO order_intents (id, client_order_id, symbol, side, requested_qty, venue, state, external_order_ref)
             VALUES ('b', 'c2', 'NIDF', 'SELL', 6, 'wealth', 'unknown', '12345')",
            [],
        )
        .unwrap();
        assert_eq!(reject_unsubmitted_unknown(&conn).unwrap(), 0);
        assert!(ambiguous_pending(&conn).unwrap());
    }

    #[test]
    fn wealth_unknown_does_not_block_bamboo() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO order_intents (id, client_order_id, symbol, side, requested_qty, venue, state, external_order_ref)
             VALUES ('w', 'c3', 'ZENITHBANK', 'SELL', 4, 'wealth', 'unknown', '274163')",
            [],
        )
        .unwrap();
        assert!(ambiguous_pending_on(&conn, Some("wealth")).unwrap());
        assert!(!ambiguous_pending_on(&conn, Some("bamboo")).unwrap());
        assert!(ambiguous_pending(&conn).unwrap());
    }

    #[test]
    fn reject_abandoned_created_clears_cash_reservation() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO order_intents (id, client_order_id, symbol, side, requested_qty, requested_notional, venue, state)
             VALUES ('c', 'c4', 'UACN', 'BUY', 1, 166.9, 'bamboo', 'created')",
            [],
        )
        .unwrap();
        assert!((pending_buy_notional_on(&conn, Some("bamboo")).unwrap() - 166.9).abs() < 0.01);
        let n = reject_abandoned_created(&conn, Some("bamboo")).unwrap();
        assert_eq!(n, 1);
        assert_eq!(pending_buy_notional_on(&conn, Some("bamboo")).unwrap(), 0.0);
        assert!(load_open_intents_on(&conn, Some("bamboo")).unwrap().is_empty());
    }
}
