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

fn arg_raw(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn parse_quote_ts(raw: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d"))
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(raw).map(|d| d.naive_utc()))
        .ok()
}

/// NGX daily bars can be yesterday and still current. Crypto marks go stale faster.
pub(crate) fn quote_freshness(as_of: &str, crypto: bool) -> (bool, Option<f64>) {
    let Some(ts) = parse_quote_ts(as_of) else {
        return (true, None);
    };
    let secs = (chrono::Utc::now().naive_utc() - ts).num_seconds() as f64;
    if !secs.is_finite() || secs < 0.0 {
        return (true, None);
    }
    let hours = secs / 3600.0;
    let stale = if crypto { hours > 2.0 } else { hours > 72.0 };
    (stale, Some(hours))
}

fn attach_freshness(obj: &mut Value, as_of: &str, crypto: bool) {
    let (stale, age) = quote_freshness(as_of, crypto);
    if let Some(map) = obj.as_object_mut() {
        map.insert("asOf".into(), json!(as_of));
        map.insert("stale".into(), json!(stale));
        map.insert("ageHours".into(), json!(age));
    }
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
        "search_memory" | "memory_search" => {
            let Some(query) = arg_raw(args, "query") else {
                return Ok(json!({ "ok": false, "error": "query is required" }));
            };
            let symbol = args
                .get("symbol")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let k = arg_i64(args, "k", 8).clamp(1, 40) as usize;
            let hits = crate::memory::keyword_search(conn, &query, symbol, k)?;
            Ok(json!({ "ok": true, "hits": hits, "source": "keyword" }))
        }
        "get_news" => {
            let symbol = arg_str(args, "symbol");
            let query = arg_str(args, "query");
            if crate::news::wants_crypto_news(
                symbol.as_deref(),
                query.as_deref(),
                crate::broker::crypto_mode(),
            ) {
                Ok(crate::runtime_util::block_on_local(
                    crate::news::coach_crypto_news(symbol.as_deref(), query.as_deref()),
                ))
            } else {
                Ok(json!({
                    "ok": false,
                    "unavailable": true,
                    "error": "NGX news is not wired in this build. Crypto headlines (BTC and other coins) use CoinDesk / Decrypt / The Block RSS — pass symbol or query. No headlines were invented.",
                }))
            }
        }
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
        "get_trade_lessons" => {
            let limit = arg_i64(args, "limit", 12).clamp(1, 30) as usize;
            let venue = if crate::broker::crypto_mode() {
                Some("busha")
            } else {
                None
            };
            Ok(json!({
                "ok": true,
                "lessons": crate::outcomes::desk_lessons(conn, venue, limit).unwrap_or_default(),
                "note": "Pattern evidence only. One loss is not a blacklist. Do not raise minConfidence.",
            }))
        }
        "get_dream_rules" => Ok(json!({
            "ok": true,
            "dreamRules": crate::dream::desk_dream_rules(conn).unwrap_or_default(),
            "note": "Standing cautions for NEW buys. Not a ticker ban, not a minConfidence knob, not a sell-now order.",
        })),
        "get_last_cycle" => {
            let rows = crate::signals::list_recent_cycle_audits(conn, 1)?;
            Ok(json!({
                "ok": true,
                "cycle": rows.first().cloned().unwrap_or(json!(null)),
                "note": "An empty signals array is valid. Do not treat a quiet cycle as a failure.",
            }))
        }
        "get_confidence_journal" => Ok(json!({
            "ok": true,
            "journal": crate::outcomes::confidence_journal(conn).unwrap_or_default(),
            "note": "Display only. Do not raise minConfidence from these buckets.",
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

/// Busha pair mark when `price_history` has no row (crypto desk / BTC).
fn busha_mark(conn: &Connection, symbol: &str) -> Option<(f64, String, Option<f64>)> {
    conn.query_row(
        "SELECT COALESCE(NULLIF(m.snapshot_price, 0), NULLIF(bp.buy_price, 0), bp.sell_price),
                m.change_pct,
                COALESCE(m.fetched_at, bp.synced_at)
         FROM busha_pairs bp
         LEFT JOIN busha_ohlc_meta m ON m.symbol = bp.symbol AND m.period = '1d'
         WHERE UPPER(bp.symbol) = UPPER(?1)",
        [symbol],
        |row| {
            Ok((
                row.get::<_, Option<f64>>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    )
    .ok()
    .and_then(|(px, chg, as_of)| {
        let px = px.filter(|p| *p > 0.0)?;
        Some((px, as_of.unwrap_or_default(), chg))
    })
}

fn latest_mark(
    conn: &Connection,
    symbol: &str,
) -> Option<(f64, String, Option<f64>, Option<i64>, bool)> {
    let crypto = crate::broker::crypto_mode() || crate::busha::is_crypto_symbol(conn, symbol);
    if let Some((price, date, chg, vol)) = last_price(conn, symbol) {
        return Some((price, date, chg, vol, crypto));
    }
    busha_mark(conn, symbol).map(|(price, date, chg)| (price, date, chg, None, true))
}

fn desk_venue(settings: &AppSettings) -> String {
    if crate::broker::crypto_mode() {
        "busha".into()
    } else {
        settings.selected_broker.clone()
    }
}

fn spendable_cash(conn: &Connection, settings: &AppSettings) -> f64 {
    cached_live_book(conn, settings)
        .map(|b| b.brokerage_balance)
        .unwrap_or_else(|| sandbox_cash(conn))
}

fn same_symbol_chase(conn: &Connection, symbol: &str) -> Result<bool> {
    let venue = if crate::broker::crypto_mode() {
        Some("busha")
    } else {
        None
    };
    let lessons = crate::outcomes::desk_lessons(conn, venue, 12).unwrap_or_default();
    Ok(lessons.iter().any(|row| {
        row.get("symbol").and_then(|v| v.as_str()) == Some(symbol)
            && row.get("pattern").and_then(|v| v.as_str()) == Some("chase_reversal")
    }))
}

fn standing_chase_warnings(conn: &Connection) -> Vec<String> {
    let mut warnings = Vec::new();
    let dreams = crate::dream::desk_dream_rules(conn).unwrap_or_default();
    if dreams.iter().any(|row| {
        row.get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("chase_reversal")
    }) {
        warnings.push(
            "Standing caution: a dream rule mentions chase_reversal. That is a pattern warning, not a ticker blacklist."
                .into(),
        );
    }
    let venue = if crate::broker::crypto_mode() {
        Some("busha")
    } else {
        None
    };
    let lessons = crate::outcomes::desk_lessons(conn, venue, 12).unwrap_or_default();
    if lessons.iter().any(|row| {
        row.get("pattern").and_then(|v| v.as_str()) == Some("chase_reversal")
            && row
                .get("repeatCount")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                >= 3
    }) {
        warnings.push(
            "Standing caution: chase_reversal has repeated at least three times on the desk. Still proposing because this name is not the prior loser."
                .into(),
        );
    }
    warnings
}

fn cached_live_book(
    conn: &Connection,
    settings: &AppSettings,
) -> Option<crate::wealth::CachedWealthBook> {
    if crate::broker::crypto_mode() {
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
    let broker = if crate::broker::crypto_mode() {
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
    let spendable = live_cash.unwrap_or(cash);
    let remaining = remaining_buy_budget(spendable, cycle_pct, min_n);
    let asset = crate::broker::asset_class().as_str();
    let sess = crate::broker::crypto_session().as_str();
    let as_of = chrono::Utc::now().to_rfc3339();
    let lede = format!(
        "Venue {broker}, {mode}, {asset}, cryptoSession={sess}, as-of {as_of}"
    );
    Ok(json!({
        "ok": true,
        "lede": lede,
        "tradingMode": mode,
        "venue": broker,
        "liveTradingEnabled": settings.live_trading_enabled,
        "haltNewBuys": settings.halt_new_buys,
        "flattenOnDrawdownArmed": settings.flatten_on_drawdown_armed,
        "sandboxCash": cash,
        "liveCash": live_cash,
        "spendableCash": spendable,
        "equity": equity,
        "spendableBudget": remaining,
        "bambooMinNotional": BAMBOO_MIN_ORDER_NOTIONAL,
        "minOrderNotional": min_n,
        "estimatedFeePct": crate::settings::fee_pct_for_venue(settings, &broker),
        "assetClass": asset,
        "cryptoSession": sess,
        "asOf": as_of,
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
    let crypto = crate::broker::crypto_mode();
    if crypto {
        return list_crypto_universe(conn, limit);
    }
    list_ngx_universe(conn, limit)
}

pub(crate) fn list_ngx_universe(conn: &Connection, limit: i64) -> Result<Value> {
    let limit = limit.clamp(5, 80);
    let mut stmt = conn.prepare(
        "SELECT i.symbol, i.name, i.sector, p.price, p.change_percent, p.volume, p.trade_date
         FROM instruments i
         JOIN price_history p ON p.symbol = i.symbol
         WHERE i.is_active = 1
           AND IFNULL(i.sector, '') != 'CRYPTO'
           AND p.trade_date = (
             SELECT MAX(trade_date) FROM price_history ph WHERE ph.symbol = i.symbol
           )
         ORDER BY ABS(COALESCE(p.change_percent, 0)) DESC
         LIMIT ?1",
    )?;
    let mut rows: Vec<Value> = stmt
        .query_map([limit], |row| {
            Ok(json!({
                "symbol": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "sector": row.get::<_, Option<String>>(2)?,
                "price": row.get::<_, f64>(3)?,
                "changePercent": row.get::<_, Option<f64>>(4)?,
                "volume": row.get::<_, Option<i64>>(5)?,
                "asOf": row.get::<_, String>(6)?,
                "source": "price_history",
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();
    for row in &mut rows {
        let as_of = row
            .get("asOf")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        attach_freshness(row, &as_of, false);
        row.as_object_mut()
            .map(|m| m.insert("rankNote".into(), json!("abs 1-day % is not an edge")));
    }
    if rows.is_empty() {
        return Ok(json!({
            "ok": false,
            "error": "No universe quotes in the local store. Ingest Pulse data or run a cycle first.",
        }));
    }
    Ok(json!({
        "ok": true,
        "desk": "ngx",
        "quotes": rows,
        "count": rows.len(),
        "rankNote": "Sorted by |1-day %|; that is not an edge.",
    }))
}

pub(crate) fn list_crypto_universe(conn: &Connection, limit: i64) -> Result<Value> {
    let limit = limit.clamp(5, 80);
    let mut stmt = conn.prepare(
        "SELECT bp.symbol,
                COALESCE(NULLIF(m.snapshot_price, 0), NULLIF(bp.buy_price, 0), bp.sell_price),
                m.change_pct,
                COALESCE(m.fetched_at, bp.synced_at)
         FROM busha_pairs bp
         LEFT JOIN busha_ohlc_meta m ON m.symbol = bp.symbol AND m.period = '1d'
         ORDER BY ABS(COALESCE(m.change_pct, 0)) DESC, bp.symbol ASC
         LIMIT ?1",
    )?;
    let mut rows: Vec<Value> = stmt
        .query_map([limit], |row| {
            Ok(json!({
                "symbol": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(0)?,
                "sector": "CRYPTO",
                "price": row.get::<_, Option<f64>>(1)?,
                "changePercent": row.get::<_, Option<f64>>(2)?,
                "volume": null,
                "asOf": row.get::<_, Option<String>>(3)?,
                "source": "busha",
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();
    for row in &mut rows {
        let as_of = row
            .get("asOf")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        attach_freshness(row, &as_of, true);
        row.as_object_mut()
            .map(|m| m.insert("rankNote".into(), json!("abs 1-day % is not an edge")));
    }
    if rows.is_empty() {
        return Ok(json!({
            "ok": false,
            "error": "No Busha pairs in the local store. Connect Busha or run a crypto cycle first.",
        }));
    }
    Ok(json!({
        "ok": true,
        "desk": "busha",
        "quotes": rows,
        "count": rows.len(),
        "rankNote": "Sorted by |1-day %|; that is not an edge.",
    }))
}

fn get_symbol_quote(conn: &Connection, symbol: &str) -> Result<Value> {
    let venue = if crate::broker::crypto_mode() {
        "busha"
    } else {
        "sandbox"
    };
    if !crate::broker::is_valid_trade_symbol(venue, symbol) {
        return Ok(json!({ "ok": false, "error": format!("invalid ticker {symbol}") }));
    }
    let indicators = IndicatorService::compute(conn, symbol)
        .ok()
        .flatten();
    match latest_mark(conn, symbol) {
        Some((price, date, chg, vol, crypto)) => {
            let mut out = json!({
                "ok": true,
                "symbol": symbol,
                "price": price,
                "changePercent": chg,
                "volume": vol,
                "source": if crypto { "busha_or_history" } else { "price_history" },
                "indicators": indicators,
            });
            attach_freshness(&mut out, &date, crypto);
            Ok(out)
        }
        None => Ok(json!({
            "ok": false,
            "error": format!("No cached quote for {symbol}"),
        })),
    }
}

fn get_price_history(conn: &Connection, symbol: &str, days: i64) -> Result<Value> {
    let venue = if crate::broker::crypto_mode() {
        "busha"
    } else {
        "sandbox"
    };
    if !crate::broker::is_valid_trade_symbol(venue, symbol) {
        return Ok(json!({ "ok": false, "error": format!("invalid ticker {symbol}") }));
    }

    if crate::broker::crypto_mode() && crate::busha::is_crypto_symbol(conn, symbol) {
        let period = if days <= 2 {
            crate::busha::BushaOhlcPeriod::OneDay
        } else {
            crate::busha::BushaOhlcPeriod::OneMonth
        };
        let limit = days.max(1).min(500);
        let points = crate::busha::ohlc_points(conn, symbol, period, limit)?;
        if !points.is_empty() {
            let bars: Vec<Value> = points
                .into_iter()
                .map(|(date, close)| {
                    json!({
                        "date": date,
                        "close": close,
                        "changePercent": null,
                        "volume": null,
                    })
                })
                .collect();
            return Ok(json!({
                "ok": true,
                "symbol": symbol,
                "bars": bars,
                "count": bars.len(),
                "source": "busha_ohlc",
            }));
        }
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
    Ok(json!({ "ok": true, "symbol": symbol, "bars": bars, "count": bars.len(), "source": "price_history" }))
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
        if crate::broker::crypto_mode() {
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

    if side == "BUY" && same_symbol_chase(conn, &symbol)? {
        return Ok(json!({
            "ok": false,
            "error": format!(
                "Refusing a BUY preview on {symbol}: this name has a chase_reversal close in the last 12 lots. That is a same-name caution, not a sector blacklist. A different symbol can still be previewed."
            ),
            "pattern": "chase_reversal",
            "scope": "same_symbol_only",
            "placed": false,
        }));
    }

    let quote = latest_mark(conn, &symbol);
    let Some((price, as_of, _, _, crypto)) = quote else {
        return Ok(json!({
            "ok": false,
            "error": format!("No cached quote for {symbol}; cannot preview a trade"),
        }));
    };
    let (stale, age_hours) = quote_freshness(&as_of, crypto);
    let est_qty = qty.unwrap_or_else(|| {
        notional
            .map(|n| {
                if crate::broker::crypto_mode() || crypto {
                    n / price
                } else {
                    (n / price).floor()
                }
            })
            .unwrap_or(0.0)
    });
    let est_notional = notional.unwrap_or(est_qty * price);
    let venue = desk_venue(settings);
    let min_n = crate::execution::min_order_notional_for_venue(&venue);
    let fee_pct = crate::settings::fee_pct_for_venue(settings, &venue);
    let estimated_fee = est_notional * fee_pct;
    let cash = spendable_cash(conn, settings);
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
    if stale {
        warnings.push(format!(
            "Quote as-of {as_of} is stale (ageHours={}). Do not treat this mark as live.",
            age_hours
                .map(|h| format!("{h:.1}"))
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    if side == "BUY" {
        warnings.extend(standing_chase_warnings(conn));
    }

    let preview = json!({
        "symbol": symbol,
        "side": side,
        "quantity": est_qty,
        "notional": est_notional,
        "price": price,
        "asOf": as_of,
        "stale": stale,
        "ageHours": age_hours,
        "venue": venue,
        "estimatedFeePct": fee_pct,
        "estimatedFee": estimated_fee,
        "estimatedCost": est_notional + estimated_fee,
        "spendableCash": cash,
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
        "spendableCash": cash,
        "venue": venue,
        "estimatedFeePct": fee_pct,
        "warning": warnings.join(" "),
        "warnings": warnings,
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
        assert!(crate::news::wants_crypto_news(Some("BTC"), None, false));
        assert!(!crate::news::wants_crypto_news(Some("GTCO"), None, false));
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

    fn seed_listed(conn: &Connection, symbol: &str, price: f64, as_of: &str) -> anyhow::Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO instruments (symbol, name) VALUES (?1, ?1)",
            [symbol],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO price_history (symbol, trade_date, price, change_percent, volume)
             VALUES (?1, ?2, ?3, 1.0, 1000)",
            rusqlite::params![symbol, as_of, price],
        )?;
        Ok(())
    }

    fn seed_sandbox(conn: &Connection, cash: f64) -> anyhow::Result<String> {
        let param_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO strategy_param_sets (id, name, is_active) VALUES (?1, 'default', 1)",
            rusqlite::params![param_id],
        )?;
        let pid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO sandbox_portfolios (id, name, starting_capital, cash_balance, strategy_param_set_id)
             VALUES (?1, 'default-sandbox', ?2, ?2, ?3)",
            rusqlite::params![pid, cash, param_id],
        )?;
        Ok(pid)
    }

    fn seed_chase_close(
        conn: &Connection,
        id: &str,
        symbol: &str,
        closed_at: &str,
        bought_at: &str,
        portfolio_id: &str,
    ) -> anyhow::Result<()> {
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES (?1, ?2, 'SELL', 0.6, 'close', '{}', 'openai:gpt-5.6-luna', 'v2.5.2', 'APPROVED', 1)",
            rusqlite::params![id, symbol],
        )?;
        conn.execute(
            "INSERT INTO signal_outcomes (
                id, signal_id, symbol, side, event, quantity, fill_price, fee,
                pnl, horizon_return_pct, confidence, venue, created_at
             ) VALUES (?1, ?1, ?2, 'SELL', 'close', 1, 8.32, 0, -1.2, -0.126, 0.6, 'wealth', ?3)",
            rusqlite::params![id, symbol, closed_at],
        )?;
        conn.execute(
            "INSERT INTO sandbox_trades (
                id, portfolio_id, symbol, side, quantity, fill_price, resulting_cash_balance, executed_at
             ) VALUES (?1, ?2, ?3, 'BUY', 1, 9.52, 1000, ?4)",
            rusqlite::params![format!("b-{id}"), portfolio_id, symbol, bought_at],
        )?;
        Ok(())
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

    #[test]
    fn quote_freshness_labels_unparseable_and_old() {
        assert_eq!(quote_freshness("not-a-date", false), (true, None));
        assert_eq!(quote_freshness("", true), (true, None));
        let old = quote_freshness("2020-01-01 00:00:00", false);
        assert!(old.0);
        assert!(old.1.unwrap() > 72.0);
        let recent = (chrono::Utc::now() - chrono::Duration::minutes(20))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let fresh_ngx = quote_freshness(&recent, false);
        assert!(!fresh_ngx.0);
        let stale_crypto = quote_freshness(&recent, true);
        assert!(!stale_crypto.0);
        let three_hours = (chrono::Utc::now() - chrono::Duration::hours(3))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert!(quote_freshness(&three_hours, true).0);
        assert!(!quote_freshness(&three_hours, false).0);
    }

    #[test]
    fn ngx_universe_excludes_crypto_and_marks_stale() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            seed_listed(conn, "GTCO", 50.0, "2026-08-18")?;
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name, sector) VALUES ('BTC', 'Bitcoin', 'CRYPTO')",
                [],
            )?;
            conn.execute(
                "INSERT INTO price_history (symbol, trade_date, price, change_percent, volume)
                 VALUES ('BTC', '2026-08-18', 100.0, 9.0, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let v = handle_tool(&db, &AppSettings::default(), "list_universe_quotes", &json!({}));
        assert_eq!(v["ok"], true);
        assert_eq!(v["desk"], "ngx");
        let quotes = v["quotes"].as_array().unwrap();
        assert!(quotes.iter().all(|q| q["symbol"] != "BTC"));
        assert_eq!(quotes[0]["symbol"], "GTCO");
        assert!(quotes[0].get("asOf").is_some());
        assert_eq!(quotes[0]["stale"], true);
        assert!(quotes[0].get("ageHours").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crypto_universe_reads_busha_pairs() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO busha_pairs (symbol, pair_id, buy_price, sell_price, synced_at)
                 VALUES ('BTC', 'btc-ngn', 150000000, 149000000, '2026-08-23T09:00:00Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO busha_ohlc_meta (symbol, period, snapshot_price, change_pct, fetched_at)
                 VALUES ('BTC', '1d', 150000000, 2.5, '2026-08-23T09:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let v = db
            .with_conn(|conn| list_crypto_universe(conn, 10))
            .unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["desk"], "busha");
        assert_eq!(v["quotes"][0]["symbol"], "BTC");
        assert_eq!(v["quotes"][0]["price"], 150000000.0);
        assert_eq!(v["quotes"][0]["stale"], true);
        assert!(v["quotes"][0].get("asOf").is_some());
        let quote = handle_tool(
            &db,
            &AppSettings::default(),
            "get_symbol_quote",
            &json!({ "symbol": "BTC" }),
        );
        assert_eq!(quote["ok"], true);
        assert_eq!(quote["price"], 150000000.0);
        assert_eq!(quote["stale"], true);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn search_memory_returns_lesson_hits() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            crate::memory::insert_memory(
                conn,
                "symbol_lesson",
                Some("HOME"),
                "LESSON close HOME venue=busha result=loss pattern=chase_reversal pnl=-12.6%",
                "trade_outcome",
                None,
            )?;
            Ok(())
        })
        .unwrap();
        let v = handle_tool(
            &db,
            &AppSettings::default(),
            "search_memory",
            &json!({ "query": "LESSON chase_reversal" }),
        );
        assert_eq!(v["ok"], true);
        assert_eq!(v["source"], "keyword");
        let hits = v["hits"].as_array().unwrap();
        assert!(!hits.is_empty());
        let text = hits[0]["text"].as_str().unwrap_or("");
        assert!(text.contains("LESSON"));
        assert!(text.contains("HOME"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn account_snapshot_lede_uses_live_cash_when_book_exists() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            seed_sandbox(conn, 99_999.0)?;
            conn.execute(
                "INSERT INTO wealth_account (id, brokerage_balance, stock_value, profit, synced_at)
                 VALUES (1, 1234.0, 0, 0, datetime('now'))",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let v = handle_tool(
            &db,
            &AppSettings::default(),
            "get_account_snapshot",
            &json!({}),
        );
        assert_eq!(v["ok"], true);
        let lede = v["lede"].as_str().unwrap();
        assert!(lede.contains("Venue wealth"));
        assert!(lede.contains("as-of"));
        assert_eq!(v["sandboxCash"], 99_999.0);
        assert_eq!(v["liveCash"], 1234.0);
        assert_eq!(v["spendableCash"], 1234.0);
        assert!(v.get("estimatedFeePct").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn desk_read_tools_do_not_write_blotter_or_strategy() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            seed_listed(conn, "GTCO", 50.0, "2026-08-23")?;
            seed_sandbox(conn, 10_000.0)?;
            conn.execute(
                "INSERT INTO cycle_audits (id, cycle_id, summary, expires_at, detail, blocked_histogram, cash, executed_ids)
                 VALUES ('a1', 'c1', 'signals=0 executed=0 venue=sandbox universe=4', datetime('now','+90 days'), '{\"signals\":[]}', '{}', 10000, '[]')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let before = db
            .with_conn(|conn| {
                let trades: i64 =
                    conn.query_row("SELECT COUNT(*) FROM sandbox_trades", [], |r| r.get(0))?;
                let params: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM strategy_param_sets",
                    [],
                    |r| r.get(0),
                )?;
                let proposals: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM coach_trade_proposals",
                    [],
                    |r| r.get(0),
                )?;
                let mem: i64 =
                    conn.query_row("SELECT COUNT(*) FROM agent_memories", [], |r| r.get(0))?;
                Ok::<_, anyhow::Error>((trades, params, proposals, mem))
            })
            .unwrap();
        let settings = AppSettings::default();
        for name in [
            "get_trade_lessons",
            "get_dream_rules",
            "get_last_cycle",
            "get_confidence_journal",
        ] {
            let v = handle_tool(&db, &settings, name, &json!({}));
            assert_eq!(v["ok"], true, "{name} {v}");
            assert!(
                v["note"].as_str().unwrap_or("").contains("not")
                    || v["note"].as_str().unwrap_or("").contains("Do not")
                    || v["note"].as_str().unwrap_or("").contains("valid"),
                "{name} note missing guardrail: {v}"
            );
        }
        assert!(handle_tool(&db, &settings, "get_last_cycle", &json!({}))["cycle"]["summary"]
            .as_str()
            .unwrap()
            .contains("signals=0"));
        let after = db
            .with_conn(|conn| {
                let trades: i64 =
                    conn.query_row("SELECT COUNT(*) FROM sandbox_trades", [], |r| r.get(0))?;
                let params: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM strategy_param_sets",
                    [],
                    |r| r.get(0),
                )?;
                let proposals: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM coach_trade_proposals",
                    [],
                    |r| r.get(0),
                )?;
                let mem: i64 =
                    conn.query_row("SELECT COUNT(*) FROM agent_memories", [], |r| r.get(0))?;
                Ok::<_, anyhow::Error>((trades, params, proposals, mem))
            })
            .unwrap();
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn propose_buy_refuses_same_symbol_chase_but_allows_other_names() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            seed_listed(conn, "HOME", 8.32, "2026-08-23")?;
            seed_listed(conn, "GTCO", 50.0, "2026-08-23")?;
            let pid = seed_sandbox(conn, 50_000.0)?;
            for i in 0..3 {
                seed_chase_close(
                    conn,
                    &format!("c{i}"),
                    "HOME",
                    &format!("2026-08-01T10:{i:02}:00Z"),
                    &format!("2026-08-01T09:{i:02}:00Z"),
                    &pid,
                )?;
            }
            crate::memory::insert_memory(
                conn,
                "freeform",
                None,
                "DREAM rule pattern=chase_reversal n=5 lookback=20. Standing caution for NEW buys.",
                "dream_consolidate",
                None,
            )?;
            Ok(())
        })
        .unwrap();
        let session = db.with_conn(create_session).unwrap();
        let sid = session["id"].as_str().unwrap();
        let settings = AppSettings::default();
        let refused = handle_tool(
            &db,
            &settings,
            "propose_trade",
            &json!({
                "sessionId": sid,
                "symbol": "HOME",
                "side": "BUY",
                "notional": 5000,
                "rationale": "re-enter HOME"
            }),
        );
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["pattern"], "chase_reversal");
        assert_eq!(refused["scope"], "same_symbol_only");
        assert!(refused["error"].as_str().unwrap().contains("HOME"));
        let proposals: i64 = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM coach_trade_proposals",
                    [],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)
            })
            .unwrap();
        assert_eq!(proposals, 0);

        let allowed = handle_tool(
            &db,
            &settings,
            "propose_trade",
            &json!({
                "sessionId": sid,
                "symbol": "GTCO",
                "side": "BUY",
                "notional": 5000,
                "rationale": "different name"
            }),
        );
        assert_eq!(allowed["ok"], true);
        assert_eq!(allowed["placed"], false);
        assert_eq!(allowed["preview"]["estimatedFeePct"], 0.005);
        assert!(allowed["preview"].get("venue").is_some());
        let warn = allowed["warning"].as_str().unwrap_or("");
        assert!(warn.contains("dream rule") || warn.contains("Standing caution"));
        assert!(warn.contains("three times") || warn.contains("repeat"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn propose_trade_spendable_prefers_live_book() {
        let (dir, db) = setup();
        db.with_conn(|conn| {
            seed_listed(conn, "GTCO", 50.0, "2026-08-23")?;
            seed_sandbox(conn, 99_999.0)?;
            conn.execute(
                "INSERT INTO wealth_account (id, brokerage_balance, stock_value, profit, synced_at)
                 VALUES (1, 400.0, 0, 0, datetime('now'))",
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
                "rationale": "size vs live cash"
            }),
        );
        assert_eq!(v["ok"], true);
        assert_eq!(v["spendableCash"], 400.0);
        let warn = v["warning"].as_str().unwrap_or("");
        assert!(warn.contains("insufficient"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
