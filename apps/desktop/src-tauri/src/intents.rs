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
    let v: f64 = conn.query_row(
        "SELECT COALESCE(SUM(requested_notional), 0) FROM order_intents
         WHERE side = 'BUY' AND state IN ('created', 'submitted', 'unknown')",
        [],
        |row| row.get(0),
    )?;
    Ok(v)
}

pub fn pending_sell_qty(conn: &Connection, symbol: &str) -> Result<f64> {
    let v: f64 = conn.query_row(
        "SELECT COALESCE(SUM(requested_qty), 0) FROM order_intents
         WHERE symbol = ?1 AND side = 'SELL' AND state IN ('created', 'submitted', 'unknown')",
        [symbol],
        |row| row.get(0),
    )?;
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
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM order_intents WHERE state = 'unknown'",
        [],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

pub fn load_open_intents(conn: &Connection) -> Result<Vec<OrderIntent>> {
    let mut stmt = conn.prepare(
        "SELECT id, client_order_id, external_order_id, external_order_ref, signal_id, symbol, side, requested_qty, requested_quote, requested_notional, state
         FROM order_intents WHERE state IN ('created', 'submitted', 'unknown')",
    )?;
    let rows = stmt.query_map([], |row| {
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
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn find_duplicate(
    conn: &Connection,
    symbol: &str,
    side: &str,
    qty: f64,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM order_intents
         WHERE symbol = ?1 AND side = ?2 AND requested_qty = ?3
           AND created_at >= datetime('now', '-10 minutes')
           AND state IN ('submitted', 'filled', 'unknown')
         LIMIT 1",
        rusqlite::params![symbol, side, qty],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}
