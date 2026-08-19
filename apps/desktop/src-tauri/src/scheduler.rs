use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use tokio::time::MissedTickBehavior;

use crate::app_state::{AppState, CycleGateGuard};
use crate::calendar::TradingCalendar;
use crate::runtime_util::block_on_local;
use crate::secrets::{get_secret, SECRET_PULSE_API_KEY, SECRET_PULSE_PASSWORD};
use crate::settings::get_settings;
use crate::signals::run_cycle;

const SCHEDULER_POLL_SECS: u64 = 30;
/// After a hard auto-cycle failure, wait this long before retrying (not the full interval).
pub const FAIL_BACKOFF: Duration = Duration::from_secs(60);

/// Whether the scheduler should start a cycle now.
pub fn cycle_due(
    last_success_at: Option<Instant>,
    last_fail_at: Option<Instant>,
    interval: Duration,
    now: Instant,
    fail_backoff: Duration,
) -> bool {
    if let Some(fail) = last_fail_at {
        if now.saturating_duration_since(fail) < fail_backoff {
            return false;
        }
    }
    match last_success_at {
        None => true,
        Some(last) => now.saturating_duration_since(last) >= interval,
    }
}

/// Hard failures must not consume the success interval timer.
pub fn record_cycle_outcome(
    last_success_at: &mut Option<Instant>,
    last_fail_at: &mut Option<Instant>,
    now: Instant,
    ran: Result<bool, ()>,
) {
    match ran {
        Ok(true) => {
            *last_success_at = Some(now);
            *last_fail_at = None;
        }
        Ok(false) => {}
        Err(()) => {
            *last_fail_at = Some(now);
        }
    }
}

pub fn start_scheduler(app: AppHandle, state: Arc<AppState>) {
    let app_cycle = app.clone();
    let state_cycle = state.clone();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(SCHEDULER_POLL_SECS));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_success_at: Option<Instant> = None;
        let mut last_fail_at: Option<Instant> = None;

        loop {
            ticker.tick().await;

            let settings = match state_cycle.db.with_conn(get_settings) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(target: "scheduler", error = %e, "failed to load settings");
                    continue;
                }
            };

            if !settings.auto_cycle_enabled || !settings.onboarding_complete {
                continue;
            }

            let interval =
                Duration::from_secs(u64::from(settings.auto_cycle_interval_minutes.clamp(5, 120)) * 60);
            if !cycle_due(
                last_success_at,
                last_fail_at,
                interval,
                Instant::now(),
                FAIL_BACKOFF,
            ) {
                continue;
            }

            let calendar = TradingCalendar::default();
            if !crate::broker::busha_connected()
                && !crate::runtime_util::market_activity_allowed(&calendar)
            {
                continue;
            }

            let app2 = app_cycle.clone();
            let state2 = state_cycle.clone();
            let now = Instant::now();
            match tokio::task::spawn_blocking(move || run_scheduled_cycle(&app2, &state2)).await {
                Ok(Ok(true)) => {
                    tracing::info!(target: "scheduler", "auto cycle completed");
                    record_cycle_outcome(&mut last_success_at, &mut last_fail_at, now, Ok(true));
                }
                Ok(Ok(false)) => {
                    tracing::info!(target: "scheduler", "auto cycle skipped (already running)");
                    record_cycle_outcome(&mut last_success_at, &mut last_fail_at, now, Ok(false));
                }
                Ok(Err(e)) => {
                    tracing::warn!(target: "scheduler", error = %e, "auto cycle failed");
                    record_cycle_outcome(&mut last_success_at, &mut last_fail_at, now, Err(()));
                }
                Err(e) => {
                    tracing::warn!(target: "scheduler", error = %e, "auto cycle task join failed");
                    record_cycle_outcome(&mut last_success_at, &mut last_fail_at, now, Err(()));
                }
            }
        }
    });

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = tokio::task::spawn_blocking(move || catch_up_on_launch(&app, &state)).await;
    });
}

fn catch_up_on_launch(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<()> {
    let settings = state.db.with_conn(get_settings)?;
    let pulse_password = get_secret(SECRET_PULSE_PASSWORD)?;
    let pulse_api_key = get_secret(SECRET_PULSE_API_KEY)?;
    let client = crate::ngx::NgxPulseClient::from_settings(&settings, pulse_password, pulse_api_key);
    let calendar = TradingCalendar::default();

    block_on_local(async {
        match crate::ingest::IngestionService::ingest_stocks(
            &state.db,
            &client,
            &state.cache,
            &calendar,
            false,
        )
        .await
        {
            Ok(n) => tracing::info!(target: "ngx_pulse", count = n, "launch stock ingest"),
            Err(e) => tracing::warn!(target: "ngx_pulse", error = %e, "launch stock ingest failed"),
        }
        Ok::<_, anyhow::Error>(())
    })?;

    let _ = app.emit("ingest:complete", ());
    Ok(())
}

/// Returns `Ok(true)` if a cycle ran, `Ok(false)` if skipped because another cycle holds the gate.
fn run_scheduled_cycle(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<bool> {
    let settings = state.db.with_conn(get_settings)?;
    if !settings.auto_cycle_enabled || !settings.onboarding_complete {
        return Ok(false);
    }

    let Some(_gate) = CycleGateGuard::acquire(state.clone()) else {
        return Ok(false);
    };

    let _ = app.emit(
        "cycle:start",
        serde_json::json!({ "source": "scheduler" }),
    );

    let pulse_password = get_secret(SECRET_PULSE_PASSWORD)?;
    let pulse_api_key = get_secret(SECRET_PULSE_API_KEY)?;
    let client = crate::ngx::NgxPulseClient::from_settings(&settings, pulse_password, pulse_api_key);
    let broker = crate::broker::open_live_broker(&settings);
    let calendar = TradingCalendar::default();

    match block_on_local(run_cycle(
        &state.db,
        &state.agent,
        &settings,
        &state.cache,
        &client,
        &calendar,
        broker.as_ref(),
        false,
        None,
    )) {
        Ok(result) => {
            let mut payload = result;
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("ok".into(), serde_json::json!(true));
                obj.insert("source".into(), serde_json::json!("scheduler"));
            }
            let _ = app.emit("cycle:complete", payload);
            Ok(true)
        }
        Err(e) => {
            let payload = serde_json::json!({
                "ok": false,
                "source": "scheduler",
                "signals": 0,
                "executed": 0,
                "warnings": [],
                "error": e.to_string(),
            });
            let _ = app.emit("cycle:complete", payload);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_failure_does_not_set_success_timer() {
        let mut success = None;
        let mut fail = None;
        let t0 = Instant::now();
        record_cycle_outcome(&mut success, &mut fail, t0, Err(()));
        assert!(success.is_none());
        assert!(fail.is_some());
    }

    #[test]
    fn success_clears_fail_backoff() {
        let mut success = None;
        let mut fail = Some(Instant::now());
        let t0 = Instant::now();
        record_cycle_outcome(&mut success, &mut fail, t0, Ok(true));
        assert!(success.is_some());
        assert!(fail.is_none());
    }

    #[test]
    fn skip_does_not_advance_timers() {
        let mut success = Some(Instant::now());
        let mut fail = None;
        let before = success;
        record_cycle_outcome(&mut success, &mut fail, Instant::now(), Ok(false));
        assert_eq!(success, before);
        assert!(fail.is_none());
    }

    #[test]
    fn fail_backoff_blocks_then_allows() {
        let t0 = Instant::now();
        let fail = Some(t0);
        let interval = Duration::from_secs(1800);
        assert!(!cycle_due(None, fail, interval, t0, FAIL_BACKOFF));
        assert!(cycle_due(
            None,
            fail,
            interval,
            t0 + FAIL_BACKOFF,
            FAIL_BACKOFF
        ));
        let success = Some(t0);
        assert!(!cycle_due(
            success,
            None,
            interval,
            t0 + Duration::from_secs(60),
            FAIL_BACKOFF
        ));
        assert!(cycle_due(
            success,
            None,
            interval,
            t0 + interval,
            FAIL_BACKOFF
        ));
    }
}
