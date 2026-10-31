//! The degraded state: an up monitor whose responses stay above its latency threshold. Pure: no
//! I/O, no clock.

/// The part of `monitor_state` that tracks slow responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slowness {
    /// Successful checks in a row slower than the threshold.
    pub slow_checks: i64,
    /// `checked_at` of the first check of that series.
    pub slow_since: Option<i64>,
    /// Since when the monitor is degraded; `None` when it is not.
    pub degraded_since: Option<i64>,
}

impl Slowness {
    pub const NONE: Self = Self {
        slow_checks: 0,
        slow_since: None,
        degraded_since: None,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlowTransition {
    /// `threshold` slow checks in a row, above `threshold_ms`; the series started at `since`.
    Degraded { since: i64, threshold_ms: i64 },
    /// A fast response ended the slowdown that started at `degraded_since`.
    Normal { degraded_since: i64 },
}

/// What one check tells about speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub checked_at: i64,
    /// `None` for a failed check: it says nothing about speed.
    pub latency_ms: Option<i64>,
}

/// Applies one check to the slowness of a monitor. `warn_ms` is the latency threshold (`None`:
/// off), `threshold` the number of slow checks in a row that make the monitor degraded.
/// `went_down` ends a slowdown silently: an outage replaces it. During a maintenance window
/// (`frozen`) nothing is alerted: a fast check ends a slowdown silently and a slow one drops the
/// series.
pub fn apply(
    slowness: Slowness,
    sample: Sample,
    warn_ms: Option<i64>,
    threshold: i64,
    went_down: bool,
    frozen: bool,
) -> (Slowness, Option<SlowTransition>) {
    let Some(warn_ms) = warn_ms.filter(|_| !went_down) else {
        return (Slowness::NONE, None);
    };
    let Some(latency) = sample.latency_ms else {
        return (slowness, None);
    };
    if latency <= warn_ms {
        let transition = slowness
            .degraded_since
            .filter(|_| !frozen)
            .map(|degraded_since| SlowTransition::Normal { degraded_since });
        return (Slowness::NONE, transition);
    }
    // Degraded: nothing more to count, and no write to make.
    if slowness.degraded_since.is_some() && !frozen {
        return (slowness, None);
    }
    if frozen {
        return (
            Slowness {
                slow_checks: 0,
                slow_since: None,
                ..slowness
            },
            None,
        );
    }
    let since = slowness.slow_since.unwrap_or(sample.checked_at);
    let slow_checks = slowness.slow_checks + 1;
    let degrades = slow_checks >= threshold;
    let next = Slowness {
        slow_checks,
        slow_since: Some(since),
        degraded_since: degrades.then_some(since),
    };
    (
        next,
        degrades.then_some(SlowTransition::Degraded {
            since,
            threshold_ms: warn_ms,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slow(checks: i64, since: Option<i64>, degraded: Option<i64>) -> Slowness {
        Slowness {
            slow_checks: checks,
            slow_since: since,
            degraded_since: degraded,
        }
    }

    fn at(checked_at: i64, latency: i64) -> Sample {
        Sample {
            checked_at,
            latency_ms: Some(latency),
        }
    }

    fn failed(checked_at: i64) -> Sample {
        Sample {
            checked_at,
            latency_ms: None,
        }
    }

    #[test]
    fn slow_checks_below_threshold_only_count() {
        assert_eq!(
            apply(Slowness::NONE, at(10, 900), Some(500), 3, false, false),
            (slow(1, Some(10), None), None)
        );
        assert_eq!(
            apply(
                slow(1, Some(10), None),
                at(20, 900),
                Some(500),
                3,
                false,
                false
            ),
            (slow(2, Some(10), None), None)
        );
    }

    #[test]
    fn reaching_the_threshold_degrades_from_the_first_slow_check() {
        assert_eq!(
            apply(
                slow(2, Some(10), None),
                at(30, 900),
                Some(500),
                3,
                false,
                false
            ),
            (
                slow(3, Some(10), Some(10)),
                Some(SlowTransition::Degraded {
                    since: 10,
                    threshold_ms: 500
                })
            )
        );
    }

    #[test]
    fn slow_while_degraded_changes_nothing() {
        let degraded = slow(3, Some(10), Some(10));
        assert_eq!(
            apply(degraded, at(40, 900), Some(500), 3, false, false),
            (degraded, None)
        );
    }

    #[test]
    fn latency_at_the_threshold_is_not_slow() {
        assert_eq!(
            apply(
                slow(1, Some(10), None),
                at(20, 500),
                Some(500),
                3,
                false,
                false
            ),
            (Slowness::NONE, None)
        );
    }

    #[test]
    fn fast_check_ends_a_slowdown() {
        assert_eq!(
            apply(
                slow(4, Some(10), Some(10)),
                at(50, 100),
                Some(500),
                3,
                false,
                false
            ),
            (
                Slowness::NONE,
                Some(SlowTransition::Normal { degraded_since: 10 })
            )
        );
    }

    #[test]
    fn failed_check_changes_nothing() {
        let degraded = slow(4, Some(10), Some(10));
        assert_eq!(
            apply(degraded, failed(50), Some(500), 3, false, false),
            (degraded, None)
        );
    }

    #[test]
    fn going_down_or_no_threshold_clears_silently() {
        let degraded = slow(4, Some(10), Some(10));
        assert_eq!(
            apply(degraded, failed(50), Some(500), 3, true, false),
            (Slowness::NONE, None)
        );
        assert_eq!(
            apply(degraded, at(50, 900), None, 3, false, false),
            (Slowness::NONE, None)
        );
    }

    #[test]
    fn maintenance_alerts_nothing() {
        assert_eq!(
            apply(
                slow(2, Some(10), None),
                at(30, 900),
                Some(500),
                3,
                false,
                true
            ),
            (Slowness::NONE, None)
        );
        assert_eq!(
            apply(
                slow(4, Some(10), Some(10)),
                at(50, 900),
                Some(500),
                3,
                false,
                true
            ),
            (slow(0, None, Some(10)), None)
        );
        assert_eq!(
            apply(
                slow(4, Some(10), Some(10)),
                at(50, 100),
                Some(500),
                3,
                false,
                true
            ),
            (Slowness::NONE, None)
        );
    }
}
