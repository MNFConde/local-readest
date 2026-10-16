//! Pure restart-policy decisions, kept side-effect free so the backoff and
//! circuit-breaker behavior can be unit-tested without spawning anything.
//!
//! Semantics (ROADMAP 待办⑦):
//!   * a run that lasted >= MIN_RUN_FOR_RESET counts as "it worked for a
//!     while" — the rapid-failure counter resets, so a nightly crash still
//!     restarts immediately instead of creeping to 16s delays forever;
//!   * each consecutive rapid failure delays the next start exponentially
//!     (1s → 4s → 16s, capped), so a port conflict does not turn into a
//!     fork bomb;
//!   * MAX_CONSECUTIVE_FAILURES rapid failures in a row open the circuit:
//!     the service stays down with a logged reason. Restarting can only
//!     succeed after the operator fixes the cause — retry-looping forever
//!     would only hide it.

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
        let mut failures = 0u32;
        assert!(ran_long_enough_to_reset(Duration::from_secs(600)));
        failures = 0;
        failures += 1;
        assert_eq!(backoff_delay(failures), Duration::from_secs(1));
        assert!(!should_circuit_break(failures));
        failures += 1;
        assert_eq!(backoff_delay(failures), Duration::from_secs(4));
        assert!(!should_circuit_break(failures));
    }
}
