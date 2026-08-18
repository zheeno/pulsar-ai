use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::app_state::AppState;
use crate::db::Database;
use crate::execution::{remaining_buy_budget, BAMBOO_MIN_ORDER_NOTIONAL};
use crate::memory;
use crate::settings::get_settings;

const EPS: f64 = 1e-9;
const TAKE_PROFIT_DISPLAY_DEFAULT: f64 = 0.10;
const HISTORY_TURNS: usize = 8;

/// Settings slider bounds (tighter than some schema maxes).
pub fn slider_bounds(field: &str) -> Option<(f64, f64)> {
    match field {
        "max_position_pct" => Some((0.01, 0.50)),
        "cycle_budget_pct" => Some((0.05, 1.0)),
        "min_confidence_to_trade" => Some((0.40, 0.95)),
        "max_daily_drawdown_pct" => Some((0.01, 1.0)),
        "stop_loss_pct" => Some((0.01, 0.25)),
        "take_profit_pct" => Some((0.02, 0.40)),
        "time_stop_hours" => Some((0.0, 168.0)),
        "partial_tp_fraction" => Some((0.1, 1.0)),
        _ => None,
    }
}

pub fn field_label(field: &str) -> &'static str {
    match field {
        "max_position_pct" => "Max position",
        "cycle_budget_pct" => "Cycle cash budget",
        "min_confidence_to_trade" => "Min confidence",
        "max_daily_drawdown_pct" => "Max daily drawdown",
        "stop_loss_pct" => "Stop loss",
        "take_profit_pct" => "Take profit",
        "time_stop_hours" => "Time-stop hours",
        "partial_tp_fraction" => "Partial take-profit",
        _ => "Parameter",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyParams {
    pub max_position_pct: f64,
    pub stop_loss_pct: f64,
    pub take_profit_pct: Option<f64>,
    pub min_confidence_to_trade: f64,
    pub max_daily_drawdown_pct: f64,
    pub cycle_budget_pct: f64,
    /// Hours a lot may stay in-band before a time-stop SELL. 0 = off.
    #[serde(default = "default_time_stop_hours")]
    pub time_stop_hours: f64,
    /// Fraction of the lot to sell on take-profit (1.0 = full lot).
    #[serde(default = "default_partial_tp_fraction")]
    pub partial_tp_fraction: f64,
}

fn default_time_stop_hours() -> f64 {
    24.0
}

fn default_partial_tp_fraction() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StrategyPatch {
    #[serde(default, alias = "maxPositionPct")]
    pub max_position_pct: Option<f64>,
    #[serde(default, alias = "cycleBudgetPct")]
    pub cycle_budget_pct: Option<f64>,
    #[serde(default, alias = "minConfidenceToTrade")]
    pub min_confidence_to_trade: Option<f64>,
    #[serde(default, alias = "maxDailyDrawdownPct")]
    pub max_daily_drawdown_pct: Option<f64>,
    #[serde(default, alias = "stopLossPct")]
    pub stop_loss_pct: Option<f64>,
    #[serde(default, alias = "takeProfitPct")]
    pub take_profit_pct: Option<f64>,
}

impl StrategyPatch {
    pub fn from_map(map: &BTreeMap<String, f64>) -> Self {
        Self {
            max_position_pct: map.get("max_position_pct").copied(),
            cycle_budget_pct: map.get("cycle_budget_pct").copied(),
            min_confidence_to_trade: map.get("min_confidence_to_trade").copied(),
            max_daily_drawdown_pct: map.get("max_daily_drawdown_pct").copied(),
            stop_loss_pct: map.get("stop_loss_pct").copied(),
            take_profit_pct: map.get("take_profit_pct").copied(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.max_position_pct.is_none()
            && self.cycle_budget_pct.is_none()
            && self.min_confidence_to_trade.is_none()
            && self.max_daily_drawdown_pct.is_none()
            && self.stop_loss_pct.is_none()
            && self.take_profit_pct.is_none()
    }

    pub fn entries(&self) -> Vec<(&'static str, f64)> {
        let mut out = Vec::new();
        if let Some(v) = self.max_position_pct {
            out.push(("max_position_pct", v));
        }
        if let Some(v) = self.cycle_budget_pct {
            out.push(("cycle_budget_pct", v));
        }
        if let Some(v) = self.min_confidence_to_trade {
            out.push(("min_confidence_to_trade", v));
        }
        if let Some(v) = self.max_daily_drawdown_pct {
            out.push(("max_daily_drawdown_pct", v));
        }
        if let Some(v) = self.stop_loss_pct {
            out.push(("stop_loss_pct", v));
        }
        if let Some(v) = self.take_profit_pct {
            out.push(("take_profit_pct", v));
        }
        out
    }
}

pub fn clamp_field(field: &str, value: f64) -> f64 {
    match slider_bounds(field) {
        Some((min, max)) => value.clamp(min, max),
        None => value,
    }
}

pub fn clamp_patch(patch: &StrategyPatch) -> StrategyPatch {
    StrategyPatch {
        max_position_pct: patch
            .max_position_pct
            .map(|v| clamp_field("max_position_pct", v)),
        cycle_budget_pct: patch
            .cycle_budget_pct
            .map(|v| clamp_field("cycle_budget_pct", v)),
        min_confidence_to_trade: patch
            .min_confidence_to_trade
            .map(|v| clamp_field("min_confidence_to_trade", v)),
        max_daily_drawdown_pct: patch
            .max_daily_drawdown_pct
            .map(|v| clamp_field("max_daily_drawdown_pct", v)),
        stop_loss_pct: patch.stop_loss_pct.map(|v| clamp_field("stop_loss_pct", v)),
        take_profit_pct: patch
            .take_profit_pct
            .map(|v| clamp_field("take_profit_pct", v)),
    }
}

fn current_ratio(current: &StrategyParams, field: &str) -> f64 {
    match field {
        "max_position_pct" => current.max_position_pct,
        "cycle_budget_pct" => current.cycle_budget_pct,
        "min_confidence_to_trade" => current.min_confidence_to_trade,
        "max_daily_drawdown_pct" => current.max_daily_drawdown_pct,
        "stop_loss_pct" => current.stop_loss_pct,
        "take_profit_pct" => current
            .take_profit_pct
            .unwrap_or(TAKE_PROFIT_DISPLAY_DEFAULT),
        _ => 0.0,
    }
}

pub fn drop_unchanged(patch: &StrategyPatch, current: &StrategyParams) -> StrategyPatch {
    let keep = |field: &str, proposed: Option<f64>| {
        proposed.filter(|v| (*v - current_ratio(current, field)).abs() >= EPS)
    };
    StrategyPatch {
        max_position_pct: keep("max_position_pct", patch.max_position_pct),
        cycle_budget_pct: keep("cycle_budget_pct", patch.cycle_budget_pct),
        min_confidence_to_trade: keep("min_confidence_to_trade", patch.min_confidence_to_trade),
        max_daily_drawdown_pct: keep("max_daily_drawdown_pct", patch.max_daily_drawdown_pct),
        stop_loss_pct: keep("stop_loss_pct", patch.stop_loss_pct),
        take_profit_pct: keep("take_profit_pct", patch.take_profit_pct),
    }
}

pub fn merge_patch(current: &StrategyParams, patch: &StrategyPatch) -> StrategyParams {
    StrategyParams {
        max_position_pct: patch.max_position_pct.unwrap_or(current.max_position_pct),
        stop_loss_pct: patch.stop_loss_pct.unwrap_or(current.stop_loss_pct),
        take_profit_pct: match patch.take_profit_pct {
            Some(v) => Some(v),
            None => current.take_profit_pct,
        },
        min_confidence_to_trade: patch
            .min_confidence_to_trade
            .unwrap_or(current.min_confidence_to_trade),
        max_daily_drawdown_pct: patch
            .max_daily_drawdown_pct
            .unwrap_or(current.max_daily_drawdown_pct),
        cycle_budget_pct: patch.cycle_budget_pct.unwrap_or(current.cycle_budget_pct),
        time_stop_hours: current.time_stop_hours,
        partial_tp_fraction: current.partial_tp_fraction,
    }
}

pub fn validate_ratio(name: &str, value: f64) -> Result<(), String> {
    if !(0.0..=1.0).contains(&value) {
        return Err(format!("{name} must be between 0 and 1"));
    }
    Ok(())
}

/// Same checks as Settings `update_strategy`.
pub fn validate_strategy_params(strategy: &StrategyParams) -> Result<(), String> {
    validate_ratio("maxPositionPct", strategy.max_position_pct)?;
    validate_ratio("stopLossPct", strategy.stop_loss_pct)?;
    validate_ratio("minConfidenceToTrade", strategy.min_confidence_to_trade)?;
    validate_ratio("maxDailyDrawdownPct", strategy.max_daily_drawdown_pct)?;
    if !(0.05..=1.0).contains(&strategy.cycle_budget_pct) {
        return Err("cycleBudgetPct must be between 0.05 and 1.0".into());
    }
    if let Some(tp) = strategy.take_profit_pct {
        validate_ratio("takeProfitPct", tp)?;
    }
    if !(0.0..=168.0).contains(&strategy.time_stop_hours) || !strategy.time_stop_hours.is_finite() {
        return Err("timeStopHours must be between 0 and 168".into());
    }
    if !(0.1..=1.0).contains(&strategy.partial_tp_fraction) || !strategy.partial_tp_fraction.is_finite()
    {
        return Err("partialTpFraction must be between 0.1 and 1.0".into());
    }
    Ok(())
}

pub fn persist_strategy_params(conn: &Connection, strategy: &StrategyParams) -> Result<()> {
    let updated = conn.execute(
        "UPDATE strategy_param_sets SET
            max_position_pct = ?1,
            stop_loss_pct = ?2,
            take_profit_pct = ?3,
            min_confidence_to_trade = ?4,
            max_daily_drawdown_pct = ?5,
            cycle_budget_pct = ?6,
            time_stop_hours = ?7,
            partial_tp_fraction = ?8,
            allowed_symbols = NULL
         WHERE is_active = 1",
        rusqlite::params![
            strategy.max_position_pct,
            strategy.stop_loss_pct,
            strategy.take_profit_pct,
            strategy.min_confidence_to_trade,
            strategy.max_daily_drawdown_pct,
            strategy.cycle_budget_pct,
            strategy.time_stop_hours,
            strategy.partial_tp_fraction,
        ],
    )?;
    if updated == 0 {
        anyhow::bail!("No active strategy param set found");
    }
    Ok(())
}

pub fn apply_strategy_params(db: &Database, strategy: &StrategyParams) -> Result<(), String> {
    validate_strategy_params(strategy)?;
    db.with_conn(|conn| persist_strategy_params(conn, strategy))
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone)]
pub struct StrategyRow {
    pub id: String,
    pub name: String,
    pub max_daily_trades: i64,
    pub position_size_pct: f64,
    pub is_active: bool,
    pub params: StrategyParams,
}

pub fn read_active_strategy(conn: &Connection) -> Result<StrategyRow> {
    Ok(conn.query_row(
        "SELECT id, name, max_position_pct, max_daily_trades, stop_loss_pct, take_profit_pct,
                min_confidence_to_trade, max_daily_drawdown_pct, position_size_pct,
                cycle_budget_pct, is_active, time_stop_hours, partial_tp_fraction
         FROM strategy_param_sets WHERE is_active = 1 LIMIT 1",
        [],
        |row| {
            Ok(StrategyRow {
                id: row.get(0)?,
                name: row.get(1)?,
                max_daily_trades: row.get(3)?,
                position_size_pct: row.get(8)?,
                is_active: row.get::<_, i64>(10)? == 1,
                params: StrategyParams {
                    max_position_pct: row.get(2)?,
                    stop_loss_pct: row.get(4)?,
                    take_profit_pct: row.get(5)?,
                    min_confidence_to_trade: row.get(6)?,
                    max_daily_drawdown_pct: row.get(7)?,
                    cycle_budget_pct: row.get(9)?,
                    time_stop_hours: row.get::<_, Option<f64>>(11)?.unwrap_or(24.0),
                    partial_tp_fraction: row.get::<_, Option<f64>>(12)?.unwrap_or(1.0),
                },
            })
        },
    )?)
}

pub fn read_active_strategy_json(conn: &Connection) -> Result<Value> {
    let row = read_active_strategy(conn)?;
    Ok(strategy_row_json(&row))
}

fn strategy_row_json(row: &StrategyRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "max_position_pct": row.params.max_position_pct,
        "max_daily_trades": row.max_daily_trades,
        "stop_loss_pct": row.params.stop_loss_pct,
        "take_profit_pct": row.params.take_profit_pct,
        "min_confidence_to_trade": row.params.min_confidence_to_trade,
        "max_daily_drawdown_pct": row.params.max_daily_drawdown_pct,
        "position_size_pct": row.position_size_pct,
        "cycle_budget_pct": row.params.cycle_budget_pct,
        "time_stop_hours": row.params.time_stop_hours,
        "partial_tp_fraction": row.params.partial_tp_fraction,
        "is_active": row.is_active,
    })
}

pub fn load_recent_trades(conn: &Connection, limit: i64) -> Result<Vec<Value>> {
    let limit = limit.max(1);
    let mut out = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT id, symbol, side, quantity, fill_price, simulated_fee, executed_at, resulting_cash_balance
         FROM sandbox_trades ORDER BY executed_at DESC LIMIT ?1",
    )?;
    for row in stmt.query_map([limit], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "symbol": row.get::<_, String>(1)?,
            "side": row.get::<_, String>(2)?,
            "quantity": row.get::<_, f64>(3)?,
            "fill_price": row.get::<_, f64>(4)?,
            "simulated_fee": row.get::<_, f64>(5)?,
            "executed_at": row.get::<_, String>(6)?,
            "resulting_cash_balance": row.get::<_, Option<f64>>(7)?,
            "venue": "sandbox",
            "status": "executed",
        }))
    })? {
        out.push(row?);
    }

    let mut stmt = conn.prepare(
        "SELECT id, symbol, side, quantity, fill_price, fee, created_at, status, rejection_reason
         FROM broker_orders ORDER BY created_at DESC LIMIT ?1",
    )?;
    for row in stmt.query_map([limit], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "symbol": row.get::<_, String>(1)?,
            "side": row.get::<_, String>(2)?,
            "quantity": row.get::<_, f64>(3)?,
            "fill_price": row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
            "simulated_fee": row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
            "executed_at": row.get::<_, String>(6)?,
            "resulting_cash_balance": null,
            "venue": "wealth",
            "status": row.get::<_, String>(7)?,
            "rejection_reason": row.get::<_, Option<String>>(8)?,
        }))
    })? {
        out.push(row?);
    }

    out.sort_by(|a, b| {
        let da = a.get("executed_at").and_then(|v| v.as_str()).unwrap_or("");
        let db = b.get("executed_at").and_then(|v| v.as_str()).unwrap_or("");
        db.cmp(da)
    });
    out.truncate(limit as usize);
    Ok(out)
}

/// Warn when a *proposed patch* is likely ineffective. Empty patch (chat) stays quiet.
pub fn bamboo_effectiveness_warnings(
    cash: f64,
    patch: &StrategyPatch,
    _current: &StrategyParams,
) -> Vec<String> {
    if patch.is_empty() {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    let buy_related = patch.max_position_pct.is_some()
        || patch.cycle_budget_pct.is_some()
        || patch.min_confidence_to_trade.is_some();
    if buy_related && cash + EPS < BAMBOO_MIN_ORDER_NOTIONAL {
        warnings.push(format!(
            "Cash is below ₦{:.0}; Bamboo cannot place a BUY.",
            BAMBOO_MIN_ORDER_NOTIONAL
        ));
    }
    if patch
        .cycle_budget_pct
        .is_some_and(|cycle| (cycle - 1.0).abs() < EPS)
    {
        warnings.push(
            "Cycle budget 100% means no extra cash cap beyond spendable cash.".into(),
        );
    }
    if patch
        .max_daily_drawdown_pct
        .is_some_and(|dd| (dd - 1.0).abs() < EPS)
    {
        warnings.push("Drawdown 100% means session-loss halt is off.".into());
    }
    warnings
}

fn is_current_state_nag(text: &str) -> bool {
    let l = text.to_lowercase();
    l.contains("cannot place a buy")
        || l.contains("no extra cash cap")
        || l.contains("session-loss halt is off")
}

#[derive(Debug, Clone)]
pub struct CoachProposal {
    pub need_more_context: bool,
    pub clarifying_questions: Vec<String>,
    pub summary: String,
    pub patch: StrategyPatch,
    pub rationale: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

pub fn proposal_from_llm(
    output: &Value,
    current: &StrategyParams,
) -> Result<CoachProposal, String> {
    let need_more_context = output
        .get("needMoreContext")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let clarifying_questions: Vec<String> = output
        .get("clarifyingQuestions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let summary = output
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if summary.is_empty() {
        return Err("Coach response was missing a summary".into());
    }
    let mut rationale = BTreeMap::new();
    if let Some(obj) = output.get("rationale").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(text) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                rationale.insert(k.clone(), text.to_string());
            }
        }
    }
    let warnings: Vec<String> = output
        .get("warnings")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut patch = serde_json::from_value(output.get("patch").cloned().unwrap_or(json!({})))
        .unwrap_or_default();
    patch = drop_unchanged(&clamp_patch(&patch), current);
    // A filled patch is a proposal. Do not throw it away just because the model also asked questions.
    let need_more_context = need_more_context && patch.is_empty();
    let clarifying_questions = if need_more_context {
        clarifying_questions
    } else {
        Vec::new()
    };

    Ok(CoachProposal {
        need_more_context,
        clarifying_questions,
        summary,
        patch,
        rationale,
        warnings,
    })
}

fn ratio_to_pct(ratio: f64) -> i64 {
    (ratio * 100.0).round() as i64
}

fn proposal_json(
    proposal: &CoachProposal,
    current: &StrategyParams,
    extra_warnings: Vec<String>,
) -> Value {
    let mut warnings = proposal.warnings.clone();
    for w in extra_warnings {
        if !warnings.iter().any(|existing| existing == &w) {
            warnings.push(w);
        }
    }
    let mut patch_obj = serde_json::Map::new();
    let mut rationale_obj = serde_json::Map::new();
    let mut diff = Vec::new();
    for (field, proposed) in proposal.patch.entries() {
        patch_obj.insert(field.to_string(), json!(proposed));
        let why = proposal
            .rationale
            .get(field)
            .cloned()
            .unwrap_or_default();
        if !why.is_empty() {
            rationale_obj.insert(field.to_string(), json!(why));
        }
        let current_val = current_ratio(current, field);
        diff.push(json!({
            "field": field,
            "label": field_label(field),
            "current": current_val,
            "proposed": proposed,
            "currentPct": ratio_to_pct(current_val),
            "proposedPct": ratio_to_pct(proposed),
            "rationale": why,
        }));
    }
    json!({
        "needMoreContext": proposal.need_more_context,
        "clarifyingQuestions": proposal.clarifying_questions,
        "summary": proposal.summary,
        "patch": patch_obj,
        "rationale": rationale_obj,
        "warnings": warnings,
        "current": {
            "max_position_pct": current.max_position_pct,
            "cycle_budget_pct": current.cycle_budget_pct,
            "min_confidence_to_trade": current.min_confidence_to_trade,
            "max_daily_drawdown_pct": current.max_daily_drawdown_pct,
            "stop_loss_pct": current.stop_loss_pct,
            "take_profit_pct": current.take_profit_pct.unwrap_or(TAKE_PROFIT_DISPLAY_DEFAULT),
        },
        "diff": diff,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoachChatTurn {
    pub role: String,
    pub content: String,
}

fn truncate(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= n {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(n).collect::<String>())
    }
}

fn snapshot_cash(portfolio: &Value) -> f64 {
    portfolio
        .pointer("/portfolio/cash_balance")
        .and_then(|v| v.as_f64())
        .or_else(|| portfolio.get("cash_balance").and_then(|v| v.as_f64()))
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Default)]
pub struct CoachIntent {
    pub defer: bool,
    pub wants_sl_tp: bool,
    pub drawdown_pct: Option<f64>,
    pub profit_pct: Option<f64>,
    pub already_asked: bool,
    pub greeting: bool,
    pub research: bool,
}

impl CoachIntent {
    /// Force a slider patch only when this turn is actually about changing Settings.
    pub fn should_decide(&self) -> bool {
        if self.greeting || self.research {
            return false;
        }
        self.defer || self.wants_sl_tp || self.drawdown_pct.is_some()
    }
}

fn looks_like_greeting(message: &str) -> bool {
    let t = message
        .trim()
        .trim_matches(|c: char| matches!(c, '!' | '.' | ','))
        .trim()
        .to_lowercase();
    matches!(
        t.as_str(),
        "hi" | "hey" | "hello" | "yo" | "sup" | "hiya" | "good morning" | "good afternoon"
            | "good evening" | "gm" | "howdy"
    ) || t.starts_with("hi ")
        || t.starts_with("hey ")
        || t.starts_with("hello ")
}

fn looks_like_research(message: &str) -> bool {
    let t = message.to_lowercase();
    const KEYS: &[&str] = &[
        "price",
        "quote",
        "moving",
        "movers",
        "gainer",
        "loser",
        "news",
        "headline",
        "history",
        "how is",
        "how's",
        "hows ",
        "rsi",
        "sma",
        "indicator",
        "chart",
        "current price",
        "last price",
        "big mover",
        "what's moving",
        "whats moving",
        "what is moving",
        "tape",
        "universe",
    ];
    KEYS.iter().any(|k| t.contains(k))
}

fn looks_like_change_request(message: &str) -> bool {
    const KEYS: &[&str] = &[
        "tighten",
        "loosen",
        "increase",
        "decrease",
        "set ",
        "change",
        "adjust",
        "update",
        "you decide",
        "your call",
        "you choose",
        "you pick",
        "you should decide",
        "appropriate response",
    ];
    KEYS.iter().any(|k| message.contains(k))
}

fn extract_coach_intent(message: &str, history: &[CoachChatTurn]) -> CoachIntent {
    let current = message.to_lowercase();
    let greeting = looks_like_greeting(message);
    let research = !greeting && looks_like_research(message);
    // Strategy signals come from this user turn only — never from assistant recaps of current sliders.
    let defer = [
        "you decide",
        "your call",
        "you should decide",
        "appropriate response",
        "i don't have a strategy",
        "i dont have a strategy",
        "where you come in",
        "that's on you",
        "thats on you",
        "you choose",
        "you pick",
    ]
    .iter()
    .any(|p| current.contains(p));
    let mentions_sl_tp = current.contains("stop loss")
        || current.contains("stop-loss")
        || current.contains("take profit")
        || current.contains("take-profit");
    let wants_sl_tp = mentions_sl_tp && (looks_like_change_request(&current) || defer);
    let already_asked = history.iter().any(|t| {
        t.role.eq_ignore_ascii_case("assistant")
            && (t.content.contains('?') || t.content.to_lowercase().contains("need more"))
    });
    CoachIntent {
        defer,
        wants_sl_tp,
        drawdown_pct: if greeting || research {
            None
        } else {
            parse_pct_near(
                &current,
                &["drop", "drawdown", "tolerate", "tolerance", "loss", "dd"],
            )
        },
        profit_pct: if greeting || research {
            None
        } else {
            parse_pct_near(&current, &["profit", "gain", "target", "return"])
        },
        already_asked,
        greeting,
        research,
    }
}

fn parse_pct_near(blob: &str, keywords: &[&str]) -> Option<f64> {
    let bytes = blob.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let Some(num) = blob.get(start..i).and_then(|s| s.parse::<f64>().ok()) else {
                continue;
            };
            let rest_trim = blob.get(i..).unwrap_or("").trim_start();
            let is_pct = rest_trim.starts_with('%')
                || rest_trim.starts_with("percent")
                || rest_trim.starts_with("per cent");
            if is_pct && num > 0.0 && num <= 100.0 {
                let window_start = start.saturating_sub(48);
                let window_end = (i + 24).min(blob.len());
                let window = blob.get(window_start..window_end).unwrap_or("");
                if keywords.iter().any(|k| window.contains(k)) {
                    return Some(num / 100.0);
                }
            }
            continue;
        }
        i += 1;
    }
    None
}

pub fn coach_decide_patch(current: &StrategyParams, intent: &CoachIntent) -> StrategyPatch {
    let mut patch = StrategyPatch::default();
    if let Some(dd) = intent.drawdown_pct {
        patch.max_daily_drawdown_pct = Some(clamp_field("max_daily_drawdown_pct", dd));
    }
    if intent.wants_sl_tp || intent.defer || intent.drawdown_pct.is_some() {
        let sl = intent
            .drawdown_pct
            .map(|d| (d * 0.65).clamp(0.04, 0.15))
            .unwrap_or(0.08);
        patch.stop_loss_pct = Some(clamp_field("stop_loss_pct", sl));
        let mut tp = intent.profit_pct.unwrap_or(0.18).clamp(0.12, 0.30);
        if tp + 1e-9 < sl + 0.04 {
            tp = (sl + 0.08).min(0.40);
        }
        patch.take_profit_pct = Some(clamp_field("take_profit_pct", tp));
    }
    drop_unchanged(&clamp_patch(&patch), current)
}

fn forced_decision_summary(intent: &CoachIntent, patch: &StrategyPatch) -> String {
    let mut bits = Vec::new();
    if intent.drawdown_pct.is_some() {
        bits.push("I used your stated drop tolerance for max daily drawdown and a stop-loss inside that band");
    }
    if intent.profit_pct.is_some() {
        bits.push("a daily account profit target is not a slider and is not realistic here, so I mapped it to position take-profit");
    }
    if intent.defer {
        bits.push("you asked me to decide, so this is a concrete slider patch");
    }
    if bits.is_empty() && !patch.is_empty() {
        bits.push("here is a concrete slider patch from what you already said");
    }
    format!(
        "{}. Confirm the diff to save — nothing has been applied yet.",
        bits.join("; ")
    )
}

pub fn apply_forced_decision(
    proposal: &mut CoachProposal,
    current: &StrategyParams,
    intent: &CoachIntent,
) {
    if intent.greeting || intent.research {
        proposal.patch = StrategyPatch::default();
        proposal.rationale.clear();
        return;
    }
    if !intent.should_decide() {
        return;
    }
    if proposal.patch.is_empty() {
        proposal.patch = coach_decide_patch(current, intent);
        if !proposal.patch.is_empty() {
            if proposal.need_more_context || proposal.summary.trim().is_empty() {
                proposal.summary = forced_decision_summary(intent, &proposal.patch);
            }
            for (field, _) in proposal.patch.entries() {
                proposal.rationale.entry(field.to_string()).or_insert_with(|| {
                    match field {
                        "max_daily_drawdown_pct" => {
                            "Matches the drop you said you can tolerate.".into()
                        }
                        "stop_loss_pct" => {
                            "Stop inside your drawdown band so one name cannot exhaust the daily cap.".into()
                        }
                        "take_profit_pct" => {
                            "Position take-profit. A daily account profit target is not a slider.".into()
                        }
                        _ => "Chosen from the constraints you already stated.".into(),
                    }
                });
            }
        }
    }
    if !proposal.patch.is_empty() {
        proposal.need_more_context = false;
        proposal.clarifying_questions.clear();
    }
}

fn gather_context(
    state: &Arc<AppState>,
    message: &str,
    history: &[CoachChatTurn],
) -> Result<(StrategyRow, Value, Vec<String>), String> {
    let mut warnings = Vec::new();
    let row = state
        .db
        .with_conn(read_active_strategy)
        .map_err(|e| e.to_string())?;
    let params = &row.params;

    let portfolio = crate::commands::load_portfolio_default_sync(state)?;
    if portfolio
        .get("stale")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        warnings.push("Live book refresh failed; using cached broker book.".into());
    }
    if let Some(err) = portfolio.get("wealthError").and_then(|v| v.as_str()) {
        warnings.push(format!("Live book unavailable ({err}); using sandbox figures."));
    }

    let cash = snapshot_cash(&portfolio);
    let equity = portfolio
        .get("total_equity")
        .and_then(|v| v.as_f64())
        .unwrap_or(cash);
    let market_value = portfolio
        .get("market_value")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let trading_mode = portfolio
        .get("tradingMode")
        .and_then(|v| v.as_str())
        .unwrap_or("sandbox");
    let broker_id = portfolio
        .get("brokerId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let min_notional = crate::execution::min_order_notional_for_venue(broker_id);
    let remaining = remaining_buy_budget(cash, params.cycle_budget_pct, min_notional);

    let trades = state
        .db
        .with_conn(|conn| load_recent_trades(conn, 20))
        .unwrap_or_default();
    let memories = state
        .db
        .with_conn(|conn| memory::keyword_search(conn, "strategy", None, 6))
        .unwrap_or_default();

    let intent = extract_coach_intent(message, history);
    let holdings = portfolio.get("positions").cloned().unwrap_or(json!([]));
    let context = json!({
        "conversation": {
            "message": message,
            "history": history.iter().rev().take(HISTORY_TURNS).collect::<Vec<_>>().into_iter().rev().map(|t| {
                json!({ "role": t.role, "content": t.content })
            }).collect::<Vec<_>>(),
        },
        "coachIntent": {
            "defer": intent.defer,
            "shouldDecide": intent.should_decide(),
            "wantsStopTakeProfit": intent.wants_sl_tp,
            "statedDrawdownPct": intent.drawdown_pct,
            "statedProfitPct": intent.profit_pct,
            "alreadyAskedQuestions": intent.already_asked,
            "greeting": intent.greeting,
            "research": intent.research,
        },
        "account": {
            "tradingMode": trading_mode,
            "brokerId": broker_id,
            "brokerName": portfolio.get("brokerName"),
            "cash": cash,
            "equity": equity,
            "marketValue": market_value,
            "pnlToday": portfolio.get("pnl_today").cloned().or_else(|| portfolio.get("pnlToday").cloned()),
        },
        "holdings": holdings,
        "currentParams": {
            "max_position_pct": params.max_position_pct,
            "cycle_budget_pct": params.cycle_budget_pct,
            "min_confidence_to_trade": params.min_confidence_to_trade,
            "max_daily_drawdown_pct": params.max_daily_drawdown_pct,
            "stop_loss_pct": params.stop_loss_pct,
            "take_profit_pct": params.take_profit_pct.unwrap_or(TAKE_PROFIT_DISPLAY_DEFAULT),
        },
        "recentTrades": trades,
        "strategyMemories": memories.iter().map(|m| json!({
            "kind": m.kind,
            "text": m.text,
            "source": m.source,
            "createdAt": m.created_at,
        })).collect::<Vec<_>>(),
        "facts": {
            "BAMBOO_MIN_ORDER_NOTIONAL": BAMBOO_MIN_ORDER_NOTIONAL,
            "remainingBuyBudget": remaining,
            "remainingBuyBudgetNote": "Per-cycle share of CURRENT spendable cash. Fills already reduced the wallet — do not subtract today's fills again.",
            "sliderBounds": {
                "max_position_pct": [0.01, 0.50],
                "cycle_budget_pct": [0.05, 1.0],
                "min_confidence_to_trade": [0.40, 0.95],
                "max_daily_drawdown_pct": [0.01, 1.0],
                "stop_loss_pct": [0.01, 0.25],
                "take_profit_pct": [0.02, 0.40],
            },
            "balancedDefaults": {
                "max_position_pct": 0.10,
                "cycle_budget_pct": 0.20,
                "min_confidence_to_trade": 0.65,
                "max_daily_drawdown_pct": 0.03,
                "stop_loss_pct": 0.05,
                "take_profit_pct": 0.10,
            },
        },
        "contextWarnings": warnings,
    });
    Ok((row, context, warnings))
}

#[tauri::command]
pub async fn strategy_coach_propose(
    message: String,
    history: Option<Vec<CoachChatTurn>>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let message = message.trim().to_string();
        if message.is_empty() {
            return Err("Message is required".into());
        }
        let history = history.unwrap_or_default();
        let (row, context, book_warnings) = gather_context(&state, &message, &history)?;
        let cash = context
            .pointer("/account/cash")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let traces = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent_data = state
            .agent
            .strategy_coach_sync(&settings, context, &state.db, Some(traces.clone()))
            .map_err(|e| e.to_string())?;
        let output = agent_data
            .get("output")
            .cloned()
            .unwrap_or(agent_data.clone());
        let mut proposal = proposal_from_llm(&output, &row.params)?;
        let intent = extract_coach_intent(&message, &history);
        apply_forced_decision(&mut proposal, &row.params, &intent);
        if proposal.patch.is_empty() {
            proposal.warnings.retain(|w| !is_current_state_nag(w));
        } else {
            proposal.warnings.extend(bamboo_effectiveness_warnings(
                cash,
                &proposal.patch,
                &row.params,
            ));
            proposal.warnings.extend(book_warnings);
        }
        let mut body = proposal_json(&proposal, &row.params, Vec::new());
        if let Ok(g) = traces.lock() {
            if let Some(obj) = body.as_object_mut() {
                obj.insert("toolTrace".into(), json!(g.clone()));
            }
        }
        Ok(body)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn strategy_coach_apply(
    selected: BTreeMap<String, f64>,
    rationale: Option<BTreeMap<String, String>>,
    summary: Option<String>,
    chat_excerpt: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    if selected.is_empty() {
        return Err("Select at least one parameter to apply".into());
    }
    let unknown: Vec<_> = selected
        .keys()
        .filter(|k| slider_bounds(k).is_none())
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(format!("Unknown strategy fields: {}", unknown.join(", ")));
    }

    let rationale = rationale.unwrap_or_default();
    let excerpt = chat_excerpt.unwrap_or_default();
    let summary = summary.unwrap_or_default();

    let saved = state
        .db
        .with_conn(|conn| {
            let current = read_active_strategy(conn)?;
            let previous = current.params.clone();
            let patch = drop_unchanged(
                &clamp_patch(&StrategyPatch::from_map(&selected)),
                &previous,
            );
            if patch.is_empty() {
                anyhow::bail!("Selected values match the current strategy; nothing to save");
            }
            let merged = merge_patch(&previous, &patch);
            validate_strategy_params(&merged).map_err(|e| anyhow::anyhow!(e))?;
            let tx = conn.unchecked_transaction()?;
            persist_strategy_params(&tx, &merged)?;
            let audit = format_audit(&previous, &merged, &patch, &rationale, &summary, &excerpt);
            memory::insert_memory(&tx, "freeform", None, &audit, "agent_upsert", None)?;
            tx.commit()?;
            read_active_strategy_json(conn)
        })
        .map_err(|e| e.to_string())?;

    Ok(json!({
        "ok": true,
        "strategy": saved,
        "message": "Strategy parameters were saved. Settings sliders now show the new values.",
    }))
}

fn format_audit(
    previous: &StrategyParams,
    merged: &StrategyParams,
    patch: &StrategyPatch,
    rationale: &BTreeMap<String, String>,
    summary: &str,
    excerpt: &str,
) -> String {
    let mut lines = vec!["Strategy coach applied:".to_string()];
    for (field, new_val) in patch.entries() {
        let old_val = current_ratio(previous, field);
        let why = rationale.get(field).map(|s| s.as_str()).unwrap_or("");
        let line = if why.is_empty() {
            format!(
                "- {field}: {:.4} → {:.4}",
                old_val, new_val
            )
        } else {
            format!(
                "- {field}: {:.4} → {:.4} ({})",
                old_val,
                new_val,
                truncate(why, 160)
            )
        };
        lines.push(line);
        let _ = merged;
    }
    if !summary.trim().is_empty() {
        lines.push(format!("Summary: {}", truncate(summary, 240)));
    }
    if !excerpt.trim().is_empty() {
        lines.push(format!("Chat: {}", truncate(excerpt, 400)));
    }
    lines.join("\n")
}

#[tauri::command]
pub fn coach_list_sessions(state: State<'_, Arc<AppState>>) -> Result<Vec<Value>, String> {
    state
        .db
        .with_conn(crate::coach::list_sessions)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn coach_new_session(state: State<'_, Arc<AppState>>) -> Result<Value, String> {
    state
        .db
        .with_conn(crate::coach::create_session)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn coach_delete_session(id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state
        .db
        .with_conn(|conn| crate::coach::delete_session(conn, &id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn coach_get_session(
    id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    state
        .db
        .with_conn(|conn| {
            let sid = match id.as_deref().filter(|s| !s.is_empty()) {
                Some(s) => s.to_string(),
                None => match crate::coach::latest_session_id(conn)? {
                    Some(s) => s,
                    None => {
                        return crate::coach::create_session(conn);
                    }
                },
            };
            crate::coach::load_session(conn, &sid)
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn coach_turn(
    session_id: String,
    message: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let message = message.trim().to_string();
        if message.is_empty() {
            return Err("Message is required".into());
        }
        let session_id = session_id.trim().to_string();
        if session_id.is_empty() {
            return Err("sessionId is required".into());
        }

        let history: Vec<CoachChatTurn> = state
            .db
            .with_conn(|conn| {
                crate::coach::append_message(conn, &session_id, "user", &message, None)?;
                let session = crate::coach::load_session(conn, &session_id)?;
                let mut turns = Vec::new();
                if let Some(arr) = session.get("messages").and_then(|v| v.as_array()) {
                    for m in arr {
                        let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                        let text = m.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        if role == "user" || role == "assistant" {
                            turns.push(CoachChatTurn {
                                role: role.into(),
                                content: text.into(),
                            });
                        }
                    }
                }
                Ok::<_, anyhow::Error>(turns)
            })
            .map_err(|e| e.to_string())?;
        // Drop the user message we just appended from history passed as "history"
        // gather_context uses history + current message separately.
        let history: Vec<CoachChatTurn> = history
            .into_iter()
            .rev()
            .skip(1)
            .take(HISTORY_TURNS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let (row, mut context, book_warnings) = gather_context(&state, &message, &history)?;
        if let Some(obj) = context.as_object_mut() {
            obj.insert("sessionId".into(), json!(session_id));
            obj.insert(
                "coachCapabilities".into(),
                json!([
                    "get_account_snapshot",
                    "get_holdings",
                    "get_recent_trades",
                    "get_strategy_params",
                    "list_universe_quotes",
                    "get_symbol_quote",
                    "get_price_history",
                    "get_indicators",
                    "search_memory",
                    "get_news",
                    "propose_strategy_patch",
                    "propose_trade"
                ]),
            );
        }
        let cash = context
            .pointer("/account/cash")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let traces = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let agent_data = crate::coach::with_active_session(&session_id, || {
            state.agent.strategy_coach_sync(
                &settings,
                context,
                &state.db,
                Some(traces.clone()),
            )
        })
        .map_err(|e| e.to_string())?;
        let output = agent_data
            .get("output")
            .cloned()
            .unwrap_or(agent_data.clone());
        let mut proposal = proposal_from_llm(&output, &row.params)?;
        let intent = extract_coach_intent(&message, &history);
        apply_forced_decision(&mut proposal, &row.params, &intent);
        if proposal.patch.is_empty() {
            proposal.warnings.retain(|w| !is_current_state_nag(w));
        } else {
            proposal.warnings.extend(bamboo_effectiveness_warnings(
                cash,
                &proposal.patch,
                &row.params,
            ));
            proposal.warnings.extend(book_warnings);
        }
        let mut body = proposal_json(&proposal, &row.params, Vec::new());
        let tool_trace = traces.lock().map(|g| g.clone()).unwrap_or_default();
        let trade = tool_trace.iter().rev().find_map(|t| {
            if t.get("name").and_then(|v| v.as_str()) == Some("propose_trade") {
                state
                    .db
                    .with_conn(|conn| {
                        // latest proposed for this session
                        conn.query_row(
                            "SELECT id FROM coach_trade_proposals
                             WHERE session_id = ?1 AND status = 'proposed'
                             ORDER BY created_at DESC LIMIT 1",
                            [&session_id],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(anyhow::Error::from)
                    })
                    .ok()
                    .flatten()
                    .and_then(|pid| {
                        state
                            .db
                            .with_conn(|conn| crate::coach::load_proposal(conn, &pid))
                            .ok()
                            .flatten()
                    })
            } else {
                None
            }
        });
        if let Some(obj) = body.as_object_mut() {
            obj.insert("toolTrace".into(), json!(tool_trace));
            obj.insert("trade".into(), json!(trade));
            obj.insert("status".into(), json!("done"));
        }
        let payload = json!({
            "proposal": body.get("diff").cloned(),
            "needMoreContext": body.get("needMoreContext"),
            "clarifyingQuestions": body.get("clarifyingQuestions"),
            "warnings": body.get("warnings"),
            "patch": body.get("patch"),
            "rationale": body.get("rationale"),
            "current": body.get("current"),
            "diff": body.get("diff"),
            "toolTrace": tool_trace,
            "trade": trade,
        });
        state
            .db
            .with_conn(|conn| {
                crate::coach::append_message(
                    conn,
                    &session_id,
                    "assistant",
                    proposal.summary.as_str(),
                    Some(&payload),
                )
            })
            .map_err(|e| e.to_string())?;
        if let Some(obj) = body.as_object_mut() {
            obj.insert("sessionId".into(), json!(session_id));
        }
        Ok(body)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn coach_execute_trade(
    proposal_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let settings = state.db.with_conn(get_settings).map_err(|e| e.to_string())?;
        let result = crate::coach::confirm_and_execute(&state.db, &settings, &proposal_id)
            .map_err(|e| e.to_string())?;
        let note = if result.get("ok") == Some(&json!(true)) {
            format!(
                "Trade confirmed: filled ({}).",
                result
                    .get("riskPolicyResult")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ok")
            )
        } else {
            format!(
                "Trade confirmed but not filled: {}.",
                result
                    .get("riskPolicyResult")
                    .and_then(|v| v.as_str())
                    .or_else(|| result.get("error").and_then(|v| v.as_str()))
                    .unwrap_or("blocked")
            )
        };
        let session_id: Option<String> = state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT session_id FROM coach_trade_proposals WHERE id = ?1",
                    [&proposal_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(anyhow::Error::from)
            })
            .ok()
            .flatten();
        if let Some(sid) = session_id {
            let payload = json!({
                "tradeResult": result,
                "proposalId": proposal_id,
            });
            let _ = state.db.with_conn(|conn| {
                crate::coach::append_message(conn, &sid, "assistant", &note, Some(&payload))
            });
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn coach_cancel_trade(
    proposal_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    crate::coach::cancel_proposal(&state.db, &proposal_id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample_current() -> StrategyParams {
        StrategyParams {
            max_position_pct: 0.10,
            stop_loss_pct: 0.05,
            take_profit_pct: Some(0.10),
            min_confidence_to_trade: 0.65,
            max_daily_drawdown_pct: 0.03,
            cycle_budget_pct: 0.20,
            time_stop_hours: 24.0,
            partial_tp_fraction: 1.0,
        }
    }

    fn setup_db() -> (std::path::PathBuf, Database) {
        let dir = std::env::temp_dir().join(format!("ngx-coach-{}", Uuid::new_v4()));
        let db = Database::open(&dir).unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO strategy_param_sets (
                    id, name, is_active, max_position_pct, stop_loss_pct, take_profit_pct,
                    min_confidence_to_trade, max_daily_drawdown_pct, cycle_budget_pct
                 ) VALUES (?1, 'default', 1, 0.10, 0.05, 0.10, 0.65, 0.03, 0.20)",
                rusqlite::params![Uuid::new_v4().to_string()],
            )?;
            Ok(())
        })
        .unwrap();
        (dir, db)
    }

    #[test]
    fn clamp_to_settings_slider_bounds() {
        assert!((clamp_field("stop_loss_pct", 0.50) - 0.25).abs() < EPS);
        assert!((clamp_field("min_confidence_to_trade", 0.20) - 0.40).abs() < EPS);
        assert!((clamp_field("max_position_pct", 0.90) - 0.50).abs() < EPS);
        assert!((clamp_field("cycle_budget_pct", 0.01) - 0.05).abs() < EPS);
        assert!((clamp_field("take_profit_pct", 0.01) - 0.02).abs() < EPS);
        let patch = clamp_patch(&StrategyPatch {
            stop_loss_pct: Some(0.50),
            min_confidence_to_trade: Some(0.20),
            ..StrategyPatch::default()
        });
        assert!((patch.stop_loss_pct.unwrap() - 0.25).abs() < EPS);
        assert!((patch.min_confidence_to_trade.unwrap() - 0.40).abs() < EPS);
    }

    #[test]
    fn merge_partial_patch_leaves_untouched_fields() {
        let current = sample_current();
        let merged = merge_patch(
            &current,
            &StrategyPatch {
                stop_loss_pct: Some(0.04),
                ..StrategyPatch::default()
            },
        );
        assert!((merged.stop_loss_pct - 0.04).abs() < EPS);
        assert!((merged.max_position_pct - 0.10).abs() < EPS);
        assert!((merged.cycle_budget_pct - 0.20).abs() < EPS);
        assert!((merged.min_confidence_to_trade - 0.65).abs() < EPS);
        assert!((merged.max_daily_drawdown_pct - 0.03).abs() < EPS);
        assert_eq!(merged.take_profit_pct, Some(0.10));
    }

    #[test]
    fn propose_path_does_not_update_strategy_param_sets() {
        let (dir, db) = setup_db();
        let before = db.with_conn(read_active_strategy).unwrap();
        let llm = json!({
            "needMoreContext": false,
            "clarifyingQuestions": [],
            "summary": "Tighten the stop after losses.",
            "patch": { "stop_loss_pct": 0.03 },
            "rationale": { "stop_loss_pct": "Cut losers sooner." },
            "warnings": []
        });
        let proposal = proposal_from_llm(&llm, &before.params).unwrap();
        assert!(proposal.patch.stop_loss_pct.is_some());
        let after = db.with_conn(read_active_strategy).unwrap();
        assert!((after.params.stop_loss_pct - before.params.stop_loss_pct).abs() < EPS);
        assert!((after.params.max_position_pct - before.params.max_position_pct).abs() < EPS);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn apply_writes_only_after_merge_and_validate() {
        let (dir, db) = setup_db();
        let current = db.with_conn(read_active_strategy).unwrap().params;
        let patch = drop_unchanged(
            &clamp_patch(&StrategyPatch {
                stop_loss_pct: Some(0.04),
                max_position_pct: Some(0.08),
                ..StrategyPatch::default()
            }),
            &current,
        );
        let merged = merge_patch(&current, &patch);
        apply_strategy_params(&db, &merged).unwrap();
        let after = db.with_conn(read_active_strategy).unwrap().params;
        assert!((after.stop_loss_pct - 0.04).abs() < EPS);
        assert!((after.max_position_pct - 0.08).abs() < EPS);
        assert!((after.cycle_budget_pct - 0.20).abs() < EPS);
        assert!((after.min_confidence_to_trade - 0.65).abs() < EPS);

        let invalid = StrategyParams {
            cycle_budget_pct: 0.01,
            ..merged
        };
        assert!(validate_strategy_params(&invalid).is_err());
        let still = db.with_conn(read_active_strategy).unwrap().params;
        assert!((still.cycle_budget_pct - 0.20).abs() < EPS);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bamboo_warning_when_cash_below_min_notional() {
        let current = sample_current();
        let chat = bamboo_effectiveness_warnings(4_000.0, &StrategyPatch::default(), &current);
        assert!(chat.is_empty());

        let buy_patch = StrategyPatch {
            cycle_budget_pct: Some(0.5),
            ..StrategyPatch::default()
        };
        let warnings = bamboo_effectiveness_warnings(4_000.0, &buy_patch, &current);
        assert!(warnings.iter().any(|w| w.contains("₦5000") || w.contains("₦5,000") || w.contains("5000")));
        let ok = bamboo_effectiveness_warnings(20_000.0, &buy_patch, &current);
        assert!(ok.iter().all(|w| !w.contains("cannot place a BUY")));
    }

    #[test]
    fn cycle_budget_full_warns_only_when_proposed() {
        let current = StrategyParams {
            cycle_budget_pct: 1.0,
            ..sample_current()
        };
        assert!(bamboo_effectiveness_warnings(20_000.0, &StrategyPatch::default(), &current).is_empty());
        let patch = StrategyPatch {
            cycle_budget_pct: Some(1.0),
            ..StrategyPatch::default()
        };
        let warnings = bamboo_effectiveness_warnings(20_000.0, &patch, &current);
        assert!(warnings.iter().any(|w| w.contains("100%")));
    }

    #[test]
    fn insufficient_context_empty_patch_and_questions() {
        let current = sample_current();
        let llm = json!({
            "needMoreContext": true,
            "clarifyingQuestions": ["What is your risk tolerance for this ₦ account?"],
            "summary": "I need a bit more to propose slider changes.",
            "patch": {},
            "rationale": {},
            "warnings": []
        });
        let proposal = proposal_from_llm(&llm, &current).unwrap();
        assert!(proposal.need_more_context);
        assert!(proposal.patch.is_empty());
        assert!(!proposal.clarifying_questions.is_empty());
    }

    #[test]
    fn llm_patch_wins_over_need_more_context_flag() {
        let current = sample_current();
        let llm = json!({
            "needMoreContext": true,
            "clarifyingQuestions": ["What stop loss?"],
            "summary": "I still have questions.",
            "patch": { "stop_loss_pct": 0.10 },
            "rationale": { "stop_loss_pct": "Give trades more room." },
            "warnings": []
        });
        let proposal = proposal_from_llm(&llm, &current).unwrap();
        assert!(!proposal.need_more_context);
        assert!(proposal.clarifying_questions.is_empty());
        assert!((proposal.patch.stop_loss_pct.unwrap() - 0.10).abs() < EPS);
    }

    #[test]
    fn extracts_drawdown_and_defer_then_forces_patch() {
        let current = sample_current();
        let history = vec![CoachChatTurn {
            role: "assistant".into(),
            content: "I need more information.\n- What is your risk tolerance?".into(),
        }];
        let msg = "I can tolerate a 15% drop in my investment and yes, I'm open to adjusting the stop loss and take profit settings. You should decide the appropriate response to all of these questions.";
        let intent = extract_coach_intent(msg, &history);
        assert!(intent.defer);
        assert!(intent.wants_sl_tp);
        assert!(intent.already_asked);
        assert!((intent.drawdown_pct.unwrap() - 0.15).abs() < 1e-9);
        assert!(intent.should_decide());

        let mut proposal = CoachProposal {
            need_more_context: true,
            clarifying_questions: vec!["What stop?".into()],
            summary: "I need more information.".into(),
            patch: StrategyPatch::default(),
            rationale: BTreeMap::new(),
            warnings: vec![],
        };
        apply_forced_decision(&mut proposal, &current, &intent);
        assert!(!proposal.need_more_context);
        assert!(proposal.clarifying_questions.is_empty());
        assert!(!proposal.patch.is_empty());
        assert!((proposal.patch.max_daily_drawdown_pct.unwrap() - 0.15).abs() < EPS);
        assert!(proposal.patch.stop_loss_pct.is_some());
        assert!(proposal.patch.take_profit_pct.is_some());
    }

    #[test]
    fn greeting_does_not_force_slider_patch() {
        let current = sample_current();
        let intent = extract_coach_intent("hi", &[]);
        assert!(intent.greeting);
        assert!(!intent.should_decide());
        let mut proposal = CoachProposal {
            need_more_context: false,
            clarifying_questions: vec![],
            summary: "Hey — what do you want to look at?".into(),
            patch: StrategyPatch::default(),
            rationale: BTreeMap::new(),
            warnings: vec![],
        };
        apply_forced_decision(&mut proposal, &current, &intent);
        assert!(proposal.patch.is_empty());
        assert!(proposal.summary.contains("Hey"));
    }

    #[test]
    fn price_question_not_poisoned_by_assistant_slider_recap() {
        let current = sample_current();
        let history = vec![
            CoachChatTurn {
                role: "user".into(),
                content: "hi".into(),
            },
            CoachChatTurn {
                role: "assistant".into(),
                content: "Hello! Cash ₦5485. Strategy: stop loss of 10% and take profit of 20%. Max daily drawdown of 5%. Need more?".into(),
            },
        ];
        let intent = extract_coach_intent("what's the current price for MTNN", &history);
        assert!(intent.research);
        assert!(!intent.should_decide());
        assert!(intent.already_asked);
        let mut proposal = CoachProposal {
            need_more_context: false,
            clarifying_questions: vec![],
            summary: "MTNN last ₦805 as-of 2026-08-18.".into(),
            patch: StrategyPatch::default(),
            rationale: BTreeMap::new(),
            warnings: vec![],
        };
        apply_forced_decision(&mut proposal, &current, &intent);
        assert!(proposal.patch.is_empty());
        assert!(proposal.summary.contains("MTNN"));
        assert!(!proposal.summary.contains("drop tolerance"));
    }

    #[test]
    fn movers_question_does_not_force_slider_patch() {
        let intent = extract_coach_intent("which stocks were the big movers today?", &[]);
        assert!(intent.research);
        assert!(!intent.should_decide());
        let intent_q = extract_coach_intent("?", &[]);
        assert!(!intent_q.should_decide());
    }

    #[test]
    fn research_strips_accidental_llm_patch() {
        let current = sample_current();
        let intent = extract_coach_intent("what's the current price for MTNN", &[]);
        let mut proposal = CoachProposal {
            need_more_context: false,
            clarifying_questions: vec![],
            summary: "MTNN last ₦805.".into(),
            patch: StrategyPatch {
                stop_loss_pct: Some(0.04),
                ..StrategyPatch::default()
            },
            rationale: BTreeMap::new(),
            warnings: vec![],
        };
        apply_forced_decision(&mut proposal, &current, &intent);
        assert!(proposal.patch.is_empty());
        assert_eq!(proposal.summary, "MTNN last ₦805.");
    }
}
