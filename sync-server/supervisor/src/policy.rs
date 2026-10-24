//! Pure restart-policy decisions, kept side-effect free so the backoff and
//! circuit-breaker behavior can be unit-tested without spawning anything.
//!
//! Semantics (ROADMAP 待办⑦;⑩ 增补手动控制后的完整状态图见 service.rs):
//!   * a run that lasted >= MIN_RUN_FOR_RESET counts as "it worked for a
//!     while" — the rapid-failure counter resets, so a nightly crash still
//!     restarts immediately instead of creeping to 16s delays forever;
//!   * each consecutive rapid failure delays the next start exponentially
//!     (1s → 4s → 16s, capped), so a port conflict does not turn into a
//!     fork bomb;
//!   * MAX_CONSECUTIVE_FAILURES rapid failures in a row open the circuit:
//!     from ⑩ on the service then stays down with a logged reason WAITING
//!     for an operator Start (used to be: the runner exits and only a
//!     supervisor restart recovered). Retry-looping forever would only hide
//!     the cause — "only a human recovers it" is unchanged;
//!   * an operator-initiated exit (manual Stop or Restart, ⑩) is never a
//!     failure: Stop must not burn backoff budget or creep toward the
//!     circuit breaker just because someone did maintenance.

use std::time::Duration;

/// A run shorter than this counts as a rapid failure.
pub const MIN_RUN_FOR_RESET: Duration = Duration::from_secs(30);
/// Rapid failures at this count open the circuit (service given up).
pub const MAX_CONSECUTIVE_FAILURES: u32 = 5;
/// Backoff ladder before restart #1, #2, #3+ (capped at the last step).
const BACKOFF_STEPS_SECS: [u64; 3] = [1, 4, 16];

/// Delay before the next start after `consecutive_failures` rapid failures
/// (1-based: the first failure waits 1s, the second 4s, later ones 16s).
pub fn backoff_delay(consecutive_failures: u32) -> Duration {
    let last = BACKOFF_STEPS_SECS.len() as u32 - 1;
    let idx = consecutive_failures.saturating_sub(1).min(last) as usize;
    Duration::from_secs(BACKOFF_STEPS_SECS[idx])
}

/// True when a run was long enough to forgive earlier rapid failures.
pub fn ran_long_enough_to_reset(uptime: Duration) -> bool {
    uptime >= MIN_RUN_FOR_RESET
}

/// True when the rapid-failure count should stop the restart loop entirely.
pub fn should_circuit_break(consecutive_failures: u32) -> bool {
    consecutive_failures >= MAX_CONSECUTIVE_FAILURES
}

/// How one child exit affects the rapid-failure counter. The four outcomes
/// (ROADMAP ⑩): an operator action is blameless; a long run forgives earlier
/// rapid failures; a short natural run is counted; zero-uptime crashes count
/// hardest of all (still just one tick, but never reset).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExitAccounting {
    /// Operator Stop/Restart killed it: the counter is untouched — a
    /// maintenance stop must not eat backoff budget or creep toward the
    /// circuit breaker.
    Operator,
    /// Ran long enough to forgive earlier rapid failures (counter → 0).
    Reset,
    /// Died on its own quickly: counts toward the backoff ladder/breaker.
    Count,
}

/// Classify one exit. `operator_action` is true whenever this exit was
/// caused by a manual command (Stop or Restart), regardless of uptime.
pub fn classify_exit(uptime: Duration, operator_action: bool) -> ExitAccounting {
    if operator_action {
        ExitAccounting::Operator
    } else if ran_long_enough_to_reset(uptime) {
        ExitAccounting::Reset
    } else {
        ExitAccounting::Count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_ladder_is_1s_4s_16s_then_capped() {
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(3), Duration::from_secs(16));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
        assert_eq!(backoff_delay(99), Duration::from_secs(16));
    }

    #[test]
    fn backoff_clamps_zero_failures_to_first_step() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
    }

    #[test]
    fn long_runs_reset_but_short_ones_do_not() {
        assert!(ran_long_enough_to_reset(Duration::from_secs(30)));
        assert!(ran_long_enough_to_reset(Duration::from_secs(3600)));
        assert!(!ran_long_enough_to_reset(Duration::from_secs(29)));
        assert!(!ran_long_enough_to_reset(Duration::ZERO));
    }

    #[test]
    fn circuit_opens_at_five_rapid_failures() {
        assert!(!should_circuit_break(4));
        assert!(should_circuit_break(5));
        assert!(should_circuit_break(50));
    }

    #[test]
    fn a_long_run_then_two_short_ones_stay_below_the_break() {
        // 30s run resets the counter, so two rapid failures land at 2 — the
        // service gets its exponential delay but is not given up on.
        assert!(ran_long_enough_to_reset(Duration::from_secs(600)));
        let mut failures = 0u32;
        failures += 1;
        assert_eq!(backoff_delay(failures), Duration::from_secs(1));
        assert!(!should_circuit_break(failures));
        failures += 1;
        assert_eq!(backoff_delay(failures), Duration::from_secs(4));
        assert!(!should_circuit_break(failures));
    }

    // ── ⑩ manual-control branches ─────────────────────────────────────────

    #[test]
    fn manual_stop_is_blameless_even_after_a_blink() {
        // A Stop that lands while the child is still young must not count as
        // a rapid failure: maintenance stops never burn backoff budget.
        assert_eq!(
            classify_exit(Duration::from_secs(2), true),
            ExitAccounting::Operator
        );
        assert_eq!(
            classify_exit(Duration::ZERO, true),
            ExitAccounting::Operator
        );
    }

    #[test]
    fn manual_restart_is_blameless_too() {
        // Restart kills a healthy child mid-run; the ensuing start is a
        // fresh operator intent, not a failure of the old one.
        assert_eq!(
            classify_exit(Duration::from_secs(120), true),
            ExitAccounting::Operator
        );
    }

    #[test]
    fn natural_rapid_failure_counts_even_at_zero_uptime() {
        assert_eq!(
            classify_exit(Duration::from_secs(3), false),
            ExitAccounting::Count
        );
        assert_eq!(classify_exit(Duration::ZERO, false), ExitAccounting::Count);
    }

    #[test]
    fn natural_long_run_resets() {
        assert_eq!(
            classify_exit(Duration::from_secs(30), false),
            ExitAccounting::Reset
        );
    }
}
