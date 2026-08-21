use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cache::PriceCache;
use crate::db::Database;
use crate::intents;
use crate::broker::{insert_broker_order, is_valid_trade_symbol, BrokerSession};
use crate::settings::AppSettings;
use crate::wealth::TradingMode;

pub const MAX_QUOTE_DEVIATION: f64 = 0.05;
pub const MAX_LIQUIDATION_PCT: f64 = 0.25;
pub const MAX_SIGNAL_TOTAL: usize = 40;
pub const MAX_SIGNAL_BUYS: usize = 15;
pub const MAX_SIGNAL_SELLS: usize = 15;
/// Bamboo NGX market orders reject notionals below this floor.
pub const BAMBOO_MIN_ORDER_NOTIONAL: f64 = 5_000.0;
/// Typical Busha NGN conversion floor (~₦250). Per-pair mins/maxes from `/v1/pairs` override this.
pub const BUSHA_MIN_ORDER_NOTIONAL: f64 = 250.0;

pub fn min_order_notional_for_venue(venue: &str) -> f64 {
    if venue.eq_ignore_ascii_case("bamboo") {
        BAMBOO_MIN_ORDER_NOTIONAL
    } else if venue.eq_ignore_ascii_case("busha") {
        BUSHA_MIN_ORDER_NOTIONAL
    } else {
        0.0
    }
}

/// Cycle budget, raised to the broker minimum so a live account can still place
/// one legal order when 20% of cash is below that floor.
pub fn live_buy_budget(cash: f64, cycle_budget_pct: f64, min_notional: f64) -> f64 {
    let cycle = cash * RiskPolicyService::clamp_cycle_budget_pct(cycle_budget_pct);
    if min_notional <= 0.0 {
        return cycle;
    }
    if cash + 1e-9 < min_notional {
        return 0.0;
    }
    cycle.max(min_notional).min(cash)
}

pub fn max_buys_for_min_notional(budget: f64, min_notional: f64, cap: usize) -> usize {
    if min_notional <= 0.0 {
        return cap;
    }
    if budget + 1e-9 < min_notional {
        return 0;
    }
    ((budget / min_notional).floor() as usize).min(cap)
}

/// Per-cycle BUY budget from current spendable cash.
/// Fills already reduce the wallet, so today's buy notional is not subtracted again.
pub fn remaining_buy_budget(
    cash: f64,
    cycle_budget_pct: f64,
    min_notional: f64,
) -> f64 {
    live_buy_budget(cash, cycle_budget_pct, min_notional)
}

/// Why a new BUY cannot be funded this cycle. `None` means cash/budget may still
/// allow a legal order (sandbox/wealth still need a whole-share price check).
pub fn buy_ineligible_reason(
    spendable: f64,
    remaining_budget: f64,
    min_notional: f64,
) -> Option<&'static str> {
    if spendable <= 0.0 {
        return Some("insufficient spendable cash");
    }
    if min_notional > 0.0 && spendable + 1e-9 < min_notional {
        return Some("cash below Bamboo ₦5000 minimum");
    }
    if remaining_budget <= 0.0
        || (min_notional > 0.0 && remaining_budget + 1e-9 < min_notional)
    {
        return Some("daily spend budget exhausted");
    }
    None
}

/// Today's BUY notional at `venue` (submitted/filled/unknown).
/// Excludes rejected and abandoned `created` rows. Not used as a second spend cap.
pub fn todays_buy_notional(conn: &Connection, venue: &str) -> Result<f64> {
    let v: f64 = conn.query_row(
        "SELECT COALESCE(SUM(requested_notional), 0) FROM order_intents
         WHERE UPPER(side) = 'BUY' AND venue = ?1
           AND date(created_at) = date('now')
           AND LOWER(state) IN ('submitted', 'filled', 'unknown')",
        [venue],
        |row| row.get(0),
    )?;
    Ok(v)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSet {
    pub id: String,
    pub max_position_pct: f64,
    /// Legacy column; daily BUY caps are no longer enforced.
    pub max_daily_trades: i64,
    pub stop_loss_pct: f64,
    pub min_confidence_to_trade: f64,
    pub max_daily_drawdown_pct: f64,
    /// Legacy column; BUY sizing uses `cycle_budget_pct` instead.
    pub position_size_pct: f64,
    pub cycle_budget_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalInput {
    pub id: String,
    pub symbol: String,
    pub action: String,
    pub confidence: f64,
}

pub struct RiskPolicyService;

fn symbols_match(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl RiskPolicyService {
    pub fn evaluate(
        signal: &SignalInput,
        param_set: &ParamSet,
        cash_balance: f64,
        positions: &[(String, f64, f64)],
        prices: &std::collections::HashMap<String, f64>,
        daily_drawdown_pct: f64,
        fee_pct: f64,
        target_notional: Option<f64>,
        min_order_notional: f64,
        whole_shares: bool,
    ) -> (String, f64) {
        if signal.action == "HOLD" {
            return ("BLOCKED_OTHER".into(), 0.0);
        }
        if signal.confidence < param_set.min_confidence_to_trade {
            return ("BLOCKED_CONFIDENCE".into(), 0.0);
        }
        if signal.action == "BUY" && daily_drawdown_pct >= param_set.max_daily_drawdown_pct {
            return ("BLOCKED_DRAWDOWN".into(), 0.0);
        }

        let current_price = prices
            .get(&signal.symbol)
            .copied()
            .or_else(|| {
                prices
                    .iter()
                    .find(|(s, _)| symbols_match(s, &signal.symbol))
                    .map(|(_, p)| *p)
            })
            .unwrap_or(0.0);
        let position = positions
            .iter()
            .find(|(s, _, _)| symbols_match(s, &signal.symbol));
        let position_value = position.map(|(_, q, _)| q * current_price).unwrap_or(0.0);
        let market_value: f64 = positions
            .iter()
            .map(|(s, q, c)| q * prices.get(s).copied().unwrap_or(*c))
            .sum();
        let total_equity = cash_balance + market_value;

        if signal.action == "BUY" {
            let notional = target_notional.unwrap_or_else(|| {
                cash_balance * Self::clamp_cycle_budget_pct(param_set.cycle_budget_pct)
            });
            return Self::size_buy_from_notional(
                notional,
                current_price,
                cash_balance,
                total_equity,
                position_value,
                param_set.max_position_pct,
                fee_pct,
                min_order_notional,
                whole_shares,
            );
        }

        if signal.action == "SELL" {
            if let Some((_, qty, _)) = position {
                let pending_sell = 0.0; // applied by caller via reduced qty
                let available = (*qty - pending_sell).max(0.0);
                if available > 0.0 {
                    return ("APPROVED".into(), available);
                }
            }
            return ("BLOCKED_NO_POSITION".into(), 0.0);
        }

        ("BLOCKED_OTHER".into(), 0.0)
    }

    pub fn clamp_cycle_budget_pct(pct: f64) -> f64 {
        pct.clamp(0.05, 1.0)
    }

    /// Normalize confidences to weights that sum to 1 via √confidence (flatter than raw).
    /// Equal split if sum is 0.
    pub fn confidence_weights(confidences: &[f64]) -> Vec<f64> {
        if confidences.is_empty() {
            return Vec::new();
        }
        let roots: Vec<f64> = confidences.iter().map(|c| c.max(0.0).sqrt()).collect();
        let sum: f64 = roots.iter().copied().sum();
        if sum <= 0.0 {
            let w = 1.0 / confidences.len() as f64;
            return vec![w; confidences.len()];
        }
        roots.iter().map(|r| r / sum).collect()
    }

    pub fn size_buy_from_notional(
        target_notional: f64,
        current_price: f64,
        cash_balance: f64,
        total_equity: f64,
        position_value: f64,
        max_position_pct: f64,
        fee_pct: f64,
        min_order_notional: f64,
        whole_shares: bool,
    ) -> (String, f64) {
        if current_price <= 0.0 || target_notional <= 0.0 {
            return ("BLOCKED_CASH".into(), 0.0);
        }
        let lot = if whole_shares { 1.0 } else { 1e-8 };
        let mut quantity = if whole_shares {
            (target_notional / current_price).floor()
        } else {
            target_notional / current_price
        };
        if min_order_notional > 0.0 {
            let min_qty = if whole_shares {
                (min_order_notional / current_price).ceil()
            } else {
                min_order_notional / current_price
            };
            if min_qty > quantity {
                quantity = min_qty;
            }
        }
        if quantity + 1e-12 < lot {
            return ("BLOCKED_CASH".into(), 0.0);
        }
        let max_qty_by_exposure = if total_equity > 0.0 {
            let max_value = total_equity * max_position_pct - position_value;
            if whole_shares {
                (max_value.max(0.0) / current_price).floor()
            } else {
                (max_value.max(0.0) / current_price).max(0.0)
            }
        } else {
            0.0
        };
        let position_cap_notional = total_equity.max(0.0) * max_position_pct;
        if min_order_notional <= 0.0 || min_order_notional <= position_cap_notional + 1e-9 {
            quantity = quantity.min(max_qty_by_exposure);
        }
        if quantity + 1e-12 < lot {
            return ("BLOCKED_EXPOSURE".into(), 0.0);
        }
        quantity = if whole_shares {
            Self::cap_qty_by_cash(quantity, current_price, cash_balance, fee_pct)
        } else {
            Self::cap_qty_by_cash_frac(quantity, current_price, cash_balance, fee_pct)
        };
        if quantity + 1e-12 < lot {
            return ("BLOCKED_CASH".into(), 0.0);
        }
        let notional = quantity * current_price;
        if min_order_notional > 0.0 && notional + 1e-9 < min_order_notional {
            return ("BLOCKED_CASH".into(), 0.0);
        }
        let new_exposure = if total_equity > 0.0 {
            (position_value + notional) / total_equity
        } else {
            0.0
        };
        if new_exposure > max_position_pct {
            let min_breaches_cap =
                min_order_notional > 0.0 && min_order_notional > position_cap_notional + 1e-9;
            if !min_breaches_cap {
                return ("BLOCKED_EXPOSURE".into(), 0.0);
            }
        }
        ("APPROVED".into(), quantity)
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

    pub fn cap_qty_by_cash_frac(qty: f64, price: f64, cash: f64, fee_pct: f64) -> f64 {
        if price <= 0.0 || qty <= 0.0 || cash <= 0.0 {
            return 0.0;
        }
        let unit_cost = price * (1.0 + fee_pct.max(0.0));
        if unit_cost <= 0.0 {
            return 0.0;
        }
        let max_qty = cash / unit_cost;
        qty.min(max_qty).max(0.0)
    }

    /// True if cash can buy at least one whole share at `price` after fees.
    /// Used for pre-LLM capacity gates (not full position sizing).
    pub fn can_afford_one_share(cash: f64, price: f64, fee_pct: f64) -> bool {
        if price <= 0.0 || cash <= 0.0 {
            return false;
        }
        let unit_cost = price * (1.0 + fee_pct.max(0.0));
        unit_cost > 0.0 && cash >= unit_cost
    }

    /// True if the cycle cash budget can fund at least one whole share at `price`.
    pub fn can_fund_min_lot(
        cash: f64,
        total_equity: f64,
        price: f64,
        current_position_value: f64,
        param_set: &ParamSet,
        fee_pct: f64,
    ) -> bool {
        if cash <= 0.0 || total_equity <= 0.0 || price <= 0.0 {
            return false;
        }
        let budget = cash * Self::clamp_cycle_budget_pct(param_set.cycle_budget_pct);
        if !Self::can_afford_one_share(budget, price, fee_pct) {
            return false;
        }
        let one_share_exposure = (current_position_value + price) / total_equity;
        one_share_exposure <= param_set.max_position_pct
            && Self::cap_qty_by_cash(1.0, price, cash, fee_pct) >= 1.0
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
    fn mark_signal_results(
        conn: &Connection,
        signal_ids: &[String],
        result: &str,
    ) -> Result<()> {
        for signal_id in signal_ids {
            conn.execute(
                "UPDATE signals SET risk_policy_result = ?1 WHERE id = ?2 AND executed = 0",
                rusqlite::params![result, signal_id],
            )?;
        }
        Ok(())
    }

    fn mark_buys_capital_constrained(
        conn: &Connection,
        signal_ids: &[String],
        reason: &str,
    ) -> Result<Vec<String>> {
        let mut warnings = Vec::new();
        for signal_id in signal_ids {
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT symbol, action FROM signals WHERE id = ?1 AND executed = 0",
                    [signal_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((symbol, action)) = row else {
                continue;
            };
            if !action.eq_ignore_ascii_case("BUY") {
                continue;
            }
            conn.execute(
                "UPDATE signals SET risk_policy_result = 'BLOCKED_CASH' WHERE id = ?1 AND executed = 0",
                [signal_id],
            )?;
            warnings.push(format!("Skipped BUY {symbol}: {reason}"));
        }
        Ok(warnings)
    }

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
        skip_result: Option<&str>,
    ) -> Result<(i64, Vec<String>)> {
        let mut executed = 0;
        let mut warnings = Vec::new();
        let mut cycle_sell_notional = 0.0;

        if !execute {
            // Always overwrite the persist-time default (BLOCKED_OTHER) after an
            // execution decision, even if the caller omitted skip_result.
            let status = skip_result.unwrap_or("BLOCKED_NOT_EXECUTED");
            db.with_conn(|conn| Self::mark_signal_results(conn, signal_ids, status))?;
            return Ok((0, warnings));
        }

        if trading_mode == TradingMode::Live && !live_market_open {
            warnings.push(
                "Live trader mode: brokerage market is closed — skipping live fills (no sandbox fallback)."
                    .into(),
            );
            db.with_conn(|conn| {
                Self::mark_signal_results(conn, signal_ids, "BLOCKED_MARKET_CLOSED")
            })?;
            return Ok((0, warnings));
        }

        let venue = broker.map(|s| s.id().as_str().to_string());
        let ambiguous = db
            .with_conn(|conn| intents::ambiguous_pending_on(conn, venue.as_deref()))
            .unwrap_or(true);
        if trading_mode == TradingMode::Live && ambiguous {
            db.with_conn(|conn| {
                Self::mark_signal_results(conn, signal_ids, "BLOCKED_AMBIGUOUS_ORDERS")
            })?;
            return Ok((
                0,
                vec!["Live orders are in an ambiguous state — reconcile before placing new orders.".into()],
            ));
        }

        let mut skip_buy_reason: Option<&'static str> = None;
        let buy_targets = match trading_mode {
            TradingMode::Sandbox => db.with_conn(|conn| {
                Self::plan_sandbox_buy_targets(conn, cache, signal_ids)
            })?,
            TradingMode::Live => {
                if let Some(client) = broker {
                    let wallet = client.get_wallet().await?;
                    let snap = client.get_portfolio().await?;
                    let mv = if snap.stock_value > 0.0 {
                        snap.stock_value
                    } else {
                        snap.holdings.iter().map(|h| h.current_value).sum()
                    };
                    let equity = wallet.brokerage_balance + mv;
                    let (planned, skip) = db.with_conn(|conn| {
                        let pending_buy = intents::pending_buy_notional_on(conn, Some(client.id().as_str()))?;
                        let spendable = (wallet.brokerage_balance - pending_buy).max(0.0);
                        let min_n = min_order_notional_for_venue(client.id().as_str());
                        let daily_drawdown =
                            live_drawdown_pct(conn, client.id().as_str(), equity).unwrap_or(0.0);
                        let portfolio: Option<(String, String)> = conn
                            .query_row(
                                "SELECT id, strategy_param_set_id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
                                [],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .ok();
                        let Some((_, strategy_id)) = portfolio else {
                            return Ok((std::collections::HashMap::new(), None));
                        };
                        let param_set = Self::load_param_set(conn, &strategy_id)?;
                        let remaining = remaining_buy_budget(
                            spendable,
                            param_set.cycle_budget_pct,
                            min_n,
                        );
                        if let Some(reason) = buy_ineligible_reason(spendable, remaining, min_n) {
                            return Ok((std::collections::HashMap::new(), Some(reason)));
                        }
                        let targets = Self::plan_buy_targets(
                            conn,
                            cache,
                            signal_ids,
                            &param_set,
                            spendable,
                            daily_drawdown,
                            min_n,
                            remaining,
                            venue.as_deref().unwrap_or("sandbox"),
                        )?;
                        Ok((targets, None))
                    })?;
                    skip_buy_reason = skip;
                    planned
                } else {
                    std::collections::HashMap::new()
                }
            }
        };

        if let Some(reason) = skip_buy_reason {
            tracing::info!(target: "execution", reason, "capital-constrained");
            warnings.push(format!("capital-constrained: {reason}"));
            let extra = db.with_conn(|conn| {
                Self::mark_buys_capital_constrained(conn, signal_ids, reason)
            })?;
            warnings.extend(extra);
        }

        let cap = settings.max_live_actions.max(1) as usize;
        let (to_run, overflow) = if signal_ids.len() > cap {
            (&signal_ids[..cap], &signal_ids[cap..])
        } else {
            (signal_ids, &[][..])
        };

        for signal_id in to_run {
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
                buy_targets.get(signal_id).copied(),
                skip_buy_reason,
            )
            .await
            {
                Ok(true) => executed += 1,
                Ok(false) => {}
                Err(e) => {
                    let msg = e.to_string();
                    let status = classify_live_execution_error(&msg);
                    tracing::warn!(
                        target: "execution",
                        signal_id = %signal_id,
                        error = %msg,
                        status,
                        "live signal execution failed"
                    );
                    let _ = db.with_conn(|conn| {
                        Self::mark_signal_results(conn, std::slice::from_ref(signal_id), status)
                    });
                    warnings.push(format!("Signal {signal_id}: {msg}"));
                }
            }
        }
        if !overflow.is_empty() {
            db.with_conn(|conn| {
                Self::mark_signal_results(conn, overflow, "BLOCKED_NOT_EXECUTED")
            })?;
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
        buy_target_notional: Option<f64>,
        skip_buy_reason: Option<&str>,
    ) -> Result<bool> {
        let loaded = db.with_conn(|conn| {
            let signal: Option<(String, String, f64, String)> = conn
                .query_row(
                    "SELECT symbol, action, confidence, technical_snapshot FROM signals WHERE id = ?1",
                    [signal_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .ok();
            let Some((symbol, action, confidence, snapshot)) = signal else {
                return Ok(None);
            };
            if !is_valid_trade_symbol(
                broker.map(|s| s.id().as_str()).unwrap_or("sandbox"),
                &symbol,
            ) {
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
            Ok(Some((symbol, action, confidence, snapshot, portfolio, param_set)))
        })?;

        let Some((symbol, action, confidence, snapshot, portfolio, param_set)) = loaded else {
            return Ok(false);
        };

        let input = SignalInput {
            id: signal_id.into(),
            symbol: symbol.clone(),
            action: action.clone(),
            confidence,
        };
        let fee_pct = settings.simulated_fee_pct.max(0.0);
        if settings.halt_new_buys && action == "BUY" {
            db.with_conn(|conn| {
                conn.execute(
                    "UPDATE signals SET risk_policy_result = 'BLOCKED_NOT_EXECUTED' WHERE id = ?1 AND executed = 0",
                    [signal_id],
                )?;
                Ok(())
            })?;
            return Ok(false);
        }

        // BUY already failed Pass A qualify — risk_policy_result was written in the planner.
        // Capital-constrained cycles must not call the broker for BUYs at all.
        if action == "BUY" && (buy_target_notional.is_none() || skip_buy_reason.is_some()) {
            return Ok(false);
        }

        if trading_mode == TradingMode::Live {
            let client = broker.ok_or_else(|| anyhow::anyhow!("Live broker session required"))?;
            let instrument = client.resolve_instrument_for_side(&symbol, &action).await?;
            let broker_quote = instrument.quote;
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
                if quote_deviation_applies(&action)
                    && client.id() != crate::broker::BrokerId::Busha
                    && pulse_price > 0.0
                    && !quote_within_deviation(pulse_price, broker_quote)
                {
                    anyhow::bail!(
                        "Broker quote for {symbol} deviates more than {:.0}% from Pulse",
                        MAX_QUOTE_DEVIATION * 100.0
                    );
                }
                let pending_buy = intents::pending_buy_notional_on(conn, Some(client.id().as_str()))?;
                let pending_sell = intents::pending_sell_qty_on(conn, &symbol, Some(client.id().as_str()))?;
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
                if !positions
                    .iter()
                    .any(|(s, _, _)| symbols_match(s, &symbol))
                {
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
                let daily_drawdown = live_drawdown_pct(conn, client.id().as_str(), current_equity)?;
                let (result, mut quantity) = RiskPolicyService::evaluate(
                    &input,
                    &param_set,
                    spendable,
                    &positions,
                    &prices,
                    daily_drawdown,
                    fee_pct,
                    buy_target_notional,
                    min_order_notional_for_venue(client.id().as_str()),
                    client.id().whole_shares(),
                );
                if result == "APPROVED" && action == "SELL" {
                    let frac = sell_fraction_from_snapshot(&snapshot);
                    quantity = crate::risk_exits::sell_qty_for_exit_ex(
                        quantity,
                        frac,
                        client.id().whole_shares(),
                    );
                    if client.id().whole_shares() && quantity < 1.0 {
                        conn.execute(
                            "UPDATE signals SET risk_policy_result = 'BLOCKED_NO_POSITION' WHERE id = ?1",
                            [signal_id],
                        )?;
                        return Ok(None);
                    }
                    if !client.id().whole_shares() && quantity <= 0.0 {
                        conn.execute(
                            "UPDATE signals SET risk_policy_result = 'BLOCKED_NO_POSITION' WHERE id = ?1",
                            [signal_id],
                        )?;
                        return Ok(None);
                    }
                }
                conn.execute(
                    "UPDATE signals SET risk_policy_result = ?1 WHERE id = ?2",
                    rusqlite::params![result, signal_id],
                )?;
                if result != "APPROVED" || quantity <= 0.0 {
                    return Ok(None);
                }
                // Full-lot exits go in one order (stocks + crypto). The 25% equity
                // throttle only applies to partial / discretionary sells that would
                // otherwise dump the book across many names without confirmation.
                let full_lot_exit = action == "SELL"
                    && sell_fraction_from_snapshot(&snapshot) >= 1.0 - 1e-9;
                if action == "SELL" && !full_lot_exit {
                    let cap = current_equity * MAX_LIQUIDATION_PCT;
                    if !allow_bulk_liquidation
                        && *cycle_sell_notional + quantity * broker_quote > cap
                    {
                        let mut capped =
                            ((cap - *cycle_sell_notional).max(0.0) / broker_quote).max(0.0);
                        if client.id().whole_shares() {
                            capped = capped.floor();
                        }
                        quantity = capped;
                        if (client.id().whole_shares() && quantity < 1.0)
                            || (!client.id().whole_shares() && quantity <= 0.0)
                        {
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
                    client.id().as_str(),
                )?;
                Ok(Some((intent, quantity, spendable)))
            })?;

            let Some((intent, mut quantity, spendable)) = eval else {
                return Ok(false);
            };

            let mut fee = match client
                .calculate_fee(&instrument, &action, quantity, broker_quote)
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    db.with_conn(|conn| {
                        intents::mark_terminal(
                            conn,
                            &intent.id,
                            "rejected",
                            None,
                            Some(&e.to_string()),
                        )
                    })?;
                    return Err(e);
                }
            };
            if action == "BUY" {
                let target = match fit_live_buy_qty(
                    quantity,
                    spendable,
                    fee.total_price,
                    fee.price_per_share,
                    fee.available_quantity,
                    client.id().whole_shares(),
                ) {
                    Ok(q) => q,
                    Err(msg) => {
                        db.with_conn(|conn| {
                            intents::mark_terminal(conn, &intent.id, "rejected", None, Some(&msg))
                        })?;
                        return Err(anyhow::anyhow!(msg));
                    }
                };
                if target + 1e-9 < quantity {
                    tracing::info!(
                        target: "execution",
                        symbol = %symbol,
                        requested = quantity,
                        fitted = target,
                        spendable,
                        total_price = fee.total_price,
                        available_quantity = fee.available_quantity,
                        "resized live BUY before place (no per-share fee walk)"
                    );
                    quantity = target;
                    fee = match client
                        .calculate_fee(&instrument, &action, quantity, broker_quote)
                        .await
                    {
                        Ok(f) => f,
                        Err(e) => {
                            db.with_conn(|conn| {
                                intents::mark_terminal(
                                    conn,
                                    &intent.id,
                                    "rejected",
                                    None,
                                    Some(&e.to_string()),
                                )
                            })?;
                            return Err(e);
                        }
                    };
                }
                if spendable + 1e-9 < fee.total_price {
                    let target = match fit_live_buy_qty(
                        quantity,
                        spendable,
                        fee.total_price,
                        fee.price_per_share,
                        fee.available_quantity,
                        client.id().whole_shares(),
                    ) {
                        Ok(q) => q,
                        Err(msg) => {
                            db.with_conn(|conn| {
                                intents::mark_terminal(
                                    conn,
                                    &intent.id,
                                    "rejected",
                                    None,
                                    Some(&msg),
                                )
                            })?;
                            return Err(anyhow::anyhow!(msg));
                        }
                    };
                    if target + 1e-9 < quantity {
                        quantity = target;
                        fee = match client
                            .calculate_fee(&instrument, &action, quantity, broker_quote)
                            .await
                        {
                            Ok(f) => f,
                            Err(e) => {
                                db.with_conn(|conn| {
                                    intents::mark_terminal(
                                        conn,
                                        &intent.id,
                                        "rejected",
                                        None,
                                        Some(&e.to_string()),
                                    )
                                })?;
                                return Err(e);
                            }
                        };
                    }
                }
                if (client.id().whole_shares() && quantity < 1.0)
                    || spendable + 1e-9 < fee.total_price
                {
                    let msg = format!(
                        "Insufficient brokerage balance for {symbol} (have ₦{:.2}, need ₦{:.2}, qty {}, available {})",
                        spendable,
                        fee.total_price,
                        quantity,
                        fee.available_quantity
                    );
                    db.with_conn(|conn| {
                        intents::mark_terminal(conn, &intent.id, "rejected", None, Some(&msg))
                    })?;
                    return Err(anyhow::anyhow!(msg));
                }
            }

            let min_n = min_order_notional_for_venue(client.id().as_str());
            if action == "BUY" && min_n > 0.0 && client.id().whole_shares() {
                let pps = fee.price_per_share.max(broker_quote);
                if pps > 0.0 && fee.total_price + 1e-9 < min_n {
                    let need = (min_n / pps).ceil();
                    if need > quantity {
                        quantity = need;
                        fee = match client
                            .calculate_fee(&instrument, &action, quantity, broker_quote)
                            .await
                        {
                            Ok(f) => f,
                            Err(e) => {
                                db.with_conn(|conn| {
                                    intents::mark_terminal(
                                        conn,
                                        &intent.id,
                                        "rejected",
                                        None,
                                        Some(&e.to_string()),
                                    )
                                })?;
                                return Err(e);
                            }
                        };
                    }
                }
                if fee.total_price + 1e-9 < min_n || spendable + 1e-9 < fee.total_price {
                    let msg = format!(
                        "Bamboo minimum order is ₦{min_n:.0} (need ₦{:.2}, have ₦{:.2})",
                        fee.total_price.max(min_n),
                        spendable
                    );
                    db.with_conn(|conn| {
                        intents::mark_terminal(conn, &intent.id, "rejected", None, Some(&msg))
                    })?;
                    return Err(anyhow::anyhow!(msg));
                }
            }

            if fee.total_price.max(quantity * broker_quote) > settings.max_live_notional {
                // Full-lot SELLs must still go through in one order so exits are not
                // rejected / retried in fee-multiplying chunks.
                let full_lot_exit = action == "SELL"
                    && sell_fraction_from_snapshot(&snapshot) >= 1.0 - 1e-9;
                if !full_lot_exit {
                    db.with_conn(|conn| {
                        intents::mark_terminal(
                            conn,
                            &intent.id,
                            "rejected",
                            None,
                            Some("notional cap"),
                        )
                    })?;
                    return Err(anyhow::anyhow!("Live notional exceeds configured cap"));
                }
                tracing::info!(
                    target: "execution",
                    symbol = %symbol,
                    quantity,
                    notional = fee.total_price.max(quantity * broker_quote),
                    cap = settings.max_live_notional,
                    "full-lot SELL exceeds max_live_notional; submitting anyway"
                );
            }

            db.with_conn(|conn| intents::mark_submitted(conn, &intent.id, None))?;
            tracing::info!(
                target: "execution",
                signal_id,
                symbol = %symbol,
                action = %action,
                quantity,
                venue = client.id().as_str(),
                "submitting live broker order"
            );
            let order = match client
                .place_and_await_fill(
                    &instrument,
                    &action,
                    &fee,
                    Some(&intent.client_order_id),
                )
                .await
            {
                Ok(o) => o,
                Err(e) => {
                    // HTTP/client failure: the broker never accepted an order id.
                    // Mark rejected so this does not freeze all live trading via
                    // ambiguous_pending / pending_sell_qty.
                    db.with_conn(|conn| {
                        intents::mark_terminal(
                            conn,
                            &intent.id,
                            "rejected",
                            None,
                            Some(&e.to_string()),
                        )
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
                    Some(fee.fee),
                    client.id().as_str(),
                )?;
                let state = if order.status == "executed" {
                    "filled"
                } else if order.status == "rejected" {
                    "rejected"
                } else {
                    "unknown"
                };
                intents::mark_terminal(
                    conn,
                    &intent.id,
                    state,
                    Some(order.id.as_str()),
                    order.rejection_reason.as_deref(),
                )?;
                if order.status == "executed" {
                    conn.execute("UPDATE signals SET executed = 1 WHERE id = ?1", [signal_id])?;
                    let avg_cost = if action == "SELL" {
                        snap.holdings
                            .iter()
                            .find(|h| h.symbol.eq_ignore_ascii_case(&symbol))
                            .and_then(|h| h.buy_price)
                    } else {
                        None
                    };
                    let px = fill_price.unwrap_or(broker_quote);
                    let _ = crate::outcomes::record_executed_fill(
                        conn,
                        signal_id,
                        &symbol,
                        &action,
                        quantity,
                        px,
                        fee.fee,
                        client.id().as_str(),
                        avg_cost,
                    );
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
                    "{} order still {} after polling (id {})",
                    client.display_name(),
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
            let daily_drawdown: f64 = conn
                .query_row(
                    "SELECT drawdown_pct FROM daily_performance_snapshot WHERE portfolio_id = ?1 ORDER BY snapshot_date DESC LIMIT 1",
                    [&portfolio.0],
                    |row| row.get(0),
                )
                .unwrap_or(0.0);
            let (result, mut quantity) = RiskPolicyService::evaluate(
                &input,
                &param_set,
                portfolio.1,
                &positions,
                &prices,
                daily_drawdown,
                fee_pct,
                buy_target_notional,
                0.0,
                true,
            );
            conn.execute(
                "UPDATE signals SET risk_policy_result = ?1 WHERE id = ?2",
                rusqlite::params![result, signal_id],
            )?;
            if result != "APPROVED" || quantity <= 0.0 {
                return Ok(false);
            }
            if action == "SELL" {
                quantity = crate::risk_exits::sell_qty_for_exit_ex(
                    quantity,
                    sell_fraction_from_snapshot(&snapshot),
                    true,
                );
                if quantity < 1.0 {
                    return Ok(false);
                }
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

        let avg_cost_for_close: Option<f64> = if side == "SELL" {
            conn.query_row(
                "SELECT avg_cost FROM sandbox_positions WHERE portfolio_id = ?1 AND symbol = ?2",
                rusqlite::params![portfolio_id, symbol],
                |row| row.get(0),
            )
            .ok()
        } else {
            None
        };

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
        crate::outcomes::record_executed_fill(
            conn,
            signal_id,
            symbol,
            side,
            quantity,
            fill_price,
            fee,
            "sandbox",
            avg_cost_for_close,
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

    fn plan_sandbox_buy_targets(
        conn: &Connection,
        cache: &PriceCache,
        signal_ids: &[String],
    ) -> Result<std::collections::HashMap<String, f64>> {
        let portfolio: (String, f64, String) = conn.query_row(
            "SELECT id, cash_balance, strategy_param_set_id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let param_set = Self::load_param_set(conn, &portfolio.2)?;
        let daily_drawdown: f64 = conn
            .query_row(
                "SELECT drawdown_pct FROM daily_performance_snapshot WHERE portfolio_id = ?1 ORDER BY snapshot_date DESC LIMIT 1",
                [&portfolio.0],
                |row| row.get(0),
            )
            .unwrap_or(0.0);
        let remaining = remaining_buy_budget(
            portfolio.1,
            param_set.cycle_budget_pct,
            0.0,
        );
        Self::plan_buy_targets(
            conn,
            cache,
            signal_ids,
            &param_set,
            portfolio.1,
            daily_drawdown,
            0.0,
            remaining,
            "sandbox",
        )
    }

    /// Pass A qualify + Pass B confidence-weighted cycle budget → target notional per BUY signal id.
    fn plan_buy_targets(
        conn: &Connection,
        cache: &PriceCache,
        signal_ids: &[String],
        param_set: &ParamSet,
        cash: f64,
        daily_drawdown: f64,
        min_notional: f64,
        remaining_budget: f64,
        venue: &str,
    ) -> Result<std::collections::HashMap<String, f64>> {
        let mut qualified: Vec<(String, f64, String)> = Vec::new();

        for signal_id in signal_ids {
            let signal: Option<(String, String, f64)> = conn
                .query_row(
                    "SELECT symbol, action, confidence FROM signals WHERE id = ?1",
                    [signal_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok();
            let Some((symbol, action, confidence)) = signal else {
                continue;
            };
            if action != "BUY" {
                continue;
            }
            if !is_valid_trade_symbol(venue, &symbol) {
                conn.execute(
                    "UPDATE signals SET risk_policy_result = 'BLOCKED_SYMBOL' WHERE id = ?1",
                    [signal_id],
                )?;
                continue;
            }
            if confidence < param_set.min_confidence_to_trade {
                conn.execute(
                    "UPDATE signals SET risk_policy_result = 'BLOCKED_CONFIDENCE' WHERE id = ?1",
                    [signal_id],
                )?;
                continue;
            }
            if daily_drawdown >= param_set.max_daily_drawdown_pct {
                conn.execute(
                    "UPDATE signals SET risk_policy_result = 'BLOCKED_DRAWDOWN' WHERE id = ?1",
                    [signal_id],
                )?;
                continue;
            }
            let price = Self::get_prices(conn, cache, &[symbol.clone()])?
                .get(&symbol)
                .copied()
                .unwrap_or(0.0);
            if price <= 0.0 {
                conn.execute(
                    "UPDATE signals SET risk_policy_result = 'BLOCKED_OTHER' WHERE id = ?1",
                    [signal_id],
                )?;
                continue;
            }
            qualified.push((signal_id.clone(), confidence, symbol));
        }

        qualified.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let budget = remaining_budget.min(cash).max(0.0);
        if budget <= 0.0 {
            return Ok(std::collections::HashMap::new());
        }
        let mut kept: Vec<(String, f64, f64)> = Vec::new();
        let mut reserved = 0.0;
        for (id, conf, symbol) in qualified {
            let pair_min = if venue.eq_ignore_ascii_case("busha") {
                crate::busha::min_buy_ngn_for(conn, &symbol).unwrap_or(min_notional)
            } else {
                min_notional
            };
            if pair_min > 0.0 && budget + 1e-9 < pair_min {
                conn.execute(
                    "UPDATE signals SET risk_policy_result = 'BLOCKED_NOTIONAL' WHERE id = ?1",
                    [&id],
                )?;
                continue;
            }
            if pair_min > 0.0 && budget - reserved + 1e-9 < pair_min {
                conn.execute(
                    "UPDATE signals SET risk_policy_result = 'BLOCKED_NOTIONAL' WHERE id = ?1",
                    [&id],
                )?;
                continue;
            }
            kept.push((id, conf, pair_min));
            reserved += pair_min.max(0.0);
        }
        let max_n = kept.len().min(MAX_SIGNAL_BUYS);
        if max_n == 0 {
            return Ok(std::collections::HashMap::new());
        }
        kept.truncate(max_n);
        let weights = RiskPolicyService::confidence_weights(
            &kept.iter().map(|(_, c, _)| *c).collect::<Vec<_>>(),
        );
        let mut out = std::collections::HashMap::new();
        let mut rows: Vec<(String, f64, f64)> = kept
            .into_iter()
            .zip(weights)
            .map(|((id, _, pair_min), w)| {
                let floor = pair_min.max(0.0);
                let sized = if floor <= 0.0 {
                    budget * w
                } else {
                    (budget * w).max(floor)
                };
                (id, sized, floor)
            })
            .collect();
        loop {
            let sum: f64 = rows.iter().map(|(_, n, _)| *n).sum();
            if sum <= cash + 1e-9 || rows.is_empty() {
                break;
            }
            if rows.len() == 1 {
                let floor = rows[0].2;
                if floor > 0.0 && cash + 1e-9 < floor {
                    rows.clear();
                } else {
                    rows[0].1 = cash.min(budget.max(floor));
                    if floor > 0.0 && rows[0].1 + 1e-9 < floor {
                        rows.clear();
                    }
                }
                break;
            }
            rows.pop();
            let n = rows.len() as f64;
            for row in rows.iter_mut() {
                row.1 = (budget / n).max(row.2);
            }
        }
        for (id, n, _) in rows {
            out.insert(id, n);
        }
        Ok(out)
    }

    fn load_param_set(conn: &Connection, id: &str) -> Result<ParamSet> {
        conn.query_row(
            "SELECT id, max_position_pct, max_daily_trades, stop_loss_pct, min_confidence_to_trade,
                    max_daily_drawdown_pct, position_size_pct, cycle_budget_pct
             FROM strategy_param_sets WHERE id = ?1",
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
                    cycle_budget_pct: row.get(7)?,
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
        let venue = session.id().as_str().to_string();
        let _ = db.with_conn(|conn| {
            let _ = crate::intents::reject_unsubmitted_unknown(conn);
            let _ = crate::intents::reject_abandoned_created(conn, Some(&venue));
            crate::intents::reject_stale_submitted_without_ref(conn, Some(&venue))
        });
        let open = db.with_conn(|conn| crate::intents::load_open_intents_on(conn, Some(&venue)))?;
        for intent in open {
            if let Some(eid) = intent.external_order_ref.clone() {
                match session.get_order(&eid).await {
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
                                Some(order.id.as_str()),
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
                                Some(eid.as_str()),
                                Some(&e.to_string()),
                            )
                        })?;
                    }
                }
            }
        }
        let _ = db.with_conn(|conn| crate::intents::reject_aged_unknown(conn, Some(&venue)));
        Ok(())
    }
}

pub const RETRY_SLOT_FRACTION: f64 = 0.30;

fn sell_fraction_from_snapshot(snapshot: &str) -> f64 {
    serde_json::from_str::<serde_json::Value>(snapshot)
        .ok()
        .and_then(|v| v.get("sellFraction").and_then(|x| x.as_f64()))
        .filter(|f| f.is_finite() && *f > 0.0)
        .unwrap_or(1.0)
}

/// Cap how many of `max_live_actions` may be 24h retries (≤30%, at least 1).
pub fn retry_slot_cap(max_live_actions: usize) -> usize {
    let n = max_live_actions.max(1);
    let cap = ((n as f64) * RETRY_SLOT_FRACTION).floor() as usize;
    cap.max(1).min(n)
}

/// Unexecuted BUY/SELL signals from recent cycles that should be retried on the next live run.
/// Affordability (`BLOCKED_CASH`) BUYs are never retried — they re-hit the broker while broke.
/// Pass `allow_buys = false` when the cycle is capital-constrained so queued BUYs stay parked.
/// `limit` should already be `retry_slot_cap(max_live_actions)`.
pub fn retryable_unexecuted_signal_ids(
    conn: &Connection,
    hours: i64,
    limit: usize,
    allow_buys: bool,
) -> Result<Vec<String>> {
    let cutoff = format!("-{hours} hours");
    let allow_buys_i: i64 = if allow_buys { 1 } else { 0 };
    let mut stmt = conn.prepare(
        "SELECT id FROM signals
         WHERE executed = 0
           AND UPPER(action) IN ('BUY', 'SELL')
           AND generated_at >= datetime('now', ?1)
           AND COALESCE(risk_policy_result, '') IN (
             'BLOCKED_BROKER', 'BLOCKED_CASH', 'BLOCKED_SYMBOL', 'BLOCKED_NOT_EXECUTED',
             'BLOCKED_OTHER', 'BLOCKED_AMBIGUOUS_ORDERS', 'BLOCKED_MARKET_CLOSED',
             'BLOCKED_PENDING_CONFIRM', 'BLOCKED_LIVE_DISABLED', 'BLOCKED_QUOTE_DEVIATION'
           )
           AND NOT (
             UPPER(action) = 'BUY'
             AND COALESCE(risk_policy_result, '') = 'BLOCKED_CASH'
           )
           AND (?3 = 1 OR UPPER(action) = 'SELL')
         ORDER BY generated_at ASC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![cutoff, limit as i64, allow_buys_i], |row| {
        row.get::<_, String>(0)
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn quote_within_deviation(reference: f64, broker: f64) -> bool {
    if reference <= 0.0 || broker <= 0.0 || !reference.is_finite() || !broker.is_finite() {
        return false;
    }
    ((broker - reference) / reference).abs() <= MAX_QUOTE_DEVIATION
}

/// Size a live BUY against cash and broker availability in O(1).
/// `available_quantity < 1` is treated as unknown (do not walk qty down to 0).
pub(crate) fn fit_live_buy_qty(
    requested: f64,
    spendable: f64,
    total_price: f64,
    price_per_share: f64,
    available_quantity: f64,
    whole_shares: bool,
) -> Result<f64, String> {
    let lot = if whole_shares { 1.0 } else { 1e-8 };
    let mut qty = if whole_shares {
        requested.floor()
    } else {
        requested
    };
    if qty + 1e-12 < lot {
        return Err("Cannot size a whole share".into());
    }
    if whole_shares && available_quantity >= 1.0 {
        qty = qty.min(available_quantity.floor());
    } else if !whole_shares && available_quantity > 0.0 && available_quantity.is_finite() {
        qty = qty.min(available_quantity);
    }
    if spendable + 1e-9 < total_price {
        let per_share = if requested >= lot && total_price > 0.0 {
            total_price / requested
        } else if price_per_share > 0.0 {
            price_per_share
        } else {
            0.0
        };
        if per_share <= 0.0 {
            return Err(format!(
                "Insufficient brokerage balance (have ₦{spendable:.2}, need ₦{total_price:.2})"
            ));
        }
        let fitted = spendable / per_share;
        qty = if whole_shares {
            fitted.floor().min(qty)
        } else {
            fitted.min(qty)
        };
    }
    if qty + 1e-12 < lot {
        if available_quantity > 0.0 && available_quantity < lot {
            return Err(format!("Bamboo available_quantity is {available_quantity}"));
        }
        return Err(format!(
            "Insufficient brokerage balance for 1 share (have ₦{spendable:.2}, need ₦{total_price:.2})"
        ));
    }
    Ok(qty)
}

/// Quote-deviation is a BUY stale-price guard. SELLs (stop-loss / take-profit)
/// must still reach the broker when the market is open.
pub fn quote_deviation_applies(action: &str) -> bool {
    action.eq_ignore_ascii_case("BUY")
}

/// Map a live execution failure to a specific BLOCKED_* so the persist-time
/// default (`BLOCKED_OTHER`) is never left after an attempt.
pub fn classify_live_execution_error(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("deviates") {
        "BLOCKED_QUOTE_DEVIATION"
    } else if m.contains("drawdown") {
        "BLOCKED_DRAWDOWN"
    } else if m.contains("notional") || m.contains("bulk liquidation") || m.contains("below min") {
        "BLOCKED_NOTIONAL"
    } else if m.contains("transaction pin") {
        "BLOCKED_PIN_MISSING"
    } else if m.contains("minimum order") || m.contains("min order") {
        "BLOCKED_CASH"
    } else if m.contains("insufficient") || m.contains("brokerage balance") {
        "BLOCKED_CASH"
    } else if m.contains("no position") {
        "BLOCKED_NO_POSITION"
    } else if m.contains("market") && m.contains("closed") {
        "BLOCKED_MARKET_CLOSED"
    } else if m.contains("quote missing") {
        "BLOCKED_SYMBOL"
    } else if m.contains("whole share") {
        "BLOCKED_CASH"
    } else if m.contains("matching live order") {
        "BLOCKED_NOT_EXECUTED"
    } else {
        "BLOCKED_BROKER"
    }
}

pub fn live_drawdown_pct(conn: &Connection, venue: &str, current_equity: f64) -> Result<f64> {
    // Session-open = first equity point today. Also consider prior calendar-day
    // close so overnight gaps still trip the BUY brake. Missing history is
    // fail-open (0) so day-one is not fail-closed with no points at all.
    if current_equity <= 0.0 || !current_equity.is_finite() {
        return Ok(0.0);
    }
    let session_open: Option<f64> = conn
        .query_row(
            "SELECT total_equity FROM equity_curve_points
             WHERE venue = ?1 AND date(recorded_at) = date('now')
             ORDER BY recorded_at ASC LIMIT 1",
            [venue],
            |row| row.get(0),
        )
        .optional()?;
    let prior: Option<f64> = conn
        .query_row(
            "SELECT total_equity FROM equity_curve_points
             WHERE venue = ?1 AND date(recorded_at) < date('now')
             ORDER BY recorded_at DESC LIMIT 1",
            [venue],
            |row| row.get(0),
        )
        .optional()?;
    let vs = |base: Option<f64>| -> f64 {
        match base.filter(|p| *p > 0.0 && p.is_finite()) {
            Some(b) => ((b - current_equity) / b).max(0.0),
            None => 0.0,
        }
    };
    Ok(vs(session_open).max(vs(prior)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_fund_min_lot_requires_whole_share() {
        let param_set = test_param_set();
        assert!(RiskPolicyService::can_fund_min_lot(
            10_000.0, 10_000.0, 50.0, 0.0, &param_set, 0.0015
        ));
        assert!(!RiskPolicyService::can_afford_one_share(40.0, 50.0, 0.0015));
        assert!(!RiskPolicyService::can_fund_min_lot(
            40.0, 10_000.0, 50.0, 0.0, &param_set, 0.0015
        ));
        assert!(!RiskPolicyService::can_fund_min_lot(
            0.0, 10_000.0, 50.0, 0.0, &param_set, 0.0015
        ));
        // Cycle budget (20%) must cover one share after fees on a pricey name
        assert!(!RiskPolicyService::can_fund_min_lot(
            2_100_000.0, 10_000_000.0, 2_000_000.0, 0.0, &param_set, 0.0015
        ));
        assert!(RiskPolicyService::can_fund_min_lot(
            10_100_000.0, 10_100_000.0, 2_000_000.0, 0.0, &param_set, 0.0015
        ));
    }

    #[test]
    fn rejects_stale_broker_quote() {
        assert!(quote_within_deviation(100.0, 104.0));
        assert!(!quote_within_deviation(100.0, 106.0));
        assert!(!quote_within_deviation(0.0, 10.0));
    }

    #[test]
    fn fit_buy_does_not_walk_qty_when_available_is_zero() {
        let q = super::fit_live_buy_qty(252.0, 16_750.0, 405.0, 1.61, 0.0, true).unwrap();
        assert_eq!(q, 252.0);
    }

    #[test]
    fn fit_buy_caps_to_available_in_one_step() {
        let q = super::fit_live_buy_qty(252.0, 16_750.0, 405.0, 1.61, 3.7, true).unwrap();
        assert_eq!(q, 3.0);
    }

    #[test]
    fn fit_buy_scales_to_cash_in_one_step() {
        let q = super::fit_live_buy_qty(325.0, 200.0, 445.0, 1.37, f64::MAX, true).unwrap();
        assert_eq!(q, (200.0f64 / (445.0 / 325.0)).floor());
        assert!(q >= 1.0);
        assert!(q < 325.0);
    }

    #[test]
    fn live_buy_budget_raises_to_bamboo_minimum() {
        let budget = super::live_buy_budget(16_750.0, 0.20, super::BAMBOO_MIN_ORDER_NOTIONAL);
        assert!((budget - super::BAMBOO_MIN_ORDER_NOTIONAL).abs() < 1e-9);
        assert_eq!(super::max_buys_for_min_notional(budget, super::BAMBOO_MIN_ORDER_NOTIONAL, 15), 1);
        assert_eq!(super::live_buy_budget(4_000.0, 0.20, super::BAMBOO_MIN_ORDER_NOTIONAL), 0.0);
        assert_eq!(super::live_buy_budget(50_000.0, 0.20, super::BAMBOO_MIN_ORDER_NOTIONAL), 10_000.0);
        assert_eq!(super::max_buys_for_min_notional(10_000.0, super::BAMBOO_MIN_ORDER_NOTIONAL, 15), 2);
    }

    #[test]
    fn busha_venue_uses_ngn_conversion_floor() {
        assert_eq!(super::min_order_notional_for_venue("busha"), super::BUSHA_MIN_ORDER_NOTIONAL);
        let budget = super::live_buy_budget(4_748.07, 0.20, super::BUSHA_MIN_ORDER_NOTIONAL);
        assert!(budget + 1e-9 >= super::BUSHA_MIN_ORDER_NOTIONAL);
        assert_eq!(
            super::max_buys_for_min_notional(budget, super::BUSHA_MIN_ORDER_NOTIONAL, 15),
            3
        );
    }

    #[test]
    fn buy_ineligible_when_cash_is_zero() {
        let remaining = super::remaining_buy_budget(0.0, 0.20, super::BAMBOO_MIN_ORDER_NOTIONAL);
        let reason = super::buy_ineligible_reason(0.0, remaining, super::BAMBOO_MIN_ORDER_NOTIONAL);
        assert_eq!(reason, Some("insufficient spendable cash"));
    }

    #[test]
    fn buy_ineligible_when_cash_below_bamboo_minimum() {
        let remaining = super::remaining_buy_budget(4_000.0, 0.20, super::BAMBOO_MIN_ORDER_NOTIONAL);
        let reason = super::buy_ineligible_reason(4_000.0, remaining, super::BAMBOO_MIN_ORDER_NOTIONAL);
        assert!(reason.unwrap_or("").contains("minimum"));
        assert_eq!(reason, Some("cash below Bamboo ₦5000 minimum"));
    }

    #[test]
    fn leftover_cash_after_fills_can_still_fund_min_lot() {
        // Wallet is already net of today's fills; do not treat prior BUY notional as a second cap.
        let cash = 11_043.0;
        let min_n = super::BAMBOO_MIN_ORDER_NOTIONAL;
        let remaining_full = super::remaining_buy_budget(cash, 1.0, min_n);
        assert!(super::buy_ineligible_reason(cash, remaining_full, min_n).is_none());
        assert_eq!(super::max_buys_for_min_notional(remaining_full, min_n, 15), 2);

        let remaining_half = super::remaining_buy_budget(cash, 0.5, min_n);
        assert!(super::buy_ineligible_reason(cash, remaining_half, min_n).is_none());
        assert_eq!(super::max_buys_for_min_notional(remaining_half, min_n, 15), 1);
    }

    #[test]
    fn buy_eligible_on_recovery_with_one_min_lot() {
        let cash = 16_750.0;
        let remaining = super::remaining_buy_budget(cash, 0.20, super::BAMBOO_MIN_ORDER_NOTIONAL);
        assert!(super::buy_ineligible_reason(cash, remaining, super::BAMBOO_MIN_ORDER_NOTIONAL).is_none());
        assert_eq!(
            super::max_buys_for_min_notional(remaining, super::BAMBOO_MIN_ORDER_NOTIONAL, 15),
            1
        );
    }

    #[test]
    fn todays_buy_notional_counts_filled_and_pending_not_rejected() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE order_intents (
                id TEXT PRIMARY KEY,
                side TEXT,
                requested_notional REAL,
                venue TEXT,
                state TEXT,
                created_at TEXT DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO order_intents (id, side, requested_notional, venue, state)
             VALUES ('f', 'BUY', 5000, 'bamboo', 'filled')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO order_intents (id, side, requested_notional, venue, state)
             VALUES ('s', 'BUY', 2500, 'bamboo', 'submitted')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO order_intents (id, side, requested_notional, venue, state)
             VALUES ('r', 'BUY', 9000, 'bamboo', 'rejected')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO order_intents (id, side, requested_notional, venue, state)
             VALUES ('c', 'BUY', 111, 'bamboo', 'created')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO order_intents (id, side, requested_notional, venue, state)
             VALUES ('w', 'BUY', 8000, 'wealth', 'filled')",
            [],
        )
        .unwrap();
        let n = super::todays_buy_notional(&conn, "bamboo").unwrap();
        assert!((n - 7500.0).abs() < 1e-9);
    }

    #[test]
    fn bamboo_min_lot_allowed_when_it_breaches_position_cap() {
        let (result, qty) = RiskPolicyService::size_buy_from_notional(
            5_000.0,
            1.61,
            16_750.0,
            16_750.0,
            0.0,
            0.10,
            0.0,
            super::BAMBOO_MIN_ORDER_NOTIONAL,
            true,
        );
        assert_eq!(result, "APPROVED");
        assert!(qty * 1.61 + 1e-9 >= super::BAMBOO_MIN_ORDER_NOTIONAL);
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
            cycle_budget_pct: 0.20,
        }
    }

    #[test]
    fn confidence_weights_split_flatter_than_raw_ratio() {
        let weights = RiskPolicyService::confidence_weights(&[0.9, 0.45]);
        assert_eq!(weights.len(), 2);
        let ratio = weights[0] / weights[1];
        // √0.9 / √0.45 = √2 ≈ 1.414, not raw 2:1
        assert!((ratio - std::f64::consts::SQRT_2).abs() < 1e-9);
        assert!(ratio < 2.0);
    }

    #[test]
    fn confidence_weights_equal_when_zero_sum() {
        let weights = RiskPolicyService::confidence_weights(&[0.0, 0.0]);
        assert_eq!(weights, vec![0.5, 0.5]);
    }

    #[test]
    fn cycle_budget_allocates_by_confidence_before_share_rounding() {
        let param_set = test_param_set();
        let cash = 1_000_000.0;
        let budget = cash * param_set.cycle_budget_pct;
        let weights = RiskPolicyService::confidence_weights(&[0.9, 0.45]);
        let n0 = budget * weights[0];
        let n1 = budget * weights[1];
        assert!((n0 / n1 - std::f64::consts::SQRT_2).abs() < 1e-9);
        assert!((n0 + n1 - budget).abs() < 1e-6);

        let price = 50.0;
        let (r0, q0) = RiskPolicyService::size_buy_from_notional(
            n0, price, cash, cash, 0.0, param_set.max_position_pct, 0.0, 0.0, true,
        );
        let (r1, q1) = RiskPolicyService::size_buy_from_notional(
            n1, price, cash, cash, 0.0, param_set.max_position_pct, 0.0, 0.0, true,
        );
        assert_eq!(r0, "APPROVED");
        assert_eq!(r1, "APPROVED");
        assert!(q0 > q1);
        assert!((q0 as f64 / q1 as f64 - std::f64::consts::SQRT_2).abs() < 0.05);
    }

    #[test]
    fn clamp_cycle_budget_allows_full_cash() {
        assert_eq!(RiskPolicyService::clamp_cycle_budget_pct(1.0), 1.0);
        assert_eq!(RiskPolicyService::clamp_cycle_budget_pct(1.2), 1.0);
        assert_eq!(RiskPolicyService::clamp_cycle_budget_pct(0.01), 0.05);
    }

    #[test]
    fn cycle_budget_pct_caps_total_notional() {
        let param_set = test_param_set();
        let cash = 100_000.0;
        let budget = cash * RiskPolicyService::clamp_cycle_budget_pct(param_set.cycle_budget_pct);
        assert!((budget - 20_000.0).abs() < 1e-9);
        let (result, qty) = RiskPolicyService::size_buy_from_notional(
            budget, 50.0, cash, cash, 0.0, param_set.max_position_pct, 0.0, 0.0, true,
        );
        assert_eq!(result, "APPROVED");
        assert_eq!(qty, 400.0); // 20000/50
        assert!(qty * 50.0 <= budget + 1e-9);
    }

    #[test]
    fn max_position_still_blocks_oversized_buy() {
        let param_set = test_param_set();
        // Already at max exposure
        let equity = 100_000.0;
        let position_value = 25_000.0;
        let (result, qty) = RiskPolicyService::size_buy_from_notional(
            20_000.0,
            50.0,
            75_000.0,
            equity,
            position_value,
            param_set.max_position_pct,
            0.0,
            0.0,
            true,
        );
        assert_eq!(result, "BLOCKED_EXPOSURE");
        assert_eq!(qty, 0.0);
    }

    #[test]
    fn whole_share_floor_blocks_tiny_notional() {
        let (result, qty) = RiskPolicyService::size_buy_from_notional(
            40.0, 50.0, 1_000_000.0, 1_000_000.0, 0.0, 0.25, 0.0, 0.0, true,
        );
        assert_eq!(result, "BLOCKED_CASH");
        assert_eq!(qty, 0.0);
    }

    #[test]
    fn fractional_size_allows_sub_share_crypto() {
        let (result, qty) = RiskPolicyService::size_buy_from_notional(
            650.0, 124.78, 125_000.0, 125_000.0, 0.0, 0.25, 0.0, 0.0, false,
        );
        assert_eq!(result, "APPROVED");
        assert!(qty > 5.0 && qty < 6.0);
    }

    #[test]
    fn drawdown_blocks_buy_not_sell() {
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
            param_set.max_daily_drawdown_pct,
            0.01,
            Some(50_000.0),
            0.0,
            true,
        );
        let (sell_result, sell_qty) = RiskPolicyService::evaluate(
            &sell,
            &param_set,
            1_000_000.0,
            &positions,
            &prices,
            param_set.max_daily_drawdown_pct,
            0.01,
            None,
            0.0,
            true,
        );
        assert_eq!(buy_result, "BLOCKED_DRAWDOWN");
        assert_eq!(sell_result, "APPROVED");
        assert_eq!(sell_qty, 100.0);
    }

    #[test]
    fn sell_approves_with_case_insensitive_position_match() {
        let param_set = test_param_set();
        let prices = std::collections::HashMap::from([("GTCO".into(), 50.0)]);
        let positions = vec![("gtco".into(), 100.0, 40.0)];
        let sell = SignalInput {
            id: "s".into(),
            symbol: "GTCO".into(),
            action: "SELL".into(),
            confidence: 0.8,
        };
        let (result, qty) = RiskPolicyService::evaluate(
            &sell,
            &param_set,
            1_000_000.0,
            &positions,
            &prices,
            0.0,
            0.01,
            None,
            0.0,
            true,
        );
        assert_eq!(result, "APPROVED");
        assert_eq!(qty, 100.0);
    }

    #[test]
    fn sell_without_position_is_blocked_no_position() {
        let param_set = test_param_set();
        let prices = std::collections::HashMap::from([("GTCO".into(), 50.0)]);
        let sell = SignalInput {
            id: "s".into(),
            symbol: "GTCO".into(),
            action: "SELL".into(),
            confidence: 0.8,
        };
        let (result, qty) = RiskPolicyService::evaluate(
            &sell,
            &param_set,
            1_000_000.0,
            &[],
            &prices,
            0.0,
            0.01,
            None,
            0.0,
            true,
        );
        assert_eq!(result, "BLOCKED_NO_POSITION");
        assert_eq!(qty, 0.0);
    }

    #[test]
    fn sell_approves_live_shaped_holdings() {
        let param_set = test_param_set();
        // Mirrors Wealth/Bamboo book rows: mixed case, buy_price as cost.
        let positions = vec![
            ("NIDF".into(), 6.0, 181.25),
            ("vfdgroup".into(), 102.0, 12.85),
            ("HONYFLOUR".into(), 173.0, 17.57),
        ];
        let prices = std::collections::HashMap::from([
            ("NIDF".into(), 147.7),
            ("VFDGROUP".into(), 11.8),
            ("HONYFLOUR".into(), 17.1),
        ]);
        let sell = SignalInput {
            id: "s".into(),
            symbol: "VFDGROUP".into(),
            action: "SELL".into(),
            confidence: 0.95,
        };
        let (result, qty) = RiskPolicyService::evaluate(
            &sell,
            &param_set,
            1_501.72,
            &positions,
            &prices,
            0.0,
            0.0015,
            None,
            0.0,
            true,
        );
        assert_eq!(result, "APPROVED");
        assert_eq!(qty, 102.0);
    }

    #[test]
    fn quote_deviation_is_buy_only() {
        assert!(quote_deviation_applies("BUY"));
        assert!(quote_deviation_applies("buy"));
        assert!(!quote_deviation_applies("SELL"));
        assert!(!quote_deviation_applies("HOLD"));
        // A 20% Pulse/broker gap would block a BUY but must not gate a SELL.
        assert!(!quote_within_deviation(100.0, 80.0));
    }

    #[test]
    fn classify_live_errors_never_leave_blocked_other() {
        assert_eq!(
            classify_live_execution_error("Broker quote for GTCO deviates more than 5% from Pulse"),
            "BLOCKED_QUOTE_DEVIATION"
        );
        assert_eq!(
            classify_live_execution_error("Live notional exceeds configured cap"),
            "BLOCKED_NOTIONAL"
        );
        assert_eq!(
            classify_live_execution_error("Bulk liquidation requires extra confirmation"),
            "BLOCKED_NOTIONAL"
        );
        assert_eq!(
            classify_live_execution_error("Insufficient brokerage balance for GTCO (have ₦12.00)"),
            "BLOCKED_CASH"
        );
        assert_eq!(
            classify_live_execution_error("Bamboo minimum order is ₦5000 (need ₦5000.00, have ₦16750.00)"),
            "BLOCKED_CASH"
        );
        assert_eq!(
            classify_live_execution_error("Bamboo transaction PIN missing — add it in Settings"),
            "BLOCKED_PIN_MISSING"
        );
        assert_eq!(
            classify_live_execution_error("Wealth HTTP 401 unauthorized"),
            "BLOCKED_BROKER"
        );
        assert_eq!(
            classify_live_execution_error("Matching live order already submitted for NIDF"),
            "BLOCKED_NOT_EXECUTED"
        );
        assert_eq!(
            classify_live_execution_error("Bamboo quote missing for FLOURMILL"),
            "BLOCKED_SYMBOL"
        );
        assert_eq!(
            classify_live_execution_error("Only whole share trading is possible"),
            "BLOCKED_CASH"
        );
        assert_ne!(classify_live_execution_error("timeout"), "BLOCKED_OTHER");
    }

    #[test]
    fn live_drawdown_is_zero_without_prior_day() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE equity_curve_points (
                venue TEXT, recorded_at TEXT, total_equity REAL
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO equity_curve_points (venue, recorded_at, total_equity)
             VALUES ('wealth', datetime('now'), 9572.62)",
            [],
        )
        .unwrap();
        let pct = live_drawdown_pct(&conn, "wealth", 9572.62).unwrap();
        assert_eq!(pct, 0.0);

        conn.execute(
            "INSERT INTO equity_curve_points (venue, recorded_at, total_equity)
             VALUES ('wealth', datetime('now', '-1 day'), 10_000.0)",
            [],
        )
        .unwrap();
        let pct = live_drawdown_pct(&conn, "wealth", 9_000.0).unwrap();
        assert!((pct - 0.10).abs() < 1e-9);
    }

    #[test]
    fn intra_session_drawdown_without_prior_day() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE equity_curve_points (
                venue TEXT, recorded_at TEXT, total_equity REAL
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO equity_curve_points (venue, recorded_at, total_equity)
             VALUES ('bamboo', datetime('now', '-2 hours'), 10_000.0)",
            [],
        )
        .unwrap();
        let pct = live_drawdown_pct(&conn, "bamboo", 8_500.0).unwrap();
        assert!((pct - 0.15).abs() < 1e-9);
        let buy = RiskPolicyService::evaluate(
            &SignalInput {
                id: "s".into(),
                symbol: "GTCO".into(),
                action: "BUY".into(),
                confidence: 0.9,
            },
            &test_param_set(),
            8_500.0,
            &[],
            &{
                let mut m = std::collections::HashMap::new();
                m.insert("GTCO".into(), 50.0);
                m
            },
            pct,
            0.0,
            Some(5_000.0),
            5_000.0,
            true,
        );
        assert_eq!(buy.0, "BLOCKED_DRAWDOWN");
    }

    #[test]
    fn retry_slot_cap_is_thirty_percent() {
        assert_eq!(retry_slot_cap(10), 3);
        assert_eq!(retry_slot_cap(1), 1);
        assert_eq!(retry_slot_cap(40), 12);
    }

    #[test]
    fn skip_statuses_match_live_gates() {
        let mut on = AppSettings::default();
        on.live_trading_enabled = true;
        assert!(on.should_execute(TradingMode::Live));
        assert!(on.should_execute_risk_exits(TradingMode::Live, true));
        assert!(!on.should_execute_risk_exits(TradingMode::Live, false));
        assert_eq!(on.cycle_skip_result(TradingMode::Live), None);
        // Live path reaches place_and_await_fill only when this gate is open.
        assert!(on.execution_skip(TradingMode::Live, true).is_none());

        let off = AppSettings::default();
        assert!(!off.should_execute(TradingMode::Live));
        assert_eq!(
            off.cycle_skip_result(TradingMode::Live),
            Some("BLOCKED_LIVE_DISABLED")
        );
        assert!(off.should_execute(TradingMode::Sandbox));
        assert_eq!(off.cycle_skip_result(TradingMode::Sandbox), None);
        assert!(!on.should_execute(TradingMode::Sandbox));
        assert_eq!(on.cycle_skip_result(TradingMode::Sandbox), Some("BLOCKED_BROKER"));
    }

    #[test]
    fn process_signals_execute_false_overwrites_blocked_other() {
        use crate::cache::PriceCache;
        use crate::db::Database;
        use crate::runtime_util::block_on_local;
        use crate::seed::SeedService;

        let dir = std::env::temp_dir().join(format!("pulsar-exec-skip-{}", uuid::Uuid::new_v4()));
        let db = Database::open(&dir).expect("test db");
        db.with_conn(|conn| SeedService::seed_if_empty(conn, 10_000_000.0))
            .expect("seed");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
                 VALUES ('sig-1', 'GTCO', 'SELL', 1.0, 'stop', '{}', 'rules:stop-loss', 'v2.4.0', 'BLOCKED_OTHER', 0)",
                [],
            )?;
            Ok(())
        })
        .expect("insert");

        let cache = PriceCache::new(60);
        let mut settings = AppSettings::default();
        settings.live_trading_enabled = false;
        let ids = vec!["sig-1".to_string()];
        let skip = settings.cycle_skip_result(TradingMode::Live);
        assert_eq!(skip, Some("BLOCKED_LIVE_DISABLED"));

        let (executed, _) = block_on_local(ExecutionService::process_signals(
            &db,
            &cache,
            &settings,
            &ids,
            None,
            TradingMode::Live,
            true,
            false,
            false,
            None,
            skip,
        ))
        .expect("process");
        assert_eq!(executed, 0);
        let result: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT risk_policy_result FROM signals WHERE id = 'sig-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("read");
        assert_eq!(result, "BLOCKED_LIVE_DISABLED");
        assert_ne!(result, "BLOCKED_OTHER");
        assert_ne!(result, "BLOCKED_PENDING_CONFIRM");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retryable_unexecuted_picks_failed_live_buys() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE signals (
                id TEXT PRIMARY KEY,
                symbol TEXT,
                action TEXT,
                confidence REAL,
                rationale TEXT,
                technical_snapshot TEXT,
                model_name TEXT,
                prompt_version TEXT,
                risk_policy_result TEXT,
                executed INTEGER,
                generated_at TEXT DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES ('buy-1', 'UACN', 'BUY', 0.7, 'idea', '{}', 'llm', 'v2.4.0', 'BLOCKED_BROKER', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES ('done', 'GTCO', 'BUY', 0.7, 'idea', '{}', 'llm', 'v2.4.0', 'APPROVED', 1)",
            [],
        )
        .unwrap();
        let ids = super::retryable_unexecuted_signal_ids(&conn, 24, 10, true).unwrap();
        assert_eq!(ids, vec!["buy-1".to_string()]);
    }

    #[test]
    fn retryable_excludes_blocked_cash_buys_and_all_buys_when_disallowed() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE signals (
                id TEXT PRIMARY KEY,
                symbol TEXT,
                action TEXT,
                confidence REAL,
                rationale TEXT,
                technical_snapshot TEXT,
                model_name TEXT,
                prompt_version TEXT,
                risk_policy_result TEXT,
                executed INTEGER,
                generated_at TEXT DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES ('cash-buy', 'UACN', 'BUY', 0.7, 'idea', '{}', 'llm', 'v2.4.0', 'BLOCKED_CASH', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES ('broker-buy', 'GTCO', 'BUY', 0.7, 'idea', '{}', 'llm', 'v2.4.0', 'BLOCKED_BROKER', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO signals (id, symbol, action, confidence, rationale, technical_snapshot, model_name, prompt_version, risk_policy_result, executed)
             VALUES ('sell-1', 'NIDF', 'SELL', 0.9, 'stop', '{}', 'rules:stop-loss', 'v2.4.0', 'BLOCKED_MARKET_CLOSED', 0)",
            [],
        )
        .unwrap();
        let with_buys = super::retryable_unexecuted_signal_ids(&conn, 24, 10, true).unwrap();
        assert!(!with_buys.iter().any(|id| id == "cash-buy"));
        assert!(with_buys.iter().any(|id| id == "broker-buy"));
        assert!(with_buys.iter().any(|id| id == "sell-1"));
        assert_eq!(with_buys.len(), 2);
        let sells_only = super::retryable_unexecuted_signal_ids(&conn, 24, 10, false).unwrap();
        assert_eq!(sells_only, vec!["sell-1".to_string()]);
    }
}

