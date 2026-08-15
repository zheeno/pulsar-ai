//! Shared stop-loss / take-profit evaluation (cycle + continuous risk monitor).

use std::collections::HashMap;

use crate::signals::HeldLot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    StopLoss,
    TakeProfit,
}

impl ExitKind {
    pub fn model_name(self) -> &'static str {
        match self {
            Self::StopLoss => "rules:stop-loss",
            Self::TakeProfit => "rules:take-profit",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::StopLoss => "stop_loss",
            Self::TakeProfit => "take_profit",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExitOrder {
    pub symbol: String,
    pub kind: ExitKind,
    pub rationale: String,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct PositionExitAssessment {
    pub last: Option<f64>,
    pub pnl_pct: Option<f64>,
    pub exit_hint: &'static str,
    pub exit: Option<ExitOrder>,
}

/// Evaluate one lot against stop-loss / take-profit thresholds.
pub fn assess_position_exit(
    symbol: &str,
    avg_cost: f64,
    last: Option<f64>,
    stop_loss_pct: f64,
    take_profit_pct: Option<f64>,
) -> PositionExitAssessment {
    let pnl_pct = if avg_cost > 0.0 {
        last.map(|px| (px - avg_cost) / avg_cost)
    } else {
        None
    };

    let exit = match (pnl_pct, last) {
        (Some(pnl), Some(px)) if pnl <= -stop_loss_pct => Some(ExitOrder {
            symbol: symbol.to_string(),
            kind: ExitKind::StopLoss,
            rationale: format!(
                "Stop loss: {:+.1}% vs {:.0}% threshold (avg {:.2}, last {:.2})",
                pnl * 100.0,
                stop_loss_pct * 100.0,
                avg_cost,
                px
            ),
            confidence: 1.0,
        }),
        (Some(pnl), Some(px))
            if take_profit_pct
                .map(|tp| tp > 0.0 && pnl >= tp)
                .unwrap_or(false) =>
        {
            let tp = take_profit_pct.unwrap_or(0.0);
            Some(ExitOrder {
                symbol: symbol.to_string(),
                kind: ExitKind::TakeProfit,
                rationale: format!(
                    "Take profit: {:+.1}% vs {:.0}% threshold (avg {:.2}, last {:.2})",
                    pnl * 100.0,
                    tp * 100.0,
                    avg_cost,
                    px
                ),
                confidence: 1.0,
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
        let assessment = assess_position_exit(
            &lot.symbol,
            lot.avg_cost,
            last,
            stop_loss_pct,
            take_profit_pct,
        );
        if let Some(exit) = assessment.exit {
            seen.insert(key);
            out.push(exit);
        }
    }
    out
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
        // Degenerate: only stop can match negative pnl; document precedence in assess.
        let a = assess_position_exit("X", 100.0, Some(90.0), 0.05, Some(0.10));
        assert_eq!(a.exit_hint, "stop_loss");
        let b = assess_position_exit("X", 100.0, Some(115.0), 0.05, Some(0.10));
        assert_eq!(b.exit_hint, "take_profit");
        let c = assess_position_exit("X", 100.0, Some(100.0), 0.05, Some(0.10));
        assert_eq!(c.exit_hint, "hold");
        assert!(c.exit.is_none());
    }
}
