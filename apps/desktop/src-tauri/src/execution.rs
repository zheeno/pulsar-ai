use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cache::PriceCache;
use crate::db::Database;
use crate::intents;
use crate::ngx::is_valid_ticker;
use crate::broker::BrokerSession;
use crate::settings::AppSettings;
use crate::wealth::{insert_broker_order, TradingMode};

pub const MAX_QUOTE_DEVIATION: f64 = 0.05;
pub const MAX_LIQUIDATION_PCT: f64 = 0.25;
pub const MAX_SIGNAL_TOTAL: usize = 40;
pub const MAX_SIGNAL_BUYS: usize = 15;
pub const MAX_SIGNAL_SELLS: usize = 15;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSet {
    pub id: String,
    pub max_position_pct: f64,
    pub max_daily_trades: i64,
    pub stop_loss_pct: f64,
    pub min_confidence_to_trade: f64,
    pub max_daily_drawdown_pct: f64,
    pub position_size_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalInput {
    pub id: String,
    pub symbol: String,
    pub action: String,
    pub confidence: f64,
}

pub struct RiskPolicyService;

impl RiskPolicyService {
    pub fn evaluate(
        signal: &SignalInput,
        param_set: &ParamSet,
        cash_balance: f64,
        positions: &[(String, f64, f64)],
        prices: &std::collections::HashMap<String, f64>,
        daily_trades: i64,
        daily_drawdown_pct: f64,
        fee_pct: f64,
    ) -> (String, f64) {
        if signal.action == "HOLD" {
            return ("BLOCKED_OTHER".into(), 0.0);
        }
        if signal.confidence < param_set.min_confidence_to_trade {
            return ("BLOCKED_CONFIDENCE".into(), 0.0);
        }
        if signal.action == "BUY" && daily_trades >= param_set.max_daily_trades {
            return ("BLOCKED_DAILY_TRADES".into(), 0.0);
        }
        if signal.action == "BUY" && daily_drawdown_pct >= param_set.max_daily_drawdown_pct {
            return ("BLOCKED_DRAWDOWN".into(), 0.0);
        }

        let current_price = prices.get(&signal.symbol).copied().unwrap_or(0.0);
        let position = positions.iter().find(|(s, _, _)| s == &signal.symbol);
        let position_value = position.map(|(_, q, _)| q * current_price).unwrap_or(0.0);
        let market_value: f64 = positions
            .iter()
            .map(|(s, q, c)| q * prices.get(s).copied().unwrap_or(*c))
            .sum();
        let total_equity = cash_balance + market_value;

        if signal.action == "BUY" {
            let mut quantity =
                Self::position_size(total_equity, current_price, param_set, position_value);
            quantity = Self::cap_qty_by_cash(quantity, current_price, cash_balance, fee_pct);
            if quantity < 1.0 {
                return ("BLOCKED_CASH".into(), 0.0);
            }
            let new_exposure = if total_equity > 0.0 {
                (position_value + quantity * current_price) / total_equity
            } else {
                0.0
            };
            if new_exposure > param_set.max_position_pct || quantity <= 0.0 {
                return ("BLOCKED_EXPOSURE".into(), 0.0);
            }
            return ("APPROVED".into(), quantity);
        }

        if signal.action == "SELL" {
            if let Some((_, qty, _)) = position {
                let pending_sell = 0.0; // applied by caller via reduced qty
                let available = (*qty - pending_sell).max(0.0);
                if available > 0.0 {
                    return ("APPROVED".into(), available);
                }
            }
            return ("BLOCKED_OTHER".into(), 0.0);
        }

        ("BLOCKED_OTHER".into(), 0.0)
    }

    fn position_size(
        total_equity: f64,
        current_price: f64,
        param_set: &ParamSet,
        current_position_value: f64,
    ) -> f64 {
        if current_price <= 0.0 {
            return 0.0;
        }
        let target_value = total_equity * param_set.position_size_pct;
        let max_value = total_equity * param_set.max_position_pct - current_position_value;
        let alloc_value = target_value.min(max_value.max(0.0));
        (alloc_value / current_price).floor()
    }

    /// Cap quantity so (price * qty) * (1 + fee_pct) fits in spendable cash.
    pub fn cap_qty_by_cash(qty: f64, price: f64, cash: f64, fee_pct: f64) -> f64 {
        if price <= 0.0 || qty <= 0.0 || cash <= 0.0 {
            return 0.0;
        }
        let unit_cost = price * (1.0 + fee_pct.max(0.0));
        if unit_cost <= 0.0 {
            return 0.0;
        }
        let max_qty = (cash / unit_cost).floor();
        qty.min(max_qty).max(0.0)
    }
}

pub struct FillSimulator {
    pub slippage_bps: f64,
    pub fee_pct: f64,
}

impl FillSimulator {
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            slippage_bps: settings.simulated_slippage_bps,
            fee_pct: settings.simulated_fee_pct,
        }
    }

    pub fn simulate_fill(&self, side: &str, price: f64) -> (f64, f64) {
        let multiplier = if side == "BUY" {
            1.0 + self.slippage_bps / 10000.0
        } else {
            1.0 - self.slippage_bps / 10000.0
        };
        (price * multiplier, self.slippage_bps)
    }

    pub fn calculate_fee(&self, notional: f64) -> f64 {
        notional * self.fee_pct
    }
}

pub struct ExecutionService;

impl ExecutionService {
    pub async fn process_signals(
        db: &Database,
        cache: &PriceCache,
        settings: &AppSettings,
        signal_ids: &[String],
        broker: Option<&BrokerSession>,
        trading_mode: TradingMode,
        live_market_open: bool,
        execute: bool,
        allow_bulk_liquidation: bool,
        cycle_id: Option<&str>,
    ) -> Result<(i64, Vec<String>)> {
        let mut executed = 0;
        let mut warnings = Vec::new();
        let mut cycle_sell_notional = 0.0;

        if !execute {
            return Ok((0, warnings));
        }

        if trading_mode == TradingMode::Live && !live_market_open {
            warnings.push(
                "Live trader mode: brokerage market is closed — skipping live fills (no sandbox fallback)."
                    .into(),
            );
            return Ok((0, warnings));
        }

        let ambiguous = db.with_conn(intents::ambiguous_pending).unwrap_or(true);
        if trading_mode == TradingMode::Live && ambiguous {
            return Ok((
                0,
                vec!["Live orders are in an ambiguous state — reconcile before placing new orders.".into()],
            ));
        }

        for signal_id in signal_ids.iter().take(settings.max_live_actions.max(1) as usize) {
            match Self::process_signal(
                db,
                cache,
                settings,
                signal_id,
                broker,
                trading_mode,
                allow_bulk_liquidation,
                cycle_id,
                &mut cycle_sell_notional,
            )
            .await
            {
                Ok(true) => executed += 1,
                Ok(false) => {}
                Err(e) => warnings.push(format!("Signal {signal_id}: {e}")),
            }
        }
        Ok((executed, warnings))
    }

    async fn process_signal(
        db: &Database,
        cache: &PriceCache,
        settings: &AppSettings,
        signal_id: &str,
        broker: Option<&BrokerSession>,
        trading_mode: TradingMode,
        allow_bulk_liquidation: bool,
        cycle_id: Option<&str>,
        cycle_sell_notional: &mut f64,
    ) -> Result<bool> {
        let loaded = db.with_conn(|conn| {
            let signal: Option<(String, String, f64)> = conn
                .query_row(
                    "SELECT symbol, action, confidence FROM signals WHERE id = ?1",
                    [signal_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok();
            let Some((symbol, action, confidence)) = signal else {
                return Ok(None);
            };
            if !is_valid_ticker(&symbol) {
                conn.execute(
                    "UPDATE signals SET risk_policy_result = 'BLOCKED_SYMBOL' WHERE id = ?1",
                    [signal_id],
                )?;
                return Ok(None);
            }
            let portfolio: (String, f64, String) = conn.query_row(
                "SELECT id, cash_balance, strategy_param_set_id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let param_set = Self::load_param_set(conn, &portfolio.2)?;
            Ok(Some((symbol, action, confidence, portfolio, param_set)))
        })?;

        let Some((symbol, action, confidence, portfolio, param_set)) = loaded else {
            return Ok(false);
        };

        let input = SignalInput {
            id: signal_id.into(),
            symbol: symbol.clone(),
            action: action.clone(),
            confidence,
        };
        let fee_pct = settings.simulated_fee_pct.max(0.0);

        if trading_mode == TradingMode::Live {
            let client = broker.ok_or_else(|| anyhow::anyhow!("Live broker session required"))?;
            let (stock_id, broker_quote) = client.resolve_instrument(&symbol).await?;
            if broker_quote <= 0.0 {
                return Err(anyhow::anyhow!("No broker price for {symbol}"));
            }
            let wallet = client.get_wallet().await?;
            let snap = client.get_portfolio().await?;
            let current_equity = {
                let mv = if snap.stock_value > 0.0 {
                    snap.stock_value
                } else {
                    snap.holdings.iter().map(|h| h.current_value).sum()
                };
                wallet.brokerage_balance + mv
            };

            let eval = db.with_conn(|conn| {
                let pulse_price = Self::get_prices(conn, cache, &[symbol.clone()])?
                    .get(&symbol)
                    .copied()
                    .unwrap_or(0.0);
                if pulse_price > 0.0 && !quote_within_deviation(pulse_price, broker_quote) {
                    anyhow::bail!(
                        "Broker quote for {symbol} deviates more than {:.0}% from Pulse",
                        MAX_QUOTE_DEVIATION * 100.0
                    );
                }
                let pending_buy = intents::pending_buy_notional(conn)?;
                let pending_sell = intents::pending_sell_qty(conn, &symbol)?;
                if intents::find_duplicate(conn, &symbol, &action, 0.0)?.is_some() {
                    // duplicate check uses qty later
                }
                let mut positions: Vec<(String, f64, f64)> = snap
                    .holdings
                    .iter()
                    .map(|h| {
                        let mut qty = h.quantity;
                        if h.symbol.eq_ignore_ascii_case(&symbol) {
                            qty = (qty - pending_sell).max(0.0);
                        }
                        (h.symbol.clone(), qty, h.buy_price.unwrap_or(h.price))
                    })
                    .collect();
                if !positions.iter().any(|(s, _, _)| s == &symbol) {
                    positions.push((symbol.clone(), 0.0, broker_quote));
                }
                let mut prices = std::collections::HashMap::new();
                prices.insert(symbol.clone(), broker_quote);
                for h in &snap.holdings {
                    if h.price > 0.0 {
                        prices.insert(h.symbol.clone(), h.price);
                    }
                }
                let spendable = (wallet.brokerage_balance - pending_buy).max(0.0);
                let daily_trades = intents::pending_action_count(conn)?;
                let daily_drawdown = live_drawdown_pct(conn, client.id().as_str(), current_equity)?;
                let (result, mut quantity) = RiskPolicyService::evaluate(
                    &input,
                    &param_set,
                    spendable,
                    &positions,
                    &prices,
                    daily_trades,
                    daily_drawdown,
                    fee_pct,
                );
                conn.execute(
                    "UPDATE signals SET risk_policy_result = ?1 WHERE id = ?2",
                    rusqlite::params![result, signal_id],
                )?;
                if result != "APPROVED" || quantity <= 0.0 {
                    return Ok(None);
                }
                if action == "SELL" {
                    let cap = current_equity * MAX_LIQUIDATION_PCT;
                    if !allow_bulk_liquidation && *cycle_sell_notional + quantity * broker_quote > cap {
                        quantity = ((cap - *cycle_sell_notional).max(0.0) / broker_quote).floor();
                        if quantity < 1.0 {
                            anyhow::bail!("Bulk liquidation requires extra confirmation");
                        }
                    }
                }
                if let Some(_) = intents::find_duplicate(conn, &symbol, &action, quantity)? {
                    anyhow::bail!("Matching live order already submitted for {symbol}");
                }
                let intent = intents::insert_intent(
                    conn,
                    Some(signal_id),
                    cycle_id,
                    &symbol,
                    &action,
                    quantity,
                    broker_quote,
                )?;
                Ok(Some((intent, quantity, spendable)))
            })?;

            let Some((intent, mut quantity, spendable)) = eval else {
                return Ok(false);
            };

            let mut fee = client.calculate_fee(stock_id, quantity, broker_quote).await?;
            if action == "BUY" {
                let mut cost = broker_quote * quantity + fee.rounded_fee;
                while quantity >= 1.0 && spendable < cost {
                    quantity = (quantity - 1.0).floor();
                    if quantity < 1.0 {
                        break;
                    }
                    fee = client.calculate_fee(stock_id, quantity, broker_quote).await?;
                    cost = broker_quote * quantity + fee.rounded_fee;
                }
                if quantity < 1.0 {
                    db.with_conn(|conn| {
                        intents::mark_terminal(conn, &intent.id, "rejected", None, Some("insufficient cash"))
                    })?;
                    return Err(anyhow::anyhow!(
                        "Insufficient brokerage balance for {symbol} (have ₦{:.2})",
                        spendable
                    ));
                }
            }

            if quantity * broker_quote > settings.max_live_notional {
                db.with_conn(|conn| {
                    intents::mark_terminal(conn, &intent.id, "rejected", None, Some("notional cap"))
                })?;
                return Err(anyhow::anyhow!("Live notional exceeds configured cap"));
            }

            db.with_conn(|conn| intents::mark_submitted(conn, &intent.id, None))?;
            let order = match client
                .place_and_await_fill(stock_id, &action, quantity, Some(&intent.client_order_id))
                .await
            {
                Ok(o) => o,
                Err(e) => {
                    db.with_conn(|conn| {
                        intents::mark_terminal(conn, &intent.id, "unknown", None, Some(&e.to_string()))
                    })?;
                    return Err(e);
                }
            };

            db.with_conn(|conn| {
                let fill_price = order.unit_price.or(order.quote_price).or(Some(broker_quote));
                insert_broker_order(
                    conn,
                    Some(signal_id),
                    &symbol,
                    &action,
                    quantity,
                    &order,
                    fill_price,
                    Some(fee.rounded_fee),
                )?;
                let state = if order.status == "executed" {
                    "filled"
                } else if order.status == "rejected" {
                    "rejected"
                } else {
                    "unknown"
                };
                intents::mark_terminal(conn, &intent.id, state, Some(order.id), order.rejection_reason.as_deref())?;
                if order.status == "executed" {
                    conn.execute("UPDATE signals SET executed = 1 WHERE id = ?1", [signal_id])?;
                }
                Ok(())
            })?;

            if action == "SELL" {
                *cycle_sell_notional += quantity * broker_quote;
            }
            if order.status == "rejected" {
                return Err(anyhow::anyhow!(
                    "Broker order rejected: {}",
                    order
                        .rejection_reason
                        .unwrap_or_else(|| "unknown reason".into())
                ));
            }
            if order.status != "executed" {
                return Err(anyhow::anyhow!(
                    "Wealth order still {} after polling (id {})",
                    order.status,
                    order.id
                ));
            }
            return Ok(true);
        }

        db.with_conn(|conn| {
            let positions = Self::load_positions(conn, &portfolio.0)?;
            let mut symbols: Vec<String> = positions.iter().map(|(s, _, _)| s.clone()).collect();
            if !symbols.contains(&symbol) {
                symbols.push(symbol.clone());
            }
            let prices = Self::get_prices(conn, cache, &symbols)?;
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let daily_trades: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sandbox_trades
                 WHERE portfolio_id = ?1 AND date(executed_at) = ?2 AND side = 'BUY'",
                rusqlite::params![portfolio.0, today],
                |row| row.get(0),
            )?;
            let daily_drawdown: f64 = conn
                .query_row(
                    "SELECT drawdown_pct FROM daily_performance_snapshot WHERE portfolio_id = ?1 ORDER BY snapshot_date DESC LIMIT 1",
                    [&portfolio.0],
                    |row| row.get(0),
                )
                .unwrap_or(0.0);
            let (result, quantity) = RiskPolicyService::evaluate(
                &input,
                &param_set,
                portfolio.1,
                &positions,
                &prices,
                daily_trades,
                daily_drawdown,
                fee_pct,
            );
            conn.execute(
                "UPDATE signals SET risk_policy_result = ?1 WHERE id = ?2",
                rusqlite::params![result, signal_id],
            )?;
            if result != "APPROVED" || quantity <= 0.0 {
                return Ok(false);
            }
            let current_price = prices.get(&symbol).copied().unwrap_or(0.0);
            if current_price <= 0.0 {
                return Ok(false);
            }
            Self::execute_trade(
                conn,
                settings,
                &portfolio.0,
                signal_id,
                &symbol,
                &action,
                quantity,
                current_price,
                portfolio.1,
            )?;
            conn.execute("UPDATE signals SET executed = 1 WHERE id = ?1", [signal_id])?;
            Ok(true)
        })
    }

    fn execute_trade(
        conn: &Connection,
        settings: &AppSettings,
        portfolio_id: &str,
        signal_id: &str,
        symbol: &str,
        side: &str,
        quantity: f64,
        current_price: f64,
        mut cash_balance: f64,
    ) -> Result<()> {
        conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| {
        let fill_sim = FillSimulator::from_settings(settings);
        let (fill_price, slippage_bps) = fill_sim.simulate_fill(side, current_price);
        let notional = fill_price * quantity;
        let fee = fill_sim.calculate_fee(notional);

        if side == "BUY" {
            let total_cost = notional + fee;
            if cash_balance < total_cost {
                return Err(anyhow::anyhow!("Insufficient cash"));
            }
            cash_balance -= total_cost;

            let existing: Option<(String, f64, f64)> = conn
                .query_row(
                    "SELECT id, quantity, avg_cost FROM sandbox_positions WHERE portfolio_id = ?1 AND symbol = ?2",
                    rusqlite::params![portfolio_id, symbol],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok();

            if let Some((id, qty, avg)) = existing {
                let new_qty = qty + quantity;
                let new_avg = (avg * qty + fill_price * quantity) / new_qty;
                conn.execute(
                    "UPDATE sandbox_positions SET quantity = ?1, avg_cost = ?2, updated_at = datetime('now') WHERE id = ?3",
                    rusqlite::params![new_qty, new_avg, id],
                )?;
            } else {
                let id = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO sandbox_positions (id, portfolio_id, symbol, quantity, avg_cost) VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![id, portfolio_id, symbol, quantity, fill_price],
                )?;
            }
        } else {
            let pos: (String, f64) = conn.query_row(
                "SELECT id, quantity FROM sandbox_positions WHERE portfolio_id = ?1 AND symbol = ?2",
                rusqlite::params![portfolio_id, symbol],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            cash_balance += notional - fee;
            let new_qty = pos.1 - quantity;
            if new_qty <= 0.0 {
                conn.execute("DELETE FROM sandbox_positions WHERE id = ?1", [&pos.0])?;
            } else {
                conn.execute(
                    "UPDATE sandbox_positions SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
                    rusqlite::params![new_qty, pos.0],
                )?;
            }
        }

        conn.execute(
            "UPDATE sandbox_portfolios SET cash_balance = ?1 WHERE id = ?2",
            rusqlite::params![cash_balance, portfolio_id],
        )?;

        let trade_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO sandbox_trades (id, portfolio_id, signal_id, symbol, side, quantity, fill_price, simulated_fee, simulated_slippage_bps, resulting_cash_balance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![trade_id, portfolio_id, signal_id, symbol, side, quantity, fill_price, fee, slippage_bps, cash_balance],
        )?;
        Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute("COMMIT", [])?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(e)
            }
        }
    }

    fn load_param_set(conn: &Connection, id: &str) -> Result<ParamSet> {
        conn.query_row(
            "SELECT id, max_position_pct, max_daily_trades, stop_loss_pct, min_confidence_to_trade, max_daily_drawdown_pct, position_size_pct FROM strategy_param_sets WHERE id = ?1",
            [id],
            |row| {
                Ok(ParamSet {
                    id: row.get(0)?,
                    max_position_pct: row.get(1)?,
                    max_daily_trades: row.get(2)?,
                    stop_loss_pct: row.get(3)?,
                    min_confidence_to_trade: row.get(4)?,
                    max_daily_drawdown_pct: row.get(5)?,
                    position_size_pct: row.get(6)?,
                })
            },
        )
        .map_err(Into::into)
    }

    fn load_positions(conn: &Connection, portfolio_id: &str) -> Result<Vec<(String, f64, f64)>> {
        let mut stmt = conn.prepare(
            "SELECT symbol, quantity, avg_cost FROM sandbox_positions WHERE portfolio_id = ?1",
        )?;
        let rows = stmt.query_map([portfolio_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    fn get_prices(
        conn: &Connection,
        cache: &PriceCache,
        symbols: &[String],
    ) -> Result<std::collections::HashMap<String, f64>> {
        let mut prices = std::collections::HashMap::new();
        for symbol in symbols {
            if let Some(cached) = cache.get_price(symbol) {
                prices.insert(symbol.clone(), cached.price);
                continue;
            }
            let price: Option<f64> = conn
                .query_row(
                    "SELECT price FROM price_history WHERE symbol = ?1 ORDER BY trade_date DESC LIMIT 1",
                    [symbol],
                    |row| row.get(0),
                )
                .ok();
            if let Some(p) = price {
                prices.insert(symbol.clone(), p);
            }
        }
        Ok(prices)
    }

    pub async fn reconcile_if_possible(db: &Database, session: &BrokerSession) -> Result<()> {
        let open = db.with_conn(crate::intents::load_open_intents)?;
        for intent in open {
            if let Some(eid) = intent.external_order_id {
                match session.get_order(eid).await {
                    Ok(order) => {
                        let state = if order.status == "executed" {
                            "filled"
                        } else if order.status == "rejected" {
                            "rejected"
                        } else {
                            "unknown"
                        };
                        db.with_conn(|conn| {
                            crate::intents::mark_terminal(
                                conn,
                                &intent.id,
                                state,
                                Some(order.id),
                                order.rejection_reason.as_deref(),
                            )
                        })?;
                    }
                    Err(e) => {
                        db.with_conn(|conn| {
                            crate::intents::mark_terminal(
                                conn,
                                &intent.id,
                                "unknown",
                                Some(eid),
                                Some(&e.to_string()),
                            )
                        })?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn quote_within_deviation(reference: f64, broker: f64) -> bool {
    if reference <= 0.0 || broker <= 0.0 || !reference.is_finite() || !broker.is_finite() {
        return false;
    }
    ((broker - reference) / reference).abs() <= MAX_QUOTE_DEVIATION
}

pub fn live_drawdown_pct(conn: &Connection, venue: &str, current_equity: f64) -> Result<f64> {
    if current_equity <= 0.0 || !current_equity.is_finite() {
        anyhow::bail!("Live drawdown breaker: current brokerage equity is unavailable");
    }
    let prior: Option<f64> = conn
        .query_row(
            "SELECT total_equity FROM equity_curve_points
             WHERE venue = ?1 AND date(recorded_at) < date('now')
             ORDER BY recorded_at DESC LIMIT 1",
            [venue],
            |row| row.get(0),
        )
        .optional()?;
    let Some(prior) = prior.filter(|p| *p > 0.0 && p.is_finite()) else {
        anyhow::bail!("Live drawdown breaker: prior brokerage equity is unavailable");
    };
    Ok(((prior - current_equity) / prior).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_stale_broker_quote() {
        assert!(quote_within_deviation(100.0, 104.0));
        assert!(!quote_within_deviation(100.0, 106.0));
        assert!(!quote_within_deviation(0.0, 10.0));
    }

    fn test_param_set() -> ParamSet {
        ParamSet {
            id: "test".into(),
            max_position_pct: 0.25,
            max_daily_trades: 2,
            stop_loss_pct: 0.08,
            min_confidence_to_trade: 0.5,
            max_daily_drawdown_pct: 0.05,
            position_size_pct: 0.05,
        }
    }

    #[test]
    fn daily_trade_cap_blocks_buy_not_sell() {
        let param_set = test_param_set();
        let prices = std::collections::HashMap::from([("GTCO".into(), 50.0)]);
        let positions = vec![("GTCO".into(), 100.0, 40.0)];
        let buy = SignalInput {
            id: "b".into(),
            symbol: "GTCO".into(),
            action: "BUY".into(),
            confidence: 0.8,
        };
        let sell = SignalInput {
            id: "s".into(),
            symbol: "GTCO".into(),
            action: "SELL".into(),
            confidence: 0.8,
        };
        let (buy_result, _) = RiskPolicyService::evaluate(
            &buy,
            &param_set,
            1_000_000.0,
            &positions,
            &prices,
            param_set.max_daily_trades,
            0.0,
            0.01,
        );
        let (sell_result, sell_qty) = RiskPolicyService::evaluate(
            &sell,
            &param_set,
            1_000_000.0,
            &positions,
            &prices,
            param_set.max_daily_trades,
            0.0,
            0.01,
        );
        assert_eq!(buy_result, "BLOCKED_DAILY_TRADES");
        assert_eq!(sell_result, "APPROVED");
        assert_eq!(sell_qty, 100.0);
    }
}

