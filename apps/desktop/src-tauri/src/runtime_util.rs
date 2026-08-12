/// Run an async future on a fresh current-thread runtime.
/// Safe to call from `tokio::task::spawn_blocking` (not from a Tokio worker directly).
pub fn block_on_local<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("local runtime")
        .block_on(future)
}

/// `APP_ENV` from process environment: `dev` or `production` (default).
pub fn app_env() -> &'static str {
    static ENV: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    ENV.get_or_init(|| {
        match std::env::var("APP_ENV")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "dev" | "development" => "dev",
            _ => "production",
        }
    })
}

pub fn is_dev() -> bool {
    app_env() == "dev"
}

/// When true, scheduler / ingest / cycle_run must respect NGX market hours.
pub fn enforce_market_hours() -> bool {
    !is_dev()
}

/// Whether trading/cycle activity is allowed right now given calendar + APP_ENV.
pub fn market_activity_allowed(calendar: &crate::calendar::TradingCalendar) -> bool {
    if !enforce_market_hours() {
        return true;
    }
    calendar.is_market_open() || calendar.is_post_close_window()
}
