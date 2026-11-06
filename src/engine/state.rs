//! Per engine runtime state: suspensions and statistics, ported from the
//! error handling in `searx/engines/__init__.py` (`ban_time_on_fail`,
//! `max_ban_time_on_fail` and `suspended_times`).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::FailureKind;
use crate::settings::Search;

/// Counters kept per engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineStats {
    pub requests: u64,
    pub results: u64,
    pub errors: u64,
    pub timeouts: u64,
    pub http_errors: u64,
    pub total_time: f64,
}

impl EngineStats {
    /// Average response time in seconds.
    pub fn average_time(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.total_time / self.requests as f64
        }
    }
}

#[derive(Debug, Clone, Default)]
struct EngineState {
    consecutive_failures: u64,
    /// Successful answers that carried no results. An engine that keeps
    /// answering nothing is costing every search its timeout for free.
    empty_streak: u64,
    /// When the engine may rejoin the rotation.
    quiet_until: Option<Instant>,
    suspended_until: Option<Instant>,
    suspension_reason: Option<String>,
    stats: EngineStats,
}

/// A read-only view of one engine's state, used by `/stats`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSnapshot {
    pub engine: String,
    pub stats: EngineStats,
    pub consecutive_failures: u64,
    pub suspended: bool,
    pub suspended_for: f64,
    pub suspension_reason: Option<String>,
    /// Engine is resting due to repeated empty results.
    pub resting: bool,
    /// Remaining rest time in seconds.
    pub resting_for: f64,
}

/// Thread safe map of engine states.
#[derive(Debug, Default)]
pub struct EngineStates {
    states: Mutex<HashMap<String, EngineState>>,
    /// Engines where a page fetched by the browser solver still failed to
    /// parse. The browser cannot help them, so retrying only spends a tab.
    solver_dead: Mutex<HashSet<String>>,
    /// Engines the browser solver has already delivered results for. They are
    /// worth queueing a tab for; every other blocked engine is not, until it
    /// proves itself.
    solver_useful: Mutex<HashSet<String>>,
}

impl EngineStates {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an earlier solver attempt already proved useless for an engine.
    pub fn solver_cannot_help(&self, engine: &str) -> bool {
        self.solver_dead
            .lock()
            .expect("solver marks are not poisoned")
            .contains(engine)
    }

    /// Whether the browser solver has already produced results for an engine.
    pub fn solver_is_useful(&self, engine: &str) -> bool {
        self.solver_useful
            .lock()
            .expect("solver marks are not poisoned")
            .contains(engine)
    }

    /// Remember that a page cleared by the solver parsed into results.
    pub fn mark_solver_useful(&self, engine: &str) {
        let mut useful = self
            .solver_useful
            .lock()
            .expect("solver marks are not poisoned");
        if useful.insert(engine.to_string()) {
            tracing::debug!(engine, "the solver works for this engine; it may queue");
        }
    }

    /// Remember that a page cleared by the solver still did not parse.
    pub fn mark_solver_cannot_help(&self, engine: &str) {
        let mut dead = self
            .solver_dead
            .lock()
            .expect("solver marks are not poisoned");
        if dead.insert(engine.to_string()) {
            tracing::debug!(
                engine,
                "the solver cannot help this engine; no further attempts"
            );
        }
    }

    fn with_state<T>(&self, engine: &str, action: impl FnOnce(&mut EngineState) -> T) -> T {
        let mut states = self.states.lock().expect("engine states are not poisoned");
        let state = states.entry(engine.to_string()).or_default();
        action(state)
    }

    /// Remaining suspension time, if the engine is suspended.
    pub fn suspension(&self, engine: &str) -> Option<(Duration, Option<String>)> {
        self.with_state(engine, |state| {
            // An engine that keeps answering nothing is rested, not failed.
            if let Some(until) = state.quiet_until {
                let now = Instant::now();
                if until > now {
                    return Some((until - now, Some("resting".to_string())));
                }
                state.quiet_until = None;
            }
            state.suspended_until.and_then(|until| {
                let now = Instant::now();
                if until > now {
                    Some((until - now, state.suspension_reason.clone()))
                } else {
                    state.suspended_until = None;
                    state.suspension_reason = None;
                    None
                }
            })
        })
    }

    /// Record a successful request.
    pub fn record_success(
        &self,
        engine: &str,
        results: usize,
        elapsed: Duration,
        settings: &Search,
    ) {
        self.with_state(engine, |state| {
            state.consecutive_failures = 0;
            if results == 0 {
                state.empty_streak += 1;
                if settings.empty_result_limit > 0
                    && state.empty_streak >= settings.empty_result_limit
                {
                    state.quiet_until = Some(
                        Instant::now() + Duration::from_secs(settings.empty_result_cooldown.max(1)),
                    );
                    state.empty_streak = 0;
                }
            } else {
                state.empty_streak = 0;
                state.quiet_until = None;
            }
            state.suspended_until = None;
            state.suspension_reason = None;
            state.stats.requests += 1;
            state.stats.results += results as u64;
            state.stats.total_time += elapsed.as_secs_f64();
        });
    }

    /// Record a failure and return the suspension time in seconds (0 = none).
    ///
    /// `cap` shortens the recorded suspension. It exists for the browser
    /// solver: a challenge that the solver can clear should not bench the
    /// engine for a day, because the very next search may get through.
    pub fn record_failure(
        &self,
        engine: &str,
        kind: FailureKind,
        settings: &Search,
        cap: Option<u64>,
    ) -> u64 {
        self.with_state(engine, |state| {
            state.stats.errors += 1;
            if kind == FailureKind::Timeout {
                state.stats.timeouts += 1;
            }
            if kind == FailureKind::Http {
                state.stats.http_errors += 1;
            }
            state.consecutive_failures += 1;

            let suspension = match kind.suspension_key() {
                Some(key) => settings.suspended_times.get(key).copied().unwrap_or(3_600),
                None => {
                    // Repeated soft failures degrade into a short suspension,
                    // exactly like upstream's ban_time_on_fail.
                    if state.consecutive_failures >= 2 {
                        (state.consecutive_failures * settings.ban_time_on_fail)
                            .min(settings.max_ban_time_on_fail)
                    } else {
                        0
                    }
                }
            };
            let suspension = match cap {
                Some(cap) if cap > 0 => suspension.min(cap),
                _ => suspension,
            };
            if suspension > 0 {
                state.suspended_until = Some(Instant::now() + Duration::from_secs(suspension));
                state.suspension_reason = Some(kind.error_type().to_string());
            }
            suspension
        })
    }

    /// Reset the counters (used by tests and by `/stats` reset).
    pub fn reset(&self) {
        self.states
            .lock()
            .expect("engine states are not poisoned")
            .clear();
    }

    /// Snapshot of every engine that has been used.
    pub fn snapshot(&self) -> Vec<EngineSnapshot> {
        let now = Instant::now();
        let states = self.states.lock().expect("engine states are not poisoned");
        let mut snapshots: Vec<EngineSnapshot> = states
            .iter()
            .map(|(engine, state)| {
                let (suspended, suspended_for, suspension_reason) = match state.suspended_until {
                    Some(until) if until > now => (
                        true,
                        (until - now).as_secs_f64(),
                        state.suspension_reason.clone(),
                    ),
                    _ => (false, 0.0, None),
                };
                let (resting, resting_for) = match state.quiet_until {
                    Some(until) if until > now => (true, (until - now).as_secs_f64()),
                    _ => (false, 0.0),
                };
                EngineSnapshot {
                    engine: engine.clone(),
                    stats: state.stats.clone(),
                    consecutive_failures: state.consecutive_failures,
                    suspended,
                    suspended_for,
                    suspension_reason,
                    resting,
                    resting_for,
                }
            })
            .collect();
        snapshots.sort_by(|a, b| a.engine.cmp(&b.engine));
        snapshots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_denied_suspends_for_a_day() {
        let states = EngineStates::new();
        let settings = Search::default();
        let seconds =
            states.record_failure("duckduckgo", FailureKind::AccessDenied, &settings, None);
        assert_eq!(seconds, 86_400);
        assert!(states.suspension("duckduckgo").is_some());
    }

    #[test]
    fn a_solver_cap_shortens_a_day_long_ban() {
        let states = EngineStates::new();
        let settings = Search::default();
        let seconds = states.record_failure("yandex", FailureKind::Captcha, &settings, Some(300));
        assert_eq!(seconds, 300);
        assert!(states.suspension("yandex").is_some());
    }

    #[test]
    fn two_soft_failures_trigger_a_short_suspension() {
        let states = EngineStates::new();
        let settings = Search::default();
        assert_eq!(
            states.record_failure("x", FailureKind::Other, &settings, None),
            0
        );
        let second = states.record_failure("x", FailureKind::Other, &settings, None);
        assert_eq!(second, 10);
        assert!(states.suspension("x").is_some());
    }

    #[test]
    fn success_clears_the_state() {
        let states = EngineStates::new();
        let settings = Search::default();
        states.record_failure("x", FailureKind::TooManyRequests, &settings, None);
        states.record_success("x", 3, Duration::from_millis(100), &Search::default());
        assert!(states.suspension("x").is_none());
        let snapshot = states.snapshot();
        assert_eq!(snapshot[0].stats.results, 3);
        assert_eq!(snapshot[0].consecutive_failures, 0);
    }

    #[test]
    fn empty_results_below_limit_do_not_rest() {
        let states = EngineStates::new();
        let settings = Search {
            empty_result_limit: 3,
            empty_result_cooldown: 60,
            ..Default::default()
        };

        // N-1 empty results: no resting yet
        for _ in 0..2 {
            states.record_success("eng", 0, Duration::from_millis(50), &settings);
        }
        let snapshot = states.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert!(!snapshot[0].resting);
        assert_eq!(snapshot[0].resting_for, 0.0);
    }

    #[test]
    fn empty_results_at_limit_trigger_resting() {
        let states = EngineStates::new();
        let settings = Search {
            empty_result_limit: 3,
            empty_result_cooldown: 60,
            ..Default::default()
        };

        // Exactly N empty results triggers rest
        for _ in 0..3 {
            states.record_success("eng", 0, Duration::from_millis(50), &settings);
        }
        let snapshot = states.snapshot();
        assert!(snapshot[0].resting);
        assert!(snapshot[0].resting_for > 0.0);

        // suspension() should return the resting status
        let sus = states.suspension("eng");
        assert!(sus.is_some());
        let (_, reason) = sus.unwrap();
        assert_eq!(reason.as_deref(), Some("resting"));
    }

    #[test]
    fn cooldown_expiry_clears_resting() {
        let states = EngineStates::new();
        let settings = Search {
            empty_result_limit: 2,
            empty_result_cooldown: 60,
            ..Default::default()
        };

        // Trigger resting
        states.record_success("eng", 0, Duration::from_millis(50), &settings);
        states.record_success("eng", 0, Duration::from_millis(50), &settings);
        assert!(states.suspension("eng").is_some());

        // Simulate cooldown expiry by setting quiet_until to the past
        states.with_state("eng", |state| {
            state.quiet_until = Some(Instant::now() - Duration::from_secs(1));
        });

        // snapshot should return resting=false before suspension() cleans up
        let snapshot = states.snapshot();
        assert!(!snapshot[0].resting);
        assert_eq!(snapshot[0].resting_for, 0.0);

        // suspension() also returns None after expiry
        assert!(states.suspension("eng").is_none());
    }

    #[test]
    fn actual_results_clear_resting_and_reset_streak() {
        let states = EngineStates::new();
        let settings = Search {
            empty_result_limit: 2,
            empty_result_cooldown: 60,
            ..Default::default()
        };

        // Trigger resting
        states.record_success("eng", 0, Duration::from_millis(50), &settings);
        states.record_success("eng", 0, Duration::from_millis(50), &settings);
        assert!(states.suspension("eng").is_some());

        // Return actual results - should clear resting immediately
        states.record_success("eng", 5, Duration::from_millis(100), &settings);

        assert!(states.suspension("eng").is_none());
        let snapshot = states.snapshot();
        assert!(!snapshot[0].resting);
        assert_eq!(snapshot[0].resting_for, 0.0);

        // The empty streak counter should be reset, so N-1 more empties don't rest
        states.record_success("eng", 0, Duration::from_millis(50), &settings);
        assert!(states.suspension("eng").is_none());
    }

    #[test]
    fn limit_zero_disables_resting() {
        let states = EngineStates::new();
        let settings = Search {
            empty_result_limit: 0, // disabled
            empty_result_cooldown: 60,
            ..Default::default()
        };

        // Many empty results should not trigger resting
        for _ in 0..100 {
            states.record_success("eng", 0, Duration::from_millis(50), &settings);
        }
        assert!(states.suspension("eng").is_none());
        let snapshot = states.snapshot();
        assert!(!snapshot[0].resting);
    }

    #[test]
    fn resting_distinct_from_failure_suspension() {
        let states = EngineStates::new();
        let settings = Search {
            empty_result_limit: 2,
            empty_result_cooldown: 60,
            ..Default::default()
        };

        // Trigger failure suspension
        states.record_failure("a", FailureKind::AccessDenied, &settings, None);
        let snap_a = states
            .snapshot()
            .into_iter()
            .find(|s| s.engine == "a")
            .unwrap();
        assert!(snap_a.suspended);
        assert!(!snap_a.resting);
        assert!(snap_a.suspension_reason.is_some());

        // Trigger resting
        states.record_success("b", 0, Duration::from_millis(50), &settings);
        states.record_success("b", 0, Duration::from_millis(50), &settings);
        let snap_b = states
            .snapshot()
            .into_iter()
            .find(|s| s.engine == "b")
            .unwrap();
        assert!(!snap_b.suspended);
        assert!(snap_b.resting);
        assert!(snap_b.suspension_reason.is_none());

        // Both appear in suspension() but with different reasons
        let (_, reason_a) = states.suspension("a").unwrap();
        let (_, reason_b) = states.suspension("b").unwrap();
        assert_ne!(reason_a, reason_b);
        assert_eq!(reason_b.as_deref(), Some("resting"));
    }

    #[test]
    fn snapshot_clears_expired_suspension_reason() {
        let states = EngineStates::new();
        let settings = Search::default();

        // Create a suspension
        states.record_failure("eng", FailureKind::AccessDenied, &settings, None);
        let snap = states
            .snapshot()
            .into_iter()
            .find(|s| s.engine == "eng")
            .unwrap();
        assert!(snap.suspended);
        assert!(snap.suspension_reason.is_some());

        // Simulate expiry by setting suspended_until to the past
        states.with_state("eng", |state| {
            state.suspended_until = Some(Instant::now() - Duration::from_secs(1));
            state.suspension_reason = Some("access_denied".to_string());
        });

        // Snapshot should clear the reason since suspension is expired
        let snap = states
            .snapshot()
            .into_iter()
            .find(|s| s.engine == "eng")
            .unwrap();
        assert!(!snap.suspended);
        assert!(snap.suspension_reason.is_none());
    }
}
