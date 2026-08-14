use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub pulse_supabase_url: Option<String>,
    pub pulse_supabase_anon_key: Option<String>,
    pub pulse_email: Option<String>,
    pub pulse_base_url: String,
    pub llm_provider: String,
    pub llm_model: String,
    pub llm_base_url: Option<String>,
    pub pulse_configured: bool,
    pub llm_configured: bool,
    pub onboarding_complete: bool,
    pub default_starting_capital: f64,
    pub simulated_slippage_bps: f64,
    pub simulated_fee_pct: f64,
    /// When true, the desktop scheduler runs full trading cycles on an interval.
    pub auto_cycle_enabled: bool,
    /// Minutes between automatic cycles (clamped to 5–120 when saved).
    pub auto_cycle_interval_minutes: u32,
    /// Live broker used for Home and order routing (`wealth` | `bamboo`).
    #[serde(default = "default_selected_broker")]
    pub selected_broker: String,
    /// Coronation Wealth email when connected.
    pub wealth_email: Option<String>,
    /// True when a Wealth session has been established.
    pub wealth_connected: bool,
    /// Bamboo phone when connected (retail login).
    pub bamboo_phone: Option<String>,
    /// True when a Bamboo session has been established.
    pub bamboo_connected: bool,
    /// User must opt in before any live order is submitted.
    pub live_trading_enabled: bool,
    /// Recurring authorization for scheduled live cycles.
    pub scheduled_live_authorized: bool,
    /// Max notional (NGN) per scheduled/manual live cycle.
    pub max_live_notional: f64,
    /// Max live actions per cycle (backend cap).
    pub max_live_actions: u32,
    /// When true, store encrypted raw LLM transcripts in the OS keychain-backed blob.
    pub retain_raw_llm_logs: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            pulse_supabase_url: None,
            pulse_supabase_anon_key: None,
            pulse_email: None,
            pulse_base_url: "https://ngxpulse.ng/api".into(),
            llm_provider: "openai".into(),
            llm_model: "gpt-4o-mini".into(),
            llm_base_url: None,
            pulse_configured: false,
            llm_configured: false,
            onboarding_complete: false,
            default_starting_capital: 10_000_000.0,
            simulated_slippage_bps: 10.0,
            simulated_fee_pct: 0.0015,
            auto_cycle_enabled: false,
            auto_cycle_interval_minutes: 30,
            selected_broker: "wealth".into(),
            wealth_email: None,
            wealth_connected: false,
            bamboo_phone: None,
            bamboo_connected: false,
            live_trading_enabled: false,
            scheduled_live_authorized: false,
            max_live_notional: 500_000.0,
            max_live_actions: 10,
            retain_raw_llm_logs: false,
        }
    }
}

pub fn get_settings(conn: &Connection) -> Result<AppSettings> {
    let mut settings = AppSettings::default();
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (key, value) = row?;
        match key.as_str() {
            "pulse_supabase_url" => settings.pulse_supabase_url = Some(value),
            "pulse_supabase_anon_key" => settings.pulse_supabase_anon_key = Some(value),
            "pulse_email" => settings.pulse_email = Some(value),
            "pulse_base_url" => settings.pulse_base_url = value,
            "llm_provider" => settings.llm_provider = value,
            "llm_model" => settings.llm_model = value,
            "llm_base_url" => settings.llm_base_url = Some(value),
            "pulse_configured" => settings.pulse_configured = value == "true",
            "llm_configured" => settings.llm_configured = value == "true",
            "onboarding_complete" => settings.onboarding_complete = value == "true",
            "default_starting_capital" => {
                settings.default_starting_capital = value.parse().unwrap_or(10_000_000.0)
            }
            "simulated_slippage_bps" => {
                settings.simulated_slippage_bps = value.parse().unwrap_or(10.0)
            }
            "simulated_fee_pct" => settings.simulated_fee_pct = value.parse().unwrap_or(0.0015),
            "auto_cycle_enabled" => settings.auto_cycle_enabled = value == "true",
            "auto_cycle_interval_minutes" => {
                settings.auto_cycle_interval_minutes =
                    value.parse::<u32>().unwrap_or(30).clamp(5, 120)
            }
            "selected_broker" => settings.selected_broker = value,
            "wealth_email" => settings.wealth_email = Some(value),
            "wealth_connected" => settings.wealth_connected = value == "true",
            "bamboo_phone" => settings.bamboo_phone = Some(value),
            "bamboo_connected" => settings.bamboo_connected = value == "true",
            "live_trading_enabled" => settings.live_trading_enabled = value == "true",
            "scheduled_live_authorized" => settings.scheduled_live_authorized = value == "true",
            "max_live_notional" => {
                settings.max_live_notional = value.parse().unwrap_or(500_000.0)
            }
            "max_live_actions" => {
                settings.max_live_actions = value.parse::<u32>().unwrap_or(10).clamp(1, 40)
            }
            "retain_raw_llm_logs" => settings.retain_raw_llm_logs = value == "true",
            _ => {}
        }
    }
    Ok(settings)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

pub fn save_settings(conn: &Connection, settings: &AppSettings) -> Result<()> {
    validate_numeric_settings(settings)?;
    if let Some(v) = &settings.pulse_supabase_url {
        set_setting(conn, "pulse_supabase_url", v)?;
    }
    if let Some(v) = &settings.pulse_supabase_anon_key {
        set_setting(conn, "pulse_supabase_anon_key", v)?;
    }
    if let Some(v) = &settings.pulse_email {
        set_setting(conn, "pulse_email", v)?;
    }
    set_setting(conn, "pulse_base_url", &settings.pulse_base_url)?;
    set_setting(conn, "llm_provider", &settings.llm_provider)?;
    set_setting(conn, "llm_model", &settings.llm_model)?;
    match settings.llm_base_url.as_deref().map(str::trim) {
        Some("") | None => {
            conn.execute("DELETE FROM settings WHERE key = 'llm_base_url'", [])?;
        }
        Some(v) => set_setting(conn, "llm_base_url", v)?,
    }
    set_setting(
        conn,
        "pulse_configured",
        if settings.pulse_configured { "true" } else { "false" },
    )?;
    set_setting(
        conn,
        "llm_configured",
        if settings.llm_configured { "true" } else { "false" },
    )?;
    set_setting(
        conn,
        "onboarding_complete",
        if settings.onboarding_complete {
            "true"
        } else {
            "false"
        },
    )?;
    set_setting(
        conn,
        "default_starting_capital",
        &settings.default_starting_capital.to_string(),
    )?;
    set_setting(
        conn,
        "simulated_slippage_bps",
        &settings.simulated_slippage_bps.to_string(),
    )?;
    set_setting(
        conn,
        "simulated_fee_pct",
        &settings.simulated_fee_pct.to_string(),
    )?;
    set_setting(
        conn,
        "auto_cycle_enabled",
        if settings.auto_cycle_enabled {
            "true"
        } else {
            "false"
        },
    )?;
    let interval = settings.auto_cycle_interval_minutes.clamp(5, 120);
    set_setting(conn, "auto_cycle_interval_minutes", &interval.to_string())?;
    set_setting(conn, "selected_broker", &settings.selected_broker)?;
    if let Some(v) = &settings.wealth_email {
        set_setting(conn, "wealth_email", v)?;
    }
    set_setting(
        conn,
        "wealth_connected",
        if settings.wealth_connected {
            "true"
        } else {
            "false"
        },
    )?;
    if let Some(v) = &settings.bamboo_phone {
        set_setting(conn, "bamboo_phone", v)?;
    }
    set_setting(
        conn,
        "bamboo_connected",
        if settings.bamboo_connected {
            "true"
        } else {
            "false"
        },
    )?;
    set_setting(
        conn,
        "live_trading_enabled",
        if settings.live_trading_enabled {
            "true"
        } else {
            "false"
        },
    )?;
    set_setting(
        conn,
        "scheduled_live_authorized",
        if settings.scheduled_live_authorized {
            "true"
        } else {
            "false"
        },
    )?;
    set_setting(conn, "max_live_notional", &settings.max_live_notional.to_string())?;
    set_setting(conn, "max_live_actions", &settings.max_live_actions.to_string())?;
    set_setting(
        conn,
        "retain_raw_llm_logs",
        if settings.retain_raw_llm_logs {
            "true"
        } else {
            "false"
        },
    )?;
    Ok(())
}

fn default_selected_broker() -> String {
    "wealth".into()
}

fn finite_in_range(name: &str, value: f64, min: f64, max: f64) -> Result<()> {
    if !value.is_finite() || value < min || value > max {
        anyhow::bail!("{name} must be a finite number between {min} and {max}");
    }
    Ok(())
}

pub fn validate_numeric_settings(settings: &AppSettings) -> Result<()> {
    finite_in_range(
        "default_starting_capital",
        settings.default_starting_capital,
        1_000.0,
        1_000_000_000_000.0,
    )?;
    finite_in_range("simulated_slippage_bps", settings.simulated_slippage_bps, 0.0, 500.0)?;
    finite_in_range("simulated_fee_pct", settings.simulated_fee_pct, 0.0, 0.05)?;
    finite_in_range("max_live_notional", settings.max_live_notional, 1_000.0, 50_000_000.0)?;
    if settings.max_live_actions < 1 || settings.max_live_actions > 40 {
        anyhow::bail!("max_live_actions must be between 1 and 40");
    }
    Ok(())
}

/// Clear user session fields after logout (keeps LLM provider/model preferences and portfolio DB).
pub fn clear_session_settings(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = 'pulse_email'", [])?;
    set_setting(conn, "pulse_configured", "false")?;
    set_setting(conn, "llm_configured", "false")?;
    set_setting(conn, "onboarding_complete", "false")?;
    conn.execute("DELETE FROM settings WHERE key = 'wealth_email'", [])?;
    set_setting(conn, "wealth_connected", "false")?;
    conn.execute("DELETE FROM settings WHERE key = 'bamboo_phone'", [])?;
    set_setting(conn, "bamboo_connected", "false")?;
    set_setting(conn, "live_trading_enabled", "false")?;
    set_setting(conn, "scheduled_live_authorized", "false")?;
    clear_wealth_cache(conn)?;
    clear_bamboo_cache(conn)?;
    Ok(())
}

pub fn clear_wealth_settings(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = 'wealth_email'", [])?;
    set_setting(conn, "wealth_connected", "false")?;
    set_setting(conn, "live_trading_enabled", "false")?;
    set_setting(conn, "scheduled_live_authorized", "false")?;
    clear_wealth_cache(conn)?;
    Ok(())
}

pub fn clear_bamboo_settings(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = 'bamboo_phone'", [])?;
    set_setting(conn, "bamboo_connected", "false")?;
    set_setting(conn, "live_trading_enabled", "false")?;
    set_setting(conn, "scheduled_live_authorized", "false")?;
    clear_bamboo_cache(conn)?;
    Ok(())
}

pub fn clear_wealth_cache(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM wealth_positions", [])?;
    conn.execute("DELETE FROM wealth_account", [])?;
    Ok(())
}

pub fn clear_bamboo_cache(conn: &Connection) -> Result<()> {
    let _ = conn.execute("DELETE FROM bamboo_positions", []);
    let _ = conn.execute("DELETE FROM bamboo_account", []);
    Ok(())
}

pub fn mark_wealth_connected(conn: &Connection, email: &str) -> Result<()> {
    set_setting(conn, "wealth_email", email)?;
    set_setting(conn, "wealth_connected", "true")?;
    Ok(())
}

pub fn mark_bamboo_connected(conn: &Connection, phone: &str) -> Result<()> {
    set_setting(conn, "bamboo_phone", phone)?;
    set_setting(conn, "bamboo_connected", "true")?;
    Ok(())
}

pub fn set_selected_broker(conn: &Connection, broker_id: &str) -> Result<()> {
    set_setting(conn, "selected_broker", broker_id)
}

/// Mark setup complete after Pulse session + LLM verification succeed.
pub fn mark_onboarding_complete(conn: &Connection) -> Result<()> {
    set_setting(conn, "pulse_configured", "true")?;
    set_setting(conn, "llm_configured", "true")?;
    set_setting(conn, "onboarding_complete", "true")?;
    Ok(())
}
