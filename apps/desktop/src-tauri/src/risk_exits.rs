//! Shared stop-loss / take-profit / time-stop evaluation (cycle + continuous risk monitor).

use std::collections::HashMap;

use crate::signals::HeldLot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    StopLoss,
    TakeProfit,
    TimeStop,
    Flatten,
}

impl ExitKind {
    pub fn model_name(self) -> &'static str {
        match self {
            Self::StopLoss => "rules:stop-loss",
            Self::TakeProfit => "rules:take-profit",
            Self::TimeStop => "rules:time-stop",
            Self::Flatten => "rules:flatten",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::StopLoss => "stop_loss",
            Self::TakeProfit => "take_profit",
            Self::TimeStop => "time_stop",
            Self::Flatten => "flatten",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExitOrder {
    pub symbol: String,
    pub kind: ExitKind,
    pub rationale: String,
    pub confidence: f64,
    /// 1.0 = full lot; (0,1) = partial take-profit scale-out.
    pub sell_fraction: f64,
}

#[derive(Debug, Clone)]
pub struct PositionExitAssessment {
    pub last: Option<f64>,
    pub pnl_pct: Option<f64>,
    pub exit_hint: &'static str,
    pub exit: Option<ExitOrder>,
}

#[derive(Debug, Clone, Copy)]
pub struct ExitParams {
    pub stop_loss_pct: f64,
    pub take_profit_pct: Option<f64>,
    pub time_stop_hours: f64,
    pub partial_tp_fraction: f64,
}

impl ExitParams {
    pub fn sl_tp(stop_loss_pct: f64, take_profit_pct: Option<f64>) -> Self {
        Self {
            stop_loss_pct,
            take_profit_pct,
            time_stop_hours: 0.0,
            partial_tp_fraction: 1.0,
        }
    }

    pub fn tp_fraction(self) -> f64 {
        if self.partial_tp_fraction <= 0.0 {
            1.0
        } else {
            self.partial_tp_fraction.clamp(0.1, 1.0)
        }
    }
}

/// Evaluate one lot against stop-loss / take-profit / time-stop.
pub fn assess_position_exit(
    symbol: &str,
    avg_cost: f64,
    last: Option<f64>,
    stop_loss_pct: f64,
    take_profit_pct: Option<f64>,
) -> PositionExitAssessment {
    assess_position_exit_full(
        symbol,
        avg_cost,
        last,
        ExitParams::sl_tp(stop_loss_pct, take_profit_pct),
        None,
    )
}

pub fn assess_position_exit_full(
    symbol: &str,
    avg_cost: f64,
    last: Option<f64>,
    params: ExitParams,
    hours_held: Option<f64>,
) -> PositionExitAssessment {
    let pnl_pct = if avg_cost > 0.0 {
        last.map(|px| (px - avg_cost) / avg_cost)
    } else {
        None
    };

    let exit = match (pnl_pct, last) {
        (Some(pnl), Some(px)) if pnl <= -params.stop_loss_pct => Some(ExitOrder {
            symbol: symbol.to_string(),
            kind: ExitKind::StopLoss,
            rationale: format!(
                "Stop loss: {:+.1}% vs {:.0}% threshold (avg {:.2}, last {:.2})",
                pnl * 100.0,
                params.stop_loss_pct * 100.0,
                avg_cost,
                px
            ),
            confidence: 1.0,
            sell_fraction: 1.0,
        }),
        (Some(pnl), Some(px))
            if params
                .take_profit_pct
                .map(|tp| tp > 0.0 && pnl >= tp)
                .unwrap_or(false) =>
        {
            let tp = params.take_profit_pct.unwrap_or(0.0);
            let frac = params.tp_fraction();
            Some(ExitOrder {
                symbol: symbol.to_string(),
                kind: ExitKind::TakeProfit,
                rationale: format!(
                    "Take profit: {:+.1}% vs {:.0}% threshold (avg {:.2}, last {:.2}); sell {:.0}% of lot",
                    pnl * 100.0,
                    tp * 100.0,
                    avg_cost,
                    px,
                    frac * 100.0
                ),
                confidence: 1.0,
                sell_fraction: frac,
            })
        }
        (Some(_), Some(px))
            if params.time_stop_hours > 0.0
                && hours_held.map(|h| h >= params.time_stop_hours).unwrap_or(false) =>
        {
            let held = hours_held.unwrap_or(0.0);
            Some(ExitOrder {
                symbol: symbol.to_string(),
                kind: ExitKind::TimeStop,
                rationale: format!(
                    "Time stop: held {held:.1}h ≥ {:.0}h (avg {:.2}, last {:.2}) — recycle capital",
                    params.time_stop_hours, avg_cost, px
                ),
                confidence: 1.0,
                sell_fraction: 1.0,
            })
        }
        _ => None,
    };

    let exit_hint = exit.as_ref().map(|e| e.kind.hint()).unwrap_or("hold");
    PositionExitAssessment {
        last,
        pnl_pct,
        exit_hint,
        exit,
    }
}

/// Evaluate held lots; `prices` overrides `lot.last_price` when present (keyed by uppercased symbol).
pub fn evaluate_position_exits(
    lots: &[HeldLot],
    stop_loss_pct: f64,
    take_profit_pct: Option<f64>,
    prices: &HashMap<String, f64>,
) -> Vec<ExitOrder> {
    evaluate_position_exits_full(
        lots,
        ExitParams::sl_tp(stop_loss_pct, take_profit_pct),
        prices,
        false,
    )
}

pub fn evaluate_position_exits_full(
    lots: &[HeldLot],
    params: ExitParams,
    prices: &HashMap<String, f64>,
    flatten: bool,
) -> Vec<ExitOrder> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for lot in lots {
        if lot.quantity <= 0.0 {
            continue;
        }
        let key = lot.symbol.to_uppercase();
        if seen.contains(&key) {
            continue;
        }
        let last = prices
            .get(&key)
            .copied()
            .filter(|p| *p > 0.0)
            .or_else(|| lot.last_price.filter(|p| *p > 0.0));
        if flatten {
            seen.insert(key);
            out.push(ExitOrder {
                symbol: lot.symbol.clone(),
                kind: ExitKind::Flatten,
                rationale: "Armed flatten-on-drawdown: sell open lot".into(),
                confidence: 1.0,
                sell_fraction: 1.0,
            });
            continue;
        }
        let assessment = assess_position_exit_full(
            &lot.symbol,
            lot.avg_cost,
            last,
            params,
            lot.hours_held(),
        );
        if let Some(exit) = assessment.exit {
            seen.insert(key);
            out.push(exit);
        }
    }
    out
}

/// Whole (or fractional) quantity to sell for an exit.
/// `whole_shares`: NGX-style integer lots. Crypto passes `false` so a full exit
/// can liquidate the exact held amount in one order.
pub fn sell_qty_for_exit(available: f64, fraction: f64) -> f64 {
    sell_qty_for_exit_ex(available, fraction, true)
}

pub fn sell_qty_for_exit_ex(available: f64, fraction: f64, whole_shares: bool) -> f64 {
    sell_qty_for_exit_with_decimals(available, fraction, whole_shares, None)
}

/// Optional `crypto_decimals` (max 8 dp) floors fractional crypto exit sizes before quoting.
pub fn sell_qty_for_exit_with_decimals(
    available: f64,
    fraction: f64,
    whole_shares: bool,
    crypto_decimals: Option<usize>,
) -> f64 {
    if available <= 0.0 {
        return 0.0;
    }
    let frac = if fraction <= 0.0 {
        1.0
    } else {
        fraction.clamp(0.1, 1.0)
    };
    if frac >= 1.0 - 1e-9 {
        return if whole_shares {
            available.floor().max(0.0)
        } else if let Some(d) = crypto_decimals {
            crate::busha::floor_crypto_amount(available, d)
        } else {
            available.max(0.0)
        };
    }
    let qty = if whole_shares {
        (available * frac).floor()
    } else if let Some(d) = crypto_decimals {
        crate::busha::floor_crypto_amount(available * frac, d)
    } else {
        available * frac
    };
    if whole_shares && qty < 1.0 {
        1.0_f64.min(available.floor())
    } else if !whole_shares && qty <= 0.0 {
        0.0
    } else if whole_shares {
        qty.min(available.floor())
    } else if let Some(d) = crypto_decimals {
        qty.min(crate::busha::floor_crypto_amount(available, d))
    } else {
        qty.min(available)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lot(symbol: &str, qty: f64, avg: f64, last: Option<f64>) -> HeldLot {
        HeldLot {
            symbol: symbol.into(),
            quantity: qty,
            avg_cost: avg,
            last_price: last,
            opened_at: None,
        }
    }

    fn aged(symbol: &str, qty: f64, avg: f64, last: Option<f64>, hours: f64) -> HeldLot {
        let opened = chrono::Utc::now() - chrono::Duration::seconds((hours * 3600.0) as i64);
        HeldLot {
            symbol: symbol.into(),
            quantity: qty,
            avg_cost: avg,
            last_price: last,
            opened_at: Some(opened.format("%Y-%m-%d %H:%M:%S").to_string()),
        }
    }

    #[test]
    fn stop_loss_fires_when_pnl_at_or_below_threshold() {
        let lots = vec![lot("GTCO", 100.0, 50.0, Some(45.0))]; // -10%
        let prices = HashMap::new();
        let exits = evaluate_position_exits(&lots, 0.08, Some(0.15), &prices);
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].kind, ExitKind::StopLoss);
        assert_eq!(exits[0].symbol, "GTCO");
        assert!(exits[0].rationale.contains("Stop loss"));
        assert_eq!(exits[0].sell_fraction, 1.0);
    }

    #[test]
    fn take_profit_fires_when_pnl_at_or_above_threshold() {
        let lots = vec![lot("MTNN", 50.0, 100.0, Some(120.0))]; // +20%
        let prices = HashMap::new();
        let exits = evaluate_position_exits(&lots, 0.08, Some(0.15), &prices);
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].kind, ExitKind::TakeProfit);
    }

    #[test]
    fn neither_when_flat_inside_band() {
        let lots = vec![lot("GTCO", 100.0, 50.0, Some(51.0))]; // +2%
        let prices = HashMap::new();
        let exits = evaluate_position_exits(&lots, 0.08, Some(0.15), &prices);
        assert!(exits.is_empty());
    }

    #[test]
    fn price_map_overrides_lot_last() {
        let lots = vec![lot("GTCO", 100.0, 50.0, Some(52.0))];
        let mut prices = HashMap::new();
        prices.insert("GTCO".into(), 40.0); // -20%
        let exits = evaluate_position_exits(&lots, 0.08, Some(0.15), &prices);
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].kind, ExitKind::StopLoss);
    }

    #[test]
    fn zero_qty_skipped() {
        let lots = vec![lot("GTCO", 0.0, 50.0, Some(40.0))];
        let prices = HashMap::new();
        assert!(evaluate_position_exits(&lots, 0.08, Some(0.15), &prices).is_empty());
    }

    #[test]
    fn take_profit_disabled_when_none_or_zero() {
        let lots = vec![lot("GTCO", 100.0, 50.0, Some(70.0))];
        let prices = HashMap::new();
        assert!(evaluate_position_exits(&lots, 0.08, None, &prices).is_empty());
        assert!(evaluate_position_exits(&lots, 0.08, Some(0.0), &prices).is_empty());
    }

    #[test]
    fn stop_loss_preferred_over_take_profit_when_both_would_match() {
        let a = assess_position_exit("X", 100.0, Some(90.0), 0.05, Some(0.10));
        assert_eq!(a.exit_hint, "stop_loss");
        let b = assess_position_exit("X", 100.0, Some(115.0), 0.05, Some(0.10));
        assert_eq!(b.exit_hint, "take_profit");
        let c = assess_position_exit("X", 100.0, Some(100.0), 0.05, Some(0.10));
        assert_eq!(c.exit_hint, "hold");
        assert!(c.exit.is_none());
    }

    #[test]
    fn time_stop_recycles_in_band_lot() {
        let lots = vec![aged("GTCO", 80.0, 50.0, Some(51.0), 30.0)];
        let params = ExitParams {
            stop_loss_pct: 0.08,
            take_profit_pct: Some(0.15),
            time_stop_hours: 24.0,
            partial_tp_fraction: 1.0,
        };
        let exits = evaluate_position_exits_full(&lots, params, &HashMap::new(), false);
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].kind, ExitKind::TimeStop);
        assert_eq!(sell_qty_for_exit(80.0, 1.0), 80.0);
        assert_eq!(sell_qty_for_exit_ex(0.75, 1.0, false), 0.75);
        assert_eq!(sell_qty_for_exit_ex(1.75, 1.0, true), 1.0);
        assert_eq!(
            sell_qty_for_exit_with_decimals(12.345678912, 1.0, false, Some(8)),
            12.34567891
        );
    }

    #[test]
    fn time_stop_off_leaves_in_band() {
        let lots = vec![aged("GTCO", 80.0, 50.0, Some(51.0), 30.0)];
        let params = ExitParams {
            stop_loss_pct: 0.08,
            take_profit_pct: Some(0.15),
            time_stop_hours: 0.0,
            partial_tp_fraction: 1.0,
        };
        let exits = evaluate_position_exits_full(&lots, params, &HashMap::new(), false);
        assert!(exits.is_empty());
    }

    #[test]
    fn partial_tp_fraction_on_take_profit() {
        let lots = vec![lot("MTNN", 100.0, 100.0, Some(120.0))];
        let params = ExitParams {
            stop_loss_pct: 0.08,
            take_profit_pct: Some(0.15),
            time_stop_hours: 0.0,
            partial_tp_fraction: 0.5,
        };
        let exits = evaluate_position_exits_full(&lots, params, &HashMap::new(), false);
        assert_eq!(exits[0].kind, ExitKind::TakeProfit);
        assert!((exits[0].sell_fraction - 0.5).abs() < 1e-9);
        assert_eq!(sell_qty_for_exit(100.0, 0.5), 50.0);
    }

    #[test]
    fn flatten_sells_all_lots() {
        let lots = vec![
            lot("GTCO", 10.0, 50.0, Some(51.0)),
            lot("MTNN", 5.0, 100.0, Some(101.0)),
        ];
        let exits = evaluate_position_exits_full(
            &lots,
            ExitParams::sl_tp(0.08, Some(0.15)),
            &HashMap::new(),
            true,
        );
        assert_eq!(exits.len(), 2);
        assert!(exits.iter().all(|e| e.kind == ExitKind::Flatten));
    }
}
