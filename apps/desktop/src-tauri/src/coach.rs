//! Coach 2.0: durable sessions, read/write tools, confirm-gated trades.

use std::cell::RefCell;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::db::Database;
use crate::execution::{remaining_buy_budget, BAMBOO_MIN_ORDER_NOTIONAL};
use crate::indicators::IndicatorService;
use crate::settings::AppSettings;
use crate::signals::SignalGenerationService;

const TITLE_MAX: usize = 72;

thread_local! {
    static ACTIVE_SESSION: RefCell<Option<String>> = const { RefCell::new(None) };
    static ACTIVE_INTENT: RefCell<Option<(String, Vec<String>)>> = const { RefCell::new(None) };
}

pub fn with_active_session<T>(id: &str, f: impl FnOnce() -> T) -> T {
    ACTIVE_SESSION.with(|s| *s.borrow_mut() = Some(id.to_string()));
    let out = f();
    ACTIVE_SESSION.with(|s| *s.borrow_mut() = None);
    out
}

pub fn with_active_intent<T>(class: &str, allowed: &[&str], f: impl FnOnce() -> T) -> T {
    ACTIVE_INTENT.with(|s| {
        *s.borrow_mut() = Some((
            class.to_string(),
            allowed.iter().map(|n| (*n).to_string()).collect(),
        ));
    });
    let out = f();
    ACTIVE_INTENT.with(|s| *s.borrow_mut() = None);
    out
}

fn active_session_id() -> Option<String> {
    ACTIVE_SESSION.with(|s| s.borrow().clone())
}

/// Agent loop may never execute or apply; UI confirm/apply commands do that.
pub fn tool_refused_for_active_intent(name: &str) -> bool {
    crate::coach_intent::is_irreversible_coach_tool(name)
}

pub fn list_sessions(conn: &Connection) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at FROM coach_sessions
         ORDER BY updated_at DESC LIMIT 40",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "title": row.get::<_, String>(1)?,
            "createdAt": row.get::<_, String>(2)?,
            "updatedAt": row.get::<_, String>(3)?,
        }))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn create_session(conn: &Connection) -> Result<Value> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO coach_sessions (id, title) VALUES (?1, 'New chat')",
        [&id],
    )?;
    load_session(conn, &id)
}

pub fn delete_session(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM coach_messages WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM coach_trade_proposals WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM coach_sessions WHERE id = ?1", [id])?;
    Ok(())
}

pub fn latest_session_id(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM coach_sessions ORDER BY updated_at DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn load_session(conn: &Connection, id: &str) -> Result<Value> {
    let (title, created, updated): (String, String, String) = conn.query_row(
        "SELECT title, created_at, updated_at FROM coach_sessions WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let mut stmt = conn.prepare(
        "SELECT id, role, text, payload, created_at FROM coach_messages
         WHERE session_id = ?1 ORDER BY created_at ASC, rowid ASC",
    )?;
    let mut messages: Vec<Value> = stmt
        .query_map([id], |row| {
            let payload_raw: Option<String> = row.get(3)?;
            let payload = payload_raw
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or(Value::Null);
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "role": row.get::<_, String>(1)?,
                "text": row.get::<_, String>(2)?,
                "payload": payload,
                "createdAt": row.get::<_, String>(4)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();
    for m in &mut messages {
        let Some(tid) = m
            .pointer("/payload/trade/id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        if let Ok(Some(p)) = load_proposal(conn, &tid) {
            if let Some(obj) = m.get_mut("payload").and_then(|v| v.as_object_mut()) {
                obj.insert("trade".into(), p);
            }
        }
    }
    Ok(json!({
        "id": id,
        "title": title,
        "createdAt": created,
        "updatedAt": updated,
        "messages": messages,
    }))
}

pub fn append_message(
    conn: &Connection,
    session_id: &str,
    role: &str,
    text: &str,
    payload: Option<&Value>,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let payload_s = payload.map(|v| v.to_string());
    conn.execute(
        "INSERT INTO coach_messages (id, session_id, role, text, payload)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, session_id, role, text, payload_s],
    )?;
    if role == "user" {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM coach_messages WHERE session_id = ?1 AND role = 'user'",
            [session_id],
            |r| r.get(0),
        )?;
        if count == 1 {
            let title = truncate_title(text);
            conn.execute(
                "UPDATE coach_sessions SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![title, session_id],
            )?;
        } else {
            conn.execute(
                "UPDATE coach_sessions SET updated_at = datetime('now') WHERE id = ?1",
                [session_id],
            )?;
        }
    } else {
        conn.execute(
            "UPDATE coach_sessions SET updated_at = datetime('now') WHERE id = ?1",
            [session_id],
        )?;
    }
    Ok(id)
}

fn truncate_title(text: &str) -> String {
    let t = text.trim().replace('\n', " ");
    if t.chars().count() <= TITLE_MAX {
        if t.is_empty() {
            "New chat".into()
        } else {
            t
        }
    } else {
        format!("{}…", t.chars().take(TITLE_MAX - 1).collect::<String>())
    }
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
}

fn arg_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|n| n as i64)))
        .unwrap_or(default)
}

fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64()).filter(|n| n.is_finite())
}

pub fn handle_tool(db: &Database, settings: &AppSettings, name: &str, args: &Value) -> Value {
    if tool_refused_for_active_intent(name) {
        return json!({
            "ok": false,
            "refused": true,
            "error": format!("{name} must be confirmed in the UI, not called from chat"),
        });
    }
    let result = db.with_conn(|conn| dispatch(conn, settings, name, args));
    match result {
        Ok(v) => v,
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn dispatch(
    conn: &Connection,
    settings: &AppSettings,
    name: &str,
    args: &Value,
) -> Result<Value> {
    match name {
        "get_account_snapshot" => Ok(get_account_snapshot(conn, settings)?),
        "get_holdings" => Ok(get_holdings(conn, settings)?),
        "get_recent_trades" => {
            let n = arg_i64(args, "limit", 20).clamp(1, 50);
            Ok(json!({
                "ok": true,
                "trades": crate::strategy_coach::load_recent_trades(conn, n)?,
            }))
        }
        "get_strategy_params" => {
            let row = crate::strategy_coach::read_active_strategy(conn)?;
            Ok(json!({
                "ok": true,
                "params": {
                    "max_position_pct": row.params.max_position_pct,
                    "cycle_budget_pct": row.params.cycle_budget_pct,
                    "min_confidence_to_trade": row.params.min_confidence_to_trade,
                    "max_daily_drawdown_pct": row.params.max_daily_drawdown_pct,
                    "stop_loss_pct": row.params.stop_loss_pct,
                    "take_profit_pct": row.params.take_profit_pct,
                    "time_stop_hours": row.params.time_stop_hours,
                    "partial_tp_fraction": row.params.partial_tp_fraction,
                },
            }))
        }
        "list_universe_quotes" => Ok(list_universe_quotes(conn, arg_i64(args, "limit", 40))?),
        "get_symbol_quote" => {
            let Some(symbol) = arg_str(args, "symbol") else {
                return Ok(json!({ "ok": false, "error": "symbol is required" }));
            };
            Ok(get_symbol_quote(conn, &symbol)?)
        }
        "get_price_history" => {
            let Some(symbol) = arg_str(args, "symbol") else {
                return Ok(json!({ "ok": false, "error": "symbol is required" }));
            };
            let days = arg_i64(args, "days", 30).clamp(5, 90);
            Ok(get_price_history(conn, &symbol, days)?)
        }
        "get_indicators" => {
            let Some(symbol) = arg_str(args, "symbol") else {
                return Ok(json!({ "ok": false, "error": "symbol is required" }));
            };
            match IndicatorService::compute(conn, &symbol)? {
                Some(snap) => Ok(json!({ "ok": true, "symbol": symbol, "indicators": snap })),
                None => Ok(json!({
                    "ok": false,
                    "error": format!("{symbol}: insufficient price history for RSI/SMA"),
                })),
            }
        }
        "search_memory" => Ok(json!({
            "ok": false,
            "error": "use memory_search via host",
        })),
        "get_news" => Ok(json!({
            "ok": false,
            "unavailable": true,
            "error": "NGX news is not wired in this build. No headlines were invented.",
        })),
        "run_symbol_screen" => Ok(run_symbol_screen(conn, args)?),
        "explain_blocked_reason" => {
            let Some(symbol) = arg_str(args, "symbol") else {
                return Ok(json!({ "ok": false, "error": "symbol is required" }));
            };
            Ok(explain_blocked(conn, &symbol)?)
        }
        "get_cycle_status" => Ok(json!({
            "ok": true,
            "autoCycleEnabled": settings.auto_cycle_enabled,
            "liveTradingEnabled": settings.live_trading_enabled,
            "haltNewBuys": settings.halt_new_buys,
            "intervalMinutes": settings.auto_cycle_interval_minutes,
        })),
        "propose_strategy_patch" => Ok(propose_strategy_patch(conn, args)?),
        "apply_strategy_patch" => Ok(json!({
            "ok": false,
            "error": "Strategy changes apply only after the user clicks Apply selected on the diff card.",
        })),
        "propose_trade" => Ok(propose_trade(conn, settings, args)?),
        "execute_trade" => Ok(execute_trade_tool_refused()),
        _ => Ok(json!({ "ok": false, "error": format!("unknown tool {name}") })),
    }
}

pub fn execute_trade_tool_refused() -> Value {
    json!({
        "ok": false,
        "error": "Execution requires the user to press Confirm on the trade card. Chat cannot place orders.",
    })
}

fn sandbox_cash(conn: &Connection) -> f64 {
    conn.query_row(
        "SELECT cash_balance FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0.0)
}

fn last_price(conn: &Connection, symbol: &str) -> Option<(f64, String, Option<f64>, Option<i64>)> {
    conn.query_row(
        "SELECT price, trade_date, change_percent, volume FROM price_history
         WHERE symbol = ?1 ORDER BY trade_date DESC LIMIT 1",
        [symbol],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .ok()
}

fn cached_live_book(
    conn: &Connection,
    settings: &AppSettings,
) -> Option<crate::wealth::CachedWealthBook> {
    if crate::broker::busha_connected() {
        return crate::busha::load_snapshot(conn).ok().flatten();
    }
    if settings.selected_broker == "bamboo" {
        crate::bamboo::load_snapshot(conn).ok().flatten()
    } else {
        crate::wealth::load_snapshot(conn).ok().flatten()
    }
}

fn get_account_snapshot(conn: &Connection, settings: &AppSettings) -> Result<Value> {
    let cash = sandbox_cash(conn);
    let broker = if crate::broker::busha_connected() {
        "busha".to_string()
    } else {
        settings.selected_broker.clone()
    };
    let min_n = crate::execution::min_order_notional_for_venue(&broker);
    let params = crate::strategy_coach::read_active_strategy(conn).ok();
    let cycle_pct = params
        .as_ref()
        .map(|r| r.params.cycle_budget_pct)
        .unwrap_or(0.20);
    let remaining = remaining_buy_budget(cash, cycle_pct, min_n);
    let live_book = cached_live_book(conn, settings);
    let (mode, live_cash, equity) = if let Some(book) = live_book {
        let mv = book.market_value();
        (
            if settings.live_trading_enabled {
                "live"
            } else {
                "sandbox"
            },
            Some(book.brokerage_balance),
            Some(book.brokerage_balance + mv),
        )
    } else {
        ("sandbox", None, None)
    };
    Ok(json!({
        "ok": true,
        "tradingMode": mode,
        "venue": broker,
        "liveTradingEnabled": settings.live_trading_enabled,
        "haltNewBuys": settings.halt_new_buys,
        "flattenOnDrawdownArmed": settings.flatten_on_drawdown_armed,
        "sandboxCash": cash,
        "liveCash": live_cash,
        "equity": equity,
        "spendableBudget": remaining,
        "bambooMinNotional": BAMBOO_MIN_ORDER_NOTIONAL,
        "minOrderNotional": min_n,
        "assetClass": crate::broker::asset_class().as_str(),
        "asOf": chrono::Utc::now().to_rfc3339(),
    }))
}

fn get_holdings(conn: &Connection, settings: &AppSettings) -> Result<Value> {
    let live_book = cached_live_book(conn, settings);
    if let Some(book) = live_book {
        let lots: Vec<Value> = book
            .holdings
            .iter()
            .filter(|h| h.quantity > 0.0)
            .map(|h| {
                let last = h.price;
                let cost = h.buy_price.unwrap_or(0.0);
                let upnl = if cost > 0.0 {
                    Some((last - cost) * h.quantity)
                } else {
                    None
                };
                json!({
                    "symbol": h.symbol,
                    "quantity": h.quantity,
                    "avgCost": cost,
                    "last": last,
                    "unrealizedPnl": upnl,
                    "source": "live-cache",
                })
            })
            .collect();
        return Ok(json!({ "ok": true, "holdings": lots }));
    }
    let pid: Option<String> = conn
        .query_row(
            "SELECT id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
    let Some(pid) = pid else {
        return Ok(json!({ "ok": true, "holdings": [] }));
    };
    let mut stmt = conn.prepare(
        "SELECT symbol, quantity, avg_cost FROM sandbox_positions WHERE portfolio_id = ?1 AND quantity > 0",
    )?;
    let mut lots = Vec::new();
    for row in stmt.query_map([&pid], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, f64>(1)?,
            row.get::<_, f64>(2)?,
        ))
    })? {
        let (symbol, qty, avg) = row?;
        let last = last_price(conn, &symbol).map(|p| p.0).unwrap_or(avg);
        lots.push(json!({
            "symbol": symbol,
            "quantity": qty,
            "avgCost": avg,
            "last": last,
            "unrealizedPnl": (last - avg) * qty,
            "source": "sandbox",
        }));
    }
    Ok(json!({ "ok": true, "holdings": lots }))
}

fn list_universe_quotes(conn: &Connection, limit: i64) -> Result<Value> {
    let limit = limit.clamp(5, 80);
    let mut stmt = conn.prepare(
        "SELECT i.symbol, i.name, i.sector, p.price, p.change_percent, p.volume, p.trade_date
         FROM instruments i
         JOIN price_history p ON p.symbol = i.symbol
         WHERE i.is_active = 1
           AND p.trade_date = (
             SELECT MAX(trade_date) FROM price_history ph WHERE ph.symbol = i.symbol
           )
         ORDER BY ABS(COALESCE(p.change_percent, 0)) DESC
         LIMIT ?1",
    )?;
    let rows: Vec<Value> = stmt
        .query_map([limit], |row| {
            Ok(json!({
                "symbol": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "sector": row.get::<_, Option<String>>(2)?,
                "price": row.get::<_, f64>(3)?,
                "changePercent": row.get::<_, Option<f64>>(4)?,
                "volume": row.get::<_, Option<i64>>(5)?,
                "asOf": row.get::<_, String>(6)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();
    if rows.is_empty() {
        return Ok(json!({
            "ok": false,
            "error": "No universe quotes in the local store. Ingest Pulse data or run a cycle first.",
        }));
    }
    Ok(json!({ "ok": true, "quotes": rows, "count": rows.len() }))
}

fn get_symbol_quote(conn: &Connection, symbol: &str) -> Result<Value> {
    let venue = if crate::broker::busha_connected() {
        "busha"
    } else {
        "sandbox"
    };
    if !crate::broker::is_valid_trade_symbol(venue, symbol) {
        return Ok(json!({ "ok": false, "error": format!("invalid ticker {symbol}") }));
    }
    match last_price(conn, symbol) {
        Some((price, date, chg, vol)) => Ok(json!({
            "ok": true,
            "symbol": symbol,
            "price": price,
            "changePercent": chg,
            "volume": vol,
            "asOf": date,
            "source": "price_history",
        })),
        None => Ok(json!({
            "ok": false,
            "error": format!("No cached quote for {symbol}"),
        })),
    }
}

fn get_price_history(conn: &Connection, symbol: &str, days: i64) -> Result<Value> {
    let venue = if crate::broker::busha_connected() {
        "busha"
    } else {
        "sandbox"
    };
    if !crate::broker::is_valid_trade_symbol(venue, symbol) {
        return Ok(json!({ "ok": false, "error": format!("invalid ticker {symbol}") }));
    }
    let mut stmt = conn.prepare(
        "SELECT trade_date, price, change_percent, volume FROM price_history
         WHERE symbol = ?1 ORDER BY trade_date DESC LIMIT ?2",
    )?;
    let mut bars: Vec<Value> = stmt
        .query_map(rusqlite::params![symbol, days], |row| {
            Ok(json!({
                "date": row.get::<_, String>(0)?,
                "close": row.get::<_, f64>(1)?,
                "changePercent": row.get::<_, Option<f64>>(2)?,
                "volume": row.get::<_, Option<i64>>(3)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();
    bars.reverse();
    if bars.is_empty() {
        return Ok(json!({
            "ok": false,
            "error": format!("No price history for {symbol}"),
        }));
    }
    Ok(json!({ "ok": true, "symbol": symbol, "bars": bars, "count": bars.len() }))
}

fn run_symbol_screen(conn: &Connection, args: &Value) -> Result<Value> {
    let listed = list_universe_quotes(conn, 80)?;
    if listed.get("ok") != Some(&json!(true)) {
        return Ok(listed);
    }
    let quotes = listed.get("quotes").cloned().unwrap_or(json!([]));
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("movers");
    Ok(json!({ "ok": true, "kind": kind, "quotes": quotes }))
}

fn explain_blocked(conn: &Connection, symbol: &str) -> Result<Value> {
    let row = conn.query_row(
        "SELECT risk_policy_result, action, generated_at FROM signals
         WHERE UPPER(symbol) = ?1 ORDER BY generated_at DESC LIMIT 1",
        [symbol],
        |r| {
            Ok(json!({
                "result": r.get::<_, String>(0)?,
                "action": r.get::<_, String>(1)?,
                "at": r.get::<_, String>(2)?,
            }))
        },
    );
    match row {
        Ok(v) => Ok(json!({ "ok": true, "symbol": symbol, "last": v })),
        Err(_) => Ok(json!({
            "ok": true,
            "symbol": symbol,
            "last": null,
            "note": "No signal on file for this symbol",
        })),
    }
}

fn propose_strategy_patch(conn: &Connection, args: &Value) -> Result<Value> {
    let current = crate::strategy_coach::read_active_strategy(conn)?.params;
    let mut map = std::collections::BTreeMap::new();
    if let Some(obj) = args.get("patch").and_then(|v| v.as_object()).or(args.as_object()) {
        for (k, v) in obj {
            if let Some(n) = v.as_f64() {
                if crate::strategy_coach::slider_bounds(k).is_some() {
                    map.insert(k.clone(), n);
                }
            }
        }
    }
    if map.is_empty() {
        return Ok(json!({ "ok": false, "error": "patch is empty" }));
    }
    let patch = crate::strategy_coach::StrategyPatch::from_map(&map);
    let merged = crate::strategy_coach::merge_patch(&current, &patch);
    if let Err(e) = crate::strategy_coach::validate_strategy_params(&merged) {
        return Ok(json!({ "ok": false, "error": e }));
    }
    Ok(json!({
        "ok": true,
        "saved": false,
        "note": "Not applied until the user confirms the diff card.",
        "patch": map,
    }))
}

fn propose_trade(conn: &Connection, settings: &AppSettings, args: &Value) -> Result<Value> {
    let Some(symbol) = arg_str(args, "symbol") else {
        return Ok(json!({ "ok": false, "error": "symbol is required" }));
    };
    if !crate::broker::is_valid_trade_symbol(
        if crate::broker::busha_connected() {
            "busha"
        } else {
            "sandbox"
        },
        &symbol,
    ) {
        return Ok(json!({ "ok": false, "error": format!("invalid ticker {symbol}") }));
    }
    let side = args
        .get("side")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_uppercase();
    if side != "BUY" && side != "SELL" {
        return Ok(json!({ "ok": false, "error": "side must be BUY or SELL" }));
    }
    let qty = arg_f64(args, "quantity").filter(|q| *q > 0.0);
    let notional = arg_f64(args, "notional").filter(|n| *n > 0.0);
    let rationale = args
        .get("rationale")
        .and_then(|v| v.as_str())
        .unwrap_or("Coach proposal")
        .to_string();
    let session_id = args
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(active_session_id)
        .unwrap_or_default();

    let quote = last_price(conn, &symbol);
    let Some((price, as_of, _, _)) = quote else {
        return Ok(json!({
            "ok": false,
            "error": format!("No cached quote for {symbol}; cannot preview a trade"),
        }));
    };
    let est_qty = qty.unwrap_or_else(|| {
        notional
            .map(|n| {
                if crate::broker::busha_connected() {
                    n / price
                } else {
                    (n / price).floor()
                }
            })
            .unwrap_or(0.0)
    });
    let est_notional = notional.unwrap_or(est_qty * price);
    let min_n = crate::execution::min_order_notional_for_venue(if crate::broker::busha_connected() {
        "busha"
    } else {
        &settings.selected_broker
    });
    let cash = sandbox_cash(conn);
    let mut warnings = Vec::new();
    if side == "BUY" && est_notional + 1e-9 < min_n && min_n > 0.0 {
        warnings.push(format!(
            "Notional ₦{est_notional:.0} is below the venue minimum ₦{min_n:.0}"
        ));
    }
    if side == "BUY" && cash + 1e-9 < min_n.max(est_notional) {
        warnings.push(format!("Spendable cash ₦{cash:.0} may be insufficient"));
    }
    if side == "BUY" && settings.halt_new_buys {
        warnings.push("Halt new buys is on — a confirmed BUY will be blocked".into());
    }
    if !settings.live_trading_enabled {
        warnings.push("Live trading is off — sandbox fill only if the book is sandbox".into());
    }

    let preview = json!({
        "symbol": symbol,
        "side": side,
        "quantity": est_qty,
        "notional": est_notional,
        "price": price,
        "asOf": as_of,
        "estimatedCost": est_notional,
        "minOrderNotional": min_n,
        "warnings": warnings,
        "liveTradingEnabled": settings.live_trading_enabled,
        "haltNewBuys": settings.halt_new_buys,
    });
    let id = Uuid::new_v4().to_string();
    if !session_id.is_empty() {
        conn.execute(
            "INSERT INTO coach_trade_proposals
             (id, session_id, symbol, side, quantity, notional, rationale, preview, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'proposed')",
            rusqlite::params![
                id,
                session_id,
                symbol,
                side,
                est_qty,
                est_notional,
                rationale,
                preview.to_string()
            ],
        )?;
    }
    Ok(json!({
        "ok": true,
        "proposalId": id,
        "status": "proposed",
        "preview": preview,
        "rationale": rationale,
        "placed": false,
    }))
}

/// Confirm + execute. Refuses unless status is `proposed`.
pub fn confirm_and_execute(
    db: &Database,
    settings: &AppSettings,
    proposal_id: &str,
) -> Result<Value> {
    let row: Option<(String, String, String, Option<f64>, Option<f64>, String, String)> =
        db.with_conn(|conn| {
            conn.query_row(
                "SELECT session_id, symbol, side, quantity, notional, rationale, status
                 FROM coach_trade_proposals WHERE id = ?1",
                [proposal_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()
            .map_err(anyhow::Error::from)
        })?;
    let Some((session_id, symbol, side, qty, _notional, rationale, status)) = row else {
        anyhow::bail!("Unknown trade proposal");
    };
    if status != "proposed" {
        anyhow::bail!("Proposal is not awaiting confirm (status={status})");
    }

    let signal_id = db.with_conn(|conn| {
        SignalGenerationService::persist_coach_trade(
            conn,
            &symbol,
            &side,
            &rationale,
            qty.unwrap_or(0.0),
        )
    })?;
    let Some(signal_id) = signal_id else {
        let result = json!({ "ok": false, "error": "Could not persist a valid signal" });
        mark_proposal(db, proposal_id, "blocked", &result)?;
        return Ok(result);
    };

    let broker = crate::broker::open_live_broker(settings);
    let calendar = crate::calendar::TradingCalendar::default();
    let trading_mode = if let Some(ref session) = broker {
        crate::runtime_util::block_on_local(session.resolve_trading_mode(settings))
    } else {
        crate::wealth::TradingMode::Sandbox
    };
    let live_open = if trading_mode == crate::wealth::TradingMode::Live {
        match broker.as_ref() {
            Some(session) => {
                crate::runtime_util::block_on_local(crate::runtime_util::live_broker_market_open(
                    session, &calendar,
                ))
            }
            None => false,
        }
    } else {
        true
    };
    let do_execute = settings.should_execute(trading_mode);
    let skip = settings.cycle_skip_result(trading_mode);
    let cache = crate::cache::PriceCache::new(60);
    let (executed, warnings) = crate::runtime_util::block_on_local(
        crate::execution::ExecutionService::process_signals(
            db,
            &cache,
            settings,
            &[signal_id.clone()],
            broker.as_ref(),
            trading_mode,
            live_open,
            do_execute,
            false,
            None,
            skip,
        ),
    )?;
    let policy: String = db.with_conn(|conn| {
        conn.query_row(
            "SELECT COALESCE(risk_policy_result, 'UNSET') FROM signals WHERE id = ?1",
            [&signal_id],
            |r| r.get(0),
        )
        .map_err(anyhow::Error::from)
    })?;
    let filled = executed > 0 && policy != "BLOCKED_LIVE_DISABLED";
    let status = if filled { "executed" } else { "blocked" };
    let result = json!({
        "ok": filled,
        "signalId": signal_id,
        "executed": executed,
        "riskPolicyResult": policy,
        "warnings": warnings,
        "tradingMode": trading_mode.as_str(),
        "sessionId": session_id,
    });
    mark_proposal(db, proposal_id, status, &result)?;
    Ok(result)
}

pub fn cancel_proposal(db: &Database, proposal_id: &str) -> Result<()> {
    let session_id: Option<String> = db.with_conn(|conn| {
        let n = conn.execute(
            "UPDATE coach_trade_proposals
             SET status = 'cancelled', updated_at = datetime('now')
             WHERE id = ?1 AND status = 'proposed'",
            [proposal_id],
        )?;
        if n == 0 {
            anyhow::bail!("Proposal is not awaiting confirm");
        }
        conn.query_row(
            "SELECT session_id FROM coach_trade_proposals WHERE id = ?1",
            [proposal_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(anyhow::Error::from)
    })?;
    if let Some(sid) = session_id {
        let payload = json!({ "proposalId": proposal_id, "tradeStatus": "cancelled" });
        db.with_conn(|conn| {
            append_message(conn, &sid, "assistant", "Trade proposal cancelled.", Some(&payload))
        })?;
    }
    Ok(())
}

fn mark_proposal(db: &Database, id: &str, status: &str, result: &Value) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE coach_trade_proposals
             SET status = ?1, result = ?2, updated_at = datetime('now')
             WHERE id = ?3",
            rusqlite::params![status, result.to_string(), id],
        )?;
        Ok(())
    })
}

pub fn load_proposal(conn: &Connection, id: &str) -> Result<Option<Value>> {
    conn.query_row(
        "SELECT id, session_id, symbol, side, quantity, notional, rationale, preview, status, result
         FROM coach_trade_proposals WHERE id = ?1",
        [id],
        |row| {
            let preview: String = row.get(7)?;
            let result_s: Option<String> = row.get(9)?;
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "sessionId": row.get::<_, String>(1)?,
                "symbol": row.get::<_, String>(2)?,
                "side": row.get::<_, String>(3)?,
                "quantity": row.get::<_, Option<f64>>(4)?,
                "notional": row.get::<_, Option<f64>>(5)?,
                "rationale": row.get::<_, Option<String>>(6)?,
                "preview": serde_json::from_str::<Value>(&preview).unwrap_or(json!({})),
                "status": row.get::<_, String>(8)?,
                "result": result_s.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
            }))
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn redact_args(args: &Value) -> Value {
    let mut out = args.clone();
    if let Some(obj) = out.as_object_mut() {
        for key in ["apiKey", "password", "token", "pin", "secret"] {
            if obj.contains_key(key) {
                obj.insert(key.to_string(), json!("[redacted]"));
            }
        }
    }
    out
}

pub fn summarize_tool_result(name: &str, result: &Value) -> String {
    if result.get("unavailable").and_then(|v| v.as_bool()) == Some(true) {
        return "unavailable".into();
    }
    if result.get("ok") == Some(&json!(false)) {
        return result
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("failed")
            .chars()
            .take(160)
            .collect();
    }
    match name {
        "list_universe_quotes" => format!(
            "{} quotes",
            result.get("count").and_then(|v| v.as_u64()).unwrap_or(0)
        ),
        "get_symbol_quote" => format!(
            "{} @ {}",
            result.get("symbol").and_then(|v| v.as_str()).unwrap_or("?"),
            result.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0)
        ),
        "propose_trade" => "proposal (not placed)".into(),
        _ => "ok".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn setup() -> (std::path::PathBuf, Database) {
        let dir = std::env::temp_dir().join(format!("ngx-coach2-{}", Uuid::new_v4()));
        let db = Database::open(&dir).unwrap();
        (dir, db)
    }

    #[test]
    fn session_survives_reload_shape() {
        let (dir, db) = setup();
        let session = db.with_conn(create_session).unwrap();
        let id = session["id"].as_str().unwrap().to_string();
        db.with_conn(|conn| {
            append_message(conn, &id, "user", "What's moving today?", None)?;
            append_message(
                conn,
                &id,
                "assistant",
                "GTCO led the tape.",
                Some(&json!({ "toolTrace": [{"name":"list_universe_quotes"}] })),
            )?;
            Ok(())
        })
        .unwrap();
        let loaded = db.with_conn(|conn| load_session(conn, &id)).unwrap();
        assert_eq!(loaded["title"], "What's moving today?");
        assert_eq!(loaded["messages"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn new_chat_does_not_wipe_old() {
        let (dir, db) = setup();
        let a = db.with_conn(create_session).unwrap();
        let aid = a["id"].as_str().unwrap().to_string();
        db.with_conn(|conn| append_message(conn, &aid, "user", "hello GTCO", None))
            .unwrap();
        let b = db.with_conn(create_session).unwrap();
        let listed = db.with_conn(list_sessions).unwrap();
        assert!(listed.len() >= 2);
        let still = db.with_conn(|conn| load_session(conn, &aid)).unwrap();
        assert_eq!(still["messages"].as_array().unwrap().len(), 1);
        assert_ne!(b["id"], a["id"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn execute_trade_tool_cannot_place() {
        let v = execute_trade_tool_refused();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().to_lowercase().contains("confirm"));
    }

    #[test]
    fn confirm_required_for_execute_path() {
        let (dir, db) = setup();
        let err = confirm_and_execute(&db, &AppSettings::default(), "missing").unwrap_err();
        assert!(err.to_string().contains("Unknown"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn get_news_is_honest_unavailable() {
        let (dir, db) = setup();
        let settings = AppSettings::default();
        let v = handle_tool(&db, &settings, "get_news", &json!({ "symbol": "GTCO" }));
        assert_eq!(v["ok"], false);
        assert_eq!(v["unavailable"], true);
        assert!(!v["error"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_tool_fails_closed() {
        let (dir, db) = setup();
        let v = handle_tool(&db, &AppSettings::default(), "invent_prices", &json!({}));
        assert_eq!(v["ok"], false);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn quote_missing_is_error_not_fake() {
        let (dir, db) = setup();
        let v = handle_tool(
            &db,
            &AppSettings::default(),
            "get_symbol_quote",
            &json!({ "symbol": "GTCO" }),
        );
        assert_eq!(v["ok"], false);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn propose_trade_does_not_fill() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name) VALUES ('GTCO', 'GTCO')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price, change_percent, volume)
                 VALUES ('GTCO', '2026-08-18', 50.0, 1.2, 1000)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let session = db.with_conn(create_session).unwrap();
        let sid = session["id"].as_str().unwrap();
        let v = handle_tool(
            &db,
            &AppSettings::default(),
            "propose_trade",
            &json!({
                "sessionId": sid,
                "symbol": "GTCO",
                "side": "BUY",
                "notional": 5000,
                "rationale": "test"
            }),
        );
        assert_eq!(v["ok"], true);
        assert_eq!(v["placed"], false);
        assert_eq!(v["status"], "proposed");
        let pid = v["proposalId"].as_str().unwrap();
        let status: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status FROM coach_trade_proposals WHERE id = ?1",
                    [pid],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)
            })
            .unwrap();
        assert_eq!(status, "proposed");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn universe_quotes_from_db() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name) VALUES ('GTCO', 'GTCO')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price, change_percent, volume)
                 VALUES ('GTCO', '2026-08-18', 50.0, 2.5, 10000)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let v = handle_tool(&db, &AppSettings::default(), "list_universe_quotes", &json!({}));
        assert_eq!(v["ok"], true);
        assert_eq!(v["quotes"][0]["symbol"], "GTCO");
        assert_eq!(v["quotes"][0]["price"], 50.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn price_history_matches_db() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name) VALUES ('MTNN', 'MTN')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price) VALUES ('MTNN', '2026-08-17', 220.0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price) VALUES ('MTNN', '2026-08-18', 225.0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let v = handle_tool(
            &db,
            &AppSettings::default(),
            "get_price_history",
            &json!({ "symbol": "MTNN", "days": 10 }),
        );
        assert_eq!(v["ok"], true);
        assert_eq!(v["bars"].as_array().unwrap().len(), 2);
        assert_eq!(v["bars"][1]["close"], 225.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn indicators_fail_closed_without_history() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name) VALUES ('GTCO', 'GTCO')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price) VALUES ('GTCO', '2026-08-18', 50.0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let v = handle_tool(
            &db,
            &AppSettings::default(),
            "get_indicators",
            &json!({ "symbol": "GTCO" }),
        );
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("insufficient"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn account_snapshot_does_not_invent_live_cash() {
        let (dir, db) = setup();
        let v = handle_tool(
            &db,
            &AppSettings::default(),
            "get_account_snapshot",
            &json!({}),
        );
        assert_eq!(v["ok"], true);
        assert_eq!(v["sandboxCash"], 0.0);
        assert!(v.get("liveCash").unwrap().is_null());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn execute_trade_tool_does_not_advance_proposal() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name) VALUES ('GTCO', 'GTCO')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price) VALUES ('GTCO', '2026-08-18', 50.0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let session = db.with_conn(create_session).unwrap();
        let sid = session["id"].as_str().unwrap();
        let proposed = handle_tool(
            &db,
            &AppSettings::default(),
            "propose_trade",
            &json!({
                "sessionId": sid,
                "symbol": "GTCO",
                "side": "BUY",
                "quantity": 10.0,
                "rationale": "test"
            }),
        );
        let pid = proposed["proposalId"].as_str().unwrap().to_string();
        let refused = handle_tool(
            &db,
            &AppSettings::default(),
            "execute_trade",
            &json!({ "proposalId": pid }),
        );
        assert_eq!(refused["ok"], false);
        let status: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status FROM coach_trade_proposals WHERE id = ?1",
                    [&pid],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)
            })
            .unwrap();
        assert_eq!(status, "proposed");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn confirm_respects_halt_new_buys() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name) VALUES ('GTCO', 'GTCO')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price, change_percent, volume)
                 VALUES ('GTCO', '2026-08-18', 50.0, 1.0, 1000)",
                [],
            )?;
            let param_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO strategy_param_sets (id, name, is_active)
                 VALUES (?1, 'default', 1)",
                rusqlite::params![param_id],
            )?;
            conn.execute(
                "INSERT INTO sandbox_portfolios (id, name, starting_capital, cash_balance, strategy_param_set_id)
                 VALUES (?1, 'default-sandbox', 10000, 10000, ?2)",
                rusqlite::params![Uuid::new_v4().to_string(), param_id],
            )?;
            Ok(())
        })
        .unwrap();
        let session = db.with_conn(create_session).unwrap();
        let sid = session["id"].as_str().unwrap();
        let proposed = handle_tool(
            &db,
            &AppSettings::default(),
            "propose_trade",
            &json!({
                "sessionId": sid,
                "symbol": "GTCO",
                "side": "BUY",
                "quantity": 20.0,
                "rationale": "test"
            }),
        );
        assert_eq!(proposed["placed"], false);
        let pid = proposed["proposalId"].as_str().unwrap().to_string();
        let mut settings = AppSettings::default();
        settings.halt_new_buys = true;
        let result = confirm_and_execute(&db, &settings, &pid).unwrap();
        assert_eq!(result["ok"], false);
        let policy = result["riskPolicyResult"].as_str().unwrap_or("");
        assert!(
            policy.starts_with("BLOCKED") || result.get("error").is_some(),
            "expected blocked reason, got {result}"
        );
        let again = confirm_and_execute(&db, &settings, &pid).unwrap_err();
        assert!(again.to_string().contains("not awaiting confirm"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn secrets_never_land_in_tool_args_log() {
        let redacted = redact_args(&json!({ "symbol": "GTCO", "apiKey": "sk-live", "token": "abc" }));
        assert_eq!(redacted["symbol"], "GTCO");
        assert_eq!(redacted["apiKey"], "[redacted]");
        assert_eq!(redacted["token"], "[redacted]");
    }

    #[test]
    fn agent_loop_refuses_irreversible_writes_only() {
        let (dir, db) = setup();
        let settings = AppSettings::default();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name) VALUES ('GTCO', 'GTCO')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price) VALUES ('GTCO', '2026-08-18', 46.2)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let quote = with_active_intent("conversation", crate::coach_intent::COACH_AGENT_TOOLS, || {
            handle_tool(
                &db,
                &settings,
                "get_symbol_quote",
                &json!({ "symbol": "GTCO" }),
            )
        });
        assert_ne!(quote.get("refused"), Some(&json!(true)));
        assert_ne!(quote["ok"], false);
        let snapshot = handle_tool(&db, &settings, "get_account_snapshot", &json!({}));
        assert_ne!(snapshot.get("refused"), Some(&json!(true)));
        let exec = handle_tool(
            &db,
            &settings,
            "execute_trade",
            &json!({ "proposalId": "x" }),
        );
        assert_eq!(exec["ok"], false);
        assert!(
            exec.get("refused") == Some(&json!(true))
                || exec["error"].as_str().unwrap_or("").contains("Confirm")
        );
        let apply = handle_tool(&db, &settings, "apply_strategy_patch", &json!({}));
        assert_eq!(apply["ok"], false);
        let unknown = handle_tool(&db, &settings, "invent_prices", &json!({}));
        assert_eq!(unknown["ok"], false);
        assert!(unknown.get("refused").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn research_intent_still_allows_quote() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name) VALUES ('GTCO', 'GTCO')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price) VALUES ('GTCO', '2026-08-18', 46.2)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let settings = AppSettings::default();
        let v = with_active_intent("conversation", crate::coach_intent::COACH_AGENT_TOOLS, || {
            handle_tool(
                &db,
                &settings,
                "get_symbol_quote",
                &json!({ "symbol": "GTCO" }),
            )
        });
        assert_ne!(v.get("refused"), Some(&json!(true)));
        let trade = handle_tool(
            &db,
            &settings,
            "execute_trade",
            &json!({
                "sessionId": "x",
                "symbol": "GTCO",
                "side": "BUY",
                "quantity": 1.0
            }),
        );
        assert_eq!(trade["ok"], false);
        let _ = std::fs::remove_dir_all(dir);
    }
}
