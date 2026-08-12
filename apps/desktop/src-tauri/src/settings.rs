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
    if let Some(v) = &settings.llm_base_url {
        set_setting(conn, "llm_base_url", v)?;
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
    Ok(())
}

/// Clear user session fields after logout (keeps LLM provider/model preferences and portfolio DB).
pub fn clear_session_settings(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = 'pulse_email'", [])?;
    set_setting(conn, "pulse_configured", "false")?;
    set_setting(conn, "llm_configured", "false")?;
    set_setting(conn, "onboarding_complete", "false")?;
    Ok(())
}

/// Mark setup complete after Pulse session + LLM verification succeed.
pub fn mark_onboarding_complete(conn: &Connection) -> Result<()> {
    set_setting(conn, "pulse_configured", "true")?;
    set_setting(conn, "llm_configured", "true")?;
    set_setting(conn, "onboarding_complete", "true")?;
    Ok(())
}
