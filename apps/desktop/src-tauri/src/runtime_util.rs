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

/// Whether live broker orders may be submitted given the NGX session clock.
/// Unlike `market_activity_allowed`, this does **not** include the post-close
/// ingest window — live fills must not go out after 16:00 WAT.
pub fn ngx_session_open_for_live_orders(calendar: &crate::calendar::TradingCalendar) -> bool {
    if !enforce_market_hours() {
        return true;
    }
    calendar.is_market_open()
}

/// Broker market flag with NGX calendar fallback when the API is stale or wrong.
pub async fn live_broker_market_open(
    session: &crate::broker::BrokerSession,
    calendar: &crate::calendar::TradingCalendar,
) -> bool {
    if session.id() == crate::broker::BrokerId::Busha {
        return true;
    }
    broker_open_with_calendar_fallback(
        session.market_is_open().await.ok(),
        ngx_session_open_for_live_orders(calendar),
    )
}

/// If the broker reports open, trust it. If it reports closed or errors, use the NGX calendar.
pub fn broker_open_with_calendar_fallback(broker_open: Option<bool>, calendar_allows: bool) -> bool {
    match broker_open {
        Some(true) => true,
        Some(false) | None => calendar_allows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_open_wins_even_if_calendar_closed() {
        assert!(broker_open_with_calendar_fallback(Some(true), false));
    }

    #[test]
    fn calendar_overrides_stale_broker_closed() {
        assert!(broker_open_with_calendar_fallback(Some(false), true));
        assert!(broker_open_with_calendar_fallback(None, true));
    }

    #[test]
    fn both_closed_stays_closed() {
        assert!(!broker_open_with_calendar_fallback(Some(false), false));
        assert!(!broker_open_with_calendar_fallback(None, false));
    }

    #[test]
    fn live_orders_exclude_post_close_window() {
        use crate::calendar::TradingCalendar;
        let calendar = TradingCalendar::default();
        // In dev, gates are always open regardless of clock.
        if is_dev() {
            assert!(ngx_session_open_for_live_orders(&calendar));
            return;
        }
        // When calendar says post-close only, cycles may ingest but live orders must not.
        if calendar.is_post_close_window() && !calendar.is_market_open() {
            assert!(!ngx_session_open_for_live_orders(&calendar));
            assert!(market_activity_allowed(&calendar));
        }
    }
}
