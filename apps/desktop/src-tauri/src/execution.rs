use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cache::PriceCache;
use crate::settings::AppSettings;
use crate::wealth::{insert_broker_order, TradingMode, WealthClient};

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
            let quantity = Self::position_size(total_equity, current_price, param_set, position_value);
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
                if *qty > 0.0 {
                    return ("APPROVED".into(), *qty);
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
}

pub struct FillSimulator {
    slippage_bps: f64,
    fee_pct: f64,
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
        conn: &Connection,
        cache: &PriceCache,
        settings: &AppSettings,
        signal_ids: &[String],
        wealth: Option<&WealthClient>,
        trading_mode: TradingMode,
        live_market_open: bool,
    ) -> Result<(i64, Vec<String>)> {
        let mut executed = 0;
        let mut warnings = Vec::new();

        if trading_mode == TradingMode::Live && !live_market_open {
            warnings.push(
                "Live trader mode: Wealth market is closed — skipping live fills (no sandbox fallback)."
                    .into(),
            );
            return Ok((0, warnings));
        }

        for signal_id in signal_ids {
            match Self::process_signal(
                conn,
                cache,
                settings,
                signal_id,
                wealth,
                trading_mode,
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
        conn: &Connection,
        cache: &PriceCache,
        settings: &AppSettings,
        signal_id: &str,
        wealth: Option<&WealthClient>,
        trading_mode: TradingMode,
    ) -> Result<bool> {
        let signal: Option<(String, String, f64)> = conn
            .query_row(
                "SELECT symbol, action, confidence FROM signals WHERE id = ?1",
                [signal_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();

        let Some((symbol, action, confidence)) = signal else {
            return Ok(false);
        };

        let portfolio: (String, f64, String) = conn.query_row(
            "SELECT id, cash_balance, strategy_param_set_id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        let param_set = Self::load_param_set(conn, &portfolio.2)?;

        let (cash_balance, positions, prices, daily_trades) = if trading_mode == TradingMode::Live {
            let client = wealth.ok_or_else(|| anyhow::anyhow!("Wealth client required for live mode"))?;
            let wallet = client.get_wallet().await?;
            let snap = client.get_portfolio().await?;
            let positions: Vec<(String, f64, f64)> = snap
                .holdings
                .iter()
                .map(|h| {
                    (
                        h.symbol.clone(),
                        h.quantity,
                        h.buy_price.unwrap_or(h.price),
                    )
                })
                .collect();
            let mut symbols: Vec<String> = positions.iter().map(|(s, _, _)| s.clone()).collect();
            if !symbols.contains(&symbol) {
                symbols.push(symbol.clone());
            }
            let mut prices = Self::get_prices(conn, cache, &symbols)?;
            for h in &snap.holdings {
                if h.price > 0.0 {
                    prices.insert(h.symbol.clone(), h.price);
                }
            }
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let daily_trades: i64 = conn.query_row(
                "SELECT COUNT(*) FROM broker_orders WHERE date(created_at) = ?1 AND status = 'executed'",
                [&today],
                |row| row.get(0),
            )?;
            (wallet.brokerage_balance, positions, prices, daily_trades)
        } else {
            let positions = Self::load_positions(conn, &portfolio.0)?;
            let mut symbols: Vec<String> = positions.iter().map(|(s, _, _)| s.clone()).collect();
            if !symbols.contains(&symbol) {
                symbols.push(symbol.clone());
            }
            let prices = Self::get_prices(conn, cache, &symbols)?;
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let daily_trades: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sandbox_trades WHERE portfolio_id = ?1 AND date(executed_at) = ?2",
                rusqlite::params![portfolio.0, today],
                |row| row.get(0),
            )?;
            (portfolio.1, positions, prices, daily_trades)
        };

        let daily_drawdown: f64 = conn
            .query_row(
                "SELECT drawdown_pct FROM daily_performance_snapshot WHERE portfolio_id = ?1 ORDER BY snapshot_date DESC LIMIT 1",
                [&portfolio.0],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        let input = SignalInput {
            id: signal_id.into(),
            symbol: symbol.clone(),
            action: action.clone(),
            confidence,
        };
        let (result, quantity) = RiskPolicyService::evaluate(
            &input,
            &param_set,
            cash_balance,
            &positions,
            &prices,
            daily_trades,
            daily_drawdown,
        );

        conn.execute(
            "UPDATE signals SET risk_policy_result = ?1 WHERE id = ?2",
            rusqlite::params![result, signal_id],
        )?;

        if result != "APPROVED" || quantity <= 0.0 {
            return Ok(false);
        }

        if trading_mode == TradingMode::Live {
            let client = wealth.ok_or_else(|| anyhow::anyhow!("Wealth client required for live mode"))?;
            Self::execute_live_trade(conn, client, signal_id, &symbol, &action, quantity).await?;
        } else {
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
                cash_balance,
            )?;
        }

        conn.execute("UPDATE signals SET executed = 1 WHERE id = ?1", [signal_id])?;
        Ok(true)
    }

    async fn execute_live_trade(
        conn: &Connection,
        client: &WealthClient,
        signal_id: &str,
        symbol: &str,
        side: &str,
        quantity: f64,
    ) -> Result<()> {
        let (stock_id, price) = client.resolve_stock_id(symbol).await?;
        if price <= 0.0 {
            return Err(anyhow::anyhow!("No Wealth price for {symbol}"));
        }

        let fee = client.calculate_fee(stock_id, quantity, price).await?;
        if side == "BUY" {
            let wallet = client.get_wallet().await?;
            let cost = price * quantity + fee.rounded_fee;
            if wallet.brokerage_balance < cost {
                return Err(anyhow::anyhow!(
                    "Insufficient brokerage balance for {symbol} (need ₦{cost:.2}, have ₦{:.2})",
                    wallet.brokerage_balance
                ));
            }
        } else {
            let snap = client.get_portfolio().await?;
            let held = snap
                .holdings
                .iter()
                .find(|h| h.symbol.eq_ignore_ascii_case(symbol))
                .map(|h| h.quantity)
                .unwrap_or(0.0);
            if quantity > held {
                return Err(anyhow::anyhow!(
                    "Cannot sell {quantity} of {symbol}; holding {held}"
                ));
            }
        }

        let order = client
            .place_and_await_fill(stock_id, side, quantity)
            .await?;

        let fill_price = order.unit_price.or(order.quote_price).or(Some(price));
        insert_broker_order(
            conn,
            Some(signal_id),
            symbol,
            side,
            quantity,
            &order,
            fill_price,
            Some(fee.rounded_fee),
        )?;

        if order.status == "rejected" {
            return Err(anyhow::anyhow!(
                "Wealth order rejected: {}",
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
        Ok(())
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
}
