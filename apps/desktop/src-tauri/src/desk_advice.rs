//! Read-only desk playbook for a live Busha book that is small or down hard.
//! Advice only — never writes settings, strategy, memories, or the blotter.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::app_state::AppState;
use crate::settings::{get_settings, AppSettings};
use crate::strategy_coach::StrategyParams;

pub const SMALL_BOOK_NGN: f64 = 40_000.0;
pub const DROP_24H_TRIGGER: f64 = 0.10;
const LOOKBACK_HOURS: i64 = 24;
const FALLBACK_SPAN_HOURS: i64 = 20;

const NOTE: &str =
    "Advice only. Do not apply a patch unless the user asks to preview one. Do not raise minConfidence. Do not persist this as Memory. KPI is expectancy after venue costs, not win rate.";

#[derive(Debug, Clone)]
pub struct DeskAdviceInput {
    pub asset_class: String,
    pub trading_mode: String,
    pub broker_id: String,
    pub live_trading_enabled: bool,
    pub auto_cycle_enabled: bool,
    pub halt_new_buys: bool,
    pub auto_cycle_interval_minutes: u32,
    pub equity: Option<f64>,
    pub equity_24h_ago: Option<f64>,
    pub max_position_pct: f64,
    pub cycle_budget_pct: f64,
    pub min_confidence_to_trade: f64,
    pub max_daily_drawdown_pct: f64,
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
    pub time_stop_hours: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AdviceStep {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub aligned: bool,
    pub current: Option<String>,
    pub suggested: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeskAdvice {
    pub ok: bool,
    pub applicable: bool,
    pub severity: String,
    pub reasons: Vec<String>,
    pub title: String,
    pub headline: String,
    pub now_steps: Vec<AdviceStep>,
    pub slider_steps: Vec<AdviceStep>,
    pub kpi: String,
    pub do_not: Vec<String>,
    pub note: String,
    pub applied: bool,
    pub equity: Option<f64>,
    pub equity_24h_ago: Option<f64>,
    pub drop_24h_pct: Option<f64>,
    pub last_cycle_id: Option<String>,
}

impl Default for DeskAdviceInput {
    fn default() -> Self {
        Self {
            asset_class: "stocks".into(),
            trading_mode: "sandbox".into(),
            broker_id: "wealth".into(),
            live_trading_enabled: false,
            auto_cycle_enabled: false,
            halt_new_buys: false,
            auto_cycle_interval_minutes: 30,
            equity: None,
            equity_24h_ago: None,
            max_position_pct: 0.10,
            cycle_budget_pct: 0.20,
            min_confidence_to_trade: 0.65,
            max_daily_drawdown_pct: 0.03,
            stop_loss_pct: 0.05,
            take_profit_pct: 0.10,
            time_stop_hours: 0.0,
        }
    }
}

pub fn drop_24h_pct(equity: Option<f64>, ago: Option<f64>) -> Option<f64> {
    let now = equity.filter(|n| n.is_finite())?;
    let prior = ago.filter(|n| n.is_finite() && *n > 0.0)?;
    Some((prior - now) / prior)
}

pub fn is_live_crypto(input: &DeskAdviceInput) -> bool {
    let crypto = input.asset_class.eq_ignore_ascii_case("crypto")
        || input.broker_id.eq_ignore_ascii_case("busha");
    let live_book = matches!(
        input.trading_mode.to_ascii_lowercase().as_str(),
        "live" | "paused"
    );
    crypto && live_book
}

pub fn in_range(value: f64, lo: f64, hi: f64) -> bool {
    value.is_finite() && value + 1e-9 >= lo && value - 1e-9 <= hi
}

fn pct_label(ratio: f64) -> String {
    format!("{:.0}%", (ratio * 100.0).round())
}

fn hours_label(hours: f64) -> String {
    if hours <= 0.0 {
        "Off (Busha 72h fallback)".into()
    } else {
        format!("{:.0} h", hours.round())
    }
}

fn minutes_label(minutes: u32) -> String {
    format!("{minutes} min")
}

pub fn evaluate(input: &DeskAdviceInput) -> DeskAdvice {
    let drop = drop_24h_pct(input.equity, input.equity_24h_ago);
    let drop_hit = drop.is_some_and(|d| d + 1e-12 >= DROP_24H_TRIGGER);
    let small_book = input.equity.is_some_and(|e| e.is_finite() && e < SMALL_BOOK_NGN);
    let live_crypto = is_live_crypto(input);
    let applicable = live_crypto && (drop_hit || small_book);

    let mut reasons = Vec::new();
    if live_crypto {
        reasons.push("Live Busha crypto book".into());
    }
    if small_book {
        if let Some(eq) = input.equity {
            reasons.push(format!(
                "Equity ₦{:.0} is under the ₦{:.0} small-book line",
                eq.round(),
                SMALL_BOOK_NGN
            ));
        }
    }
    if drop_hit {
        if let (Some(d), Some(eq), Some(ago)) = (drop, input.equity, input.equity_24h_ago) {
            reasons.push(format!(
                "≈{:.0}% equity drop in ~24h (₦{:.0} → ₦{:.0})",
                (d * 100.0).round(),
                ago.round(),
                eq.round()
            ));
        }
    }

    let headline = if !applicable {
        "Playbook is on file. Home only surfaces it on a live Busha book that is under ₦40k or down 10%+ in ~24h.".into()
    } else if drop_hit && small_book {
        "Live Busha book is small and down hard. This is advice — Pulsar did not change sliders, halt, or auto-cycle.".into()
    } else if drop_hit {
        "Live Busha book is down 10%+ in about a day. This is advice — nothing was applied.".into()
    } else {
        "Live Busha book is under ₦40k. Size down. This is advice — nothing was applied.".into()
    };

    DeskAdvice {
        ok: true,
        applicable,
        severity: if applicable {
            "protect".into()
        } else {
            "none".into()
        },
        reasons,
        title: if applicable {
            "Protect the book".into()
        } else {
            "Small-book playbook".into()
        },
        headline,
        now_steps: now_steps(input),
        slider_steps: slider_steps(input),
        kpi: "Judge the desk by expectancy after Busha fees and spread — not by win rate. Do not promise an 80% hit rate.".into(),
        do_not: vec![
            "Do not raise min confidence from the journal or a lesson.".into(),
            "Do not blacklist a symbol after one losing close.".into(),
            "Do not sell on a headline alone.".into(),
            "Do not tighten stops to 1–2% — Busha noise will shake you out.".into(),
        ],
        note: NOTE.into(),
        applied: false,
        equity: input.equity,
        equity_24h_ago: input.equity_24h_ago,
        drop_24h_pct: drop,
        last_cycle_id: None,
    }
}

fn now_steps(input: &DeskAdviceInput) -> Vec<AdviceStep> {
    vec![
        AdviceStep {
            id: "auto_cycle".into(),
            label: "Turn off auto-cycle".into(),
            detail: "Opening the app must not fire a buy cycle. Run by hand when you mean it.".into(),
            aligned: !input.auto_cycle_enabled,
            current: Some(if input.auto_cycle_enabled {
                "On".into()
            } else {
                "Off".into()
            }),
            suggested: Some("Off".into()),
        },
        AdviceStep {
            id: "halt_new_buys".into(),
            label: "Halt new buys".into(),
            detail: "Block fresh BUYs. Protective sells (stop / take-profit / time-stop) still run while the app is open.".into(),
            aligned: input.halt_new_buys,
            current: Some(if input.halt_new_buys {
                "On".into()
            } else {
                "Off".into()
            }),
            suggested: Some("On".into()),
        },
        AdviceStep {
            id: "live_trading".into(),
            label: "Keep live on only for stops".into(),
            detail: "Leave live trading on if you want the risk monitor to submit protective sells. Turn it off if you want the book frozen.".into(),
            aligned: true,
            current: Some(if input.live_trading_enabled {
                "On".into()
            } else {
                "Off".into()
            }),
            suggested: Some("On only if stops should still fire".into()),
        },
        AdviceStep {
            id: "cycle_interval".into(),
            label: "If auto-cycle comes back later, slow it down".into(),
            detail: "90–120 minutes between cycles. The default 30 minutes is too chatty on a small NGN book.".into(),
            aligned: !input.auto_cycle_enabled
                || (input.auto_cycle_interval_minutes >= 90
                    && input.auto_cycle_interval_minutes <= 120),
            current: Some(minutes_label(input.auto_cycle_interval_minutes)),
            suggested: Some("90–120 min".into()),
        },
    ]
}

fn slider_steps(input: &DeskAdviceInput) -> Vec<AdviceStep> {
    vec![
        AdviceStep {
            id: "cycle_budget_pct".into(),
            label: "Cycle cash budget".into(),
            detail: "Spend 5–10% of cash per cycle — not the default 20%.".into(),
            aligned: in_range(input.cycle_budget_pct, 0.05, 0.10),
            current: Some(pct_label(input.cycle_budget_pct)),
            suggested: Some("5–10%".into()),
        },
        AdviceStep {
            id: "max_position_pct".into(),
            label: "Max position".into(),
            detail: "Cap any one name at 5–8% of equity.".into(),
            aligned: in_range(input.max_position_pct, 0.05, 0.08),
            current: Some(pct_label(input.max_position_pct)),
            suggested: Some("5–8%".into()),
        },
        AdviceStep {
            id: "max_daily_drawdown_pct".into(),
            label: "Max daily drawdown".into(),
            detail: "2–3% blocks new buys after a bad session. 100% turns that brake off.".into(),
            aligned: in_range(input.max_daily_drawdown_pct, 0.02, 0.03),
            current: Some(pct_label(input.max_daily_drawdown_pct)),
            suggested: Some("2–3%".into()),
        },
        AdviceStep {
            id: "time_stop_hours".into(),
            label: "Time-stop".into(),
            detail: "8–24 hours. 0 leaves Busha on the 72h fallback.".into(),
            aligned: in_range(input.time_stop_hours, 8.0, 24.0),
            current: Some(hours_label(input.time_stop_hours)),
            suggested: Some("8–24 h".into()),
        },
        AdviceStep {
            id: "stop_loss_pct".into(),
            label: "Stop loss".into(),
            detail: "6–8%. Do not tighten to 1–2% — crypto noise will stop you out.".into(),
            aligned: in_range(input.stop_loss_pct, 0.06, 0.08),
            current: Some(pct_label(input.stop_loss_pct)),
            suggested: Some("6–8%".into()),
        },
        AdviceStep {
            id: "take_profit_pct".into(),
            label: "Take profit".into(),
            detail: "12–15% so winners can pay for Busha costs.".into(),
            aligned: in_range(input.take_profit_pct, 0.12, 0.15),
            current: Some(pct_label(input.take_profit_pct)),
            suggested: Some("12–15%".into()),
        },
        AdviceStep {
            id: "min_confidence_to_trade".into(),
            label: "Min confidence".into(),
            detail: "Leave it near 65–70%. Journal buckets and lessons are not a knob.".into(),
            aligned: in_range(input.min_confidence_to_trade, 0.65, 0.70),
            current: Some(pct_label(input.min_confidence_to_trade)),
            suggested: Some("65–70% (leave it)".into()),
        },
    ]
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

pub fn equity_now_and_24h(
    conn: &Connection,
    venue: &str,
) -> Result<(Option<f64>, Option<f64>)> {
    let now: Option<f64> = conn
        .query_row(
            "SELECT total_equity FROM equity_curve_points
             WHERE venue = ?1
             ORDER BY recorded_at DESC LIMIT 1",
            [venue],
            |r| r.get(0),
        )
        .optional()?;

    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(LOOKBACK_HOURS))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let ago: Option<f64> = conn
        .query_row(
            "SELECT total_equity FROM equity_curve_points
             WHERE venue = ?1 AND recorded_at <= ?2
             ORDER BY recorded_at DESC LIMIT 1",
            rusqlite::params![venue, cutoff],
            |r| r.get(0),
        )
        .optional()?;
    if ago.is_some() {
        return Ok((now, ago));
    }

    let oldest: Option<(String, f64)> = conn
        .query_row(
            "SELECT recorded_at, total_equity FROM equity_curve_points
             WHERE venue = ?1
             ORDER BY recorded_at ASC LIMIT 1",
            [venue],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((recorded_at, equity)) = oldest {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&recorded_at) {
            let age = chrono::Utc::now().signed_duration_since(ts.with_timezone(&chrono::Utc));
            if age >= chrono::Duration::hours(FALLBACK_SPAN_HOURS) {
                return Ok((now, Some(equity)));
            }
        }
    }
    Ok((now, None))
}

fn input_from_db(conn: &Connection, settings: &AppSettings) -> Result<DeskAdviceInput> {
    let params = crate::strategy_coach::read_active_strategy(conn)
        .map(|row| row.params)
        .unwrap_or(StrategyParams {
            max_position_pct: 0.10,
            stop_loss_pct: 0.05,
            take_profit_pct: Some(0.10),
            min_confidence_to_trade: 0.65,
            max_daily_drawdown_pct: 0.03,
            cycle_budget_pct: 0.20,
            time_stop_hours: 0.0,
            partial_tp_fraction: 1.0,
        });

    let crypto = crate::broker::crypto_mode();
    let broker_id = if crypto {
        "busha".to_string()
    } else {
        settings.selected_broker.clone()
    };
    let venue = if crypto {
        "busha"
    } else if settings.wealth_connected || settings.bamboo_connected {
        broker_id.as_str()
    } else {
        "sandbox"
    };

    let book = cached_live_book(conn, settings);
    let (curve_now, curve_ago) = equity_now_and_24h(conn, venue)?;
    let equity = book
        .as_ref()
        .map(|b| b.brokerage_balance + b.market_value())
        .or(curve_now);

    let trading_mode = if crate::broker::crypto_reconnect_required() {
        "paused"
    } else if book.is_some() {
        "live"
    } else {
        "sandbox"
    };

    Ok(DeskAdviceInput {
        asset_class: crate::broker::asset_class().as_str().into(),
        trading_mode: trading_mode.into(),
        broker_id,
        live_trading_enabled: settings.live_trading_enabled,
        auto_cycle_enabled: settings.auto_cycle_enabled,
        halt_new_buys: settings.halt_new_buys,
        auto_cycle_interval_minutes: settings.auto_cycle_interval_minutes,
        equity,
        equity_24h_ago: curve_ago,
        max_position_pct: params.max_position_pct,
        cycle_budget_pct: params.cycle_budget_pct,
        min_confidence_to_trade: params.min_confidence_to_trade,
        max_daily_drawdown_pct: params.max_daily_drawdown_pct,
        stop_loss_pct: params.stop_loss_pct,
        take_profit_pct: params.take_profit_pct.unwrap_or(0.10),
        time_stop_hours: params.time_stop_hours,
    })
}

fn last_cycle_id(conn: &Connection) -> Result<Option<String>> {
    let value: Option<String> = conn
        .query_row(
            "SELECT COALESCE(NULLIF(cycle_id, ''), id, created_at)
             FROM cycle_audits
             ORDER BY created_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value.filter(|s| !s.is_empty()))
}

pub fn evaluate_from_db(conn: &Connection, settings: &AppSettings) -> Result<DeskAdvice> {
    let mut advice = evaluate(&input_from_db(conn, settings)?);
    advice.last_cycle_id = last_cycle_id(conn).ok().flatten();
    Ok(advice)
}

pub fn coach_tool(conn: &Connection, settings: &AppSettings) -> Result<serde_json::Value> {
    let advice = evaluate_from_db(conn, settings)?;
    Ok(serde_json::to_value(advice)?)
}

#[tauri::command]
pub fn desk_advice(state: State<'_, std::sync::Arc<AppState>>) -> Result<DeskAdvice, String> {
    let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
    state
        .db
        .with_conn(|conn| evaluate_from_db(conn, &settings))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_busha() -> DeskAdviceInput {
        DeskAdviceInput {
            asset_class: "crypto".into(),
            trading_mode: "live".into(),
            broker_id: "busha".into(),
            live_trading_enabled: true,
            auto_cycle_enabled: true,
            halt_new_buys: false,
            auto_cycle_interval_minutes: 30,
            equity: Some(15_376.79),
            equity_24h_ago: Some(21_822.11),
            ..DeskAdviceInput::default()
        }
    }

    #[test]
    fn live_busha_30pct_drop_is_protect_advice() {
        let advice = evaluate(&live_busha());
        assert!(advice.applicable);
        assert_eq!(advice.severity, "protect");
        assert!(!advice.applied);
        assert_eq!(advice.last_cycle_id, None);
        assert!(advice.note.contains("Do not raise minConfidence"));
        assert!(advice.note.contains("Do not persist this as Memory"));
        assert!(advice.headline.contains("did not change") || advice.headline.contains("nothing was applied"));
        let drop = advice.drop_24h_pct.unwrap();
        assert!(drop > 0.29 && drop < 0.31, "{drop}");
        assert!(advice.reasons.iter().any(|r| r.contains("24h")));
        assert!(advice.now_steps.iter().any(|s| s.id == "halt_new_buys" && !s.aligned));
        assert!(advice.slider_steps.iter().any(|s| s.id == "cycle_budget_pct" && !s.aligned));
        assert!(advice.kpi.to_lowercase().contains("expectancy"));
        assert!(advice.kpi.contains("not by win rate") || advice.kpi.contains("not win rate"));
    }

    #[test]
    fn live_crypto_small_book_without_curve_still_applies() {
        let mut input = live_busha();
        input.equity = Some(15_000.0);
        input.equity_24h_ago = None;
        let advice = evaluate(&input);
        assert!(advice.applicable);
        assert!(advice.reasons.iter().any(|r| r.contains("40")));
    }

    #[test]
    fn live_crypto_healthy_book_hides_card() {
        let mut input = live_busha();
        input.equity = Some(80_000.0);
        input.equity_24h_ago = Some(81_000.0);
        let advice = evaluate(&input);
        assert!(!advice.applicable);
        assert_eq!(advice.severity, "none");
        assert!(!advice.now_steps.is_empty());
        assert!(!advice.slider_steps.is_empty());
    }

    #[test]
    fn sandbox_ngx_never_surfaces_even_if_tiny() {
        let input = DeskAdviceInput {
            equity: Some(9_000.0),
            equity_24h_ago: Some(20_000.0),
            ..DeskAdviceInput::default()
        };
        let advice = evaluate(&input);
        assert!(!advice.applicable);
        assert!(!is_live_crypto(&input));
    }

    #[test]
    fn aligned_when_playbook_already_set() {
        let mut input = live_busha();
        input.auto_cycle_enabled = false;
        input.halt_new_buys = true;
        input.auto_cycle_interval_minutes = 90;
        input.cycle_budget_pct = 0.08;
        input.max_position_pct = 0.06;
        input.max_daily_drawdown_pct = 0.03;
        input.time_stop_hours = 12.0;
        input.stop_loss_pct = 0.07;
        input.take_profit_pct = 0.14;
        input.min_confidence_to_trade = 0.65;
        let advice = evaluate(&input);
        assert!(advice.now_steps.iter().all(|s| s.aligned), "{:?}", advice.now_steps);
        assert!(advice.slider_steps.iter().all(|s| s.aligned), "{:?}", advice.slider_steps);
    }

    #[test]
    fn tight_stop_is_not_aligned() {
        let mut input = live_busha();
        input.stop_loss_pct = 0.02;
        let advice = evaluate(&input);
        let stop = advice
            .slider_steps
            .iter()
            .find(|s| s.id == "stop_loss_pct")
            .unwrap();
        assert!(!stop.aligned);
        assert!(stop.detail.contains("1–2%") || stop.detail.contains("1-2%"));
    }

    #[test]
    fn do_not_raise_min_confidence_from_journal() {
        let advice = evaluate(&live_busha());
        assert!(advice.do_not.iter().any(|d| d.contains("min confidence")));
        let minc = advice
            .slider_steps
            .iter()
            .find(|s| s.id == "min_confidence_to_trade")
            .unwrap();
        assert!(minc.detail.to_lowercase().contains("leave"));
        assert!(in_range(0.65, 0.65, 0.70));
        assert!(!in_range(0.80, 0.65, 0.70));
    }

    #[test]
    fn paused_busha_still_counts_as_live_book() {
        let mut input = live_busha();
        input.trading_mode = "paused".into();
        assert!(is_live_crypto(&input));
        assert!(evaluate(&input).applicable);
    }
}
