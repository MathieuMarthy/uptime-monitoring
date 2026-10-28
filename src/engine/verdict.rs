//! The `pending` / `up` / `down` state machine. Pure: no I/O, no clock.

use crate::models::MonitorStatus;

/// Why a check failed. Stored as `checks.error_kind` and `incidents.error_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Timeout,
    Dns,
    /// The target resolves to an address refused by `block_private_targets`.
    Blocked,
    Connect,
    Tls,
    Status,
    Keyword,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Dns => "dns",
            Self::Blocked => "blocked",
            Self::Connect => "connect",
            Self::Tls => "tls",
            Self::Status => "status",
            Self::Keyword => "keyword",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: ErrorKind,
    /// Short human-readable message.
    pub message: String,
}

/// The result of one check, before it is recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub checked_at: i64,
    /// Time until the response headers arrived; `None` without a response.
    pub latency_ms: Option<i64>,
    pub status_code: Option<u16>,
    pub failure: Option<Failure>,
    /// `notAfter` of the server certificate, when one was seen.
    pub tls_expires_at: Option<i64>,
}

impl CheckOutcome {
    pub fn ok(&self) -> bool {
        self.failure.is_none()
    }
}

/// The part of `monitor_state` the state machine reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Health {
    pub status: MonitorStatus,
    pub consecutive_failures: i64,
    /// `checked_at` of the first failure of the current series.
    pub failing_since: Option<i64>,
}

impl Health {
    pub const PENDING: Self = Self {
        status: MonitorStatus::Pending,
        consecutive_failures: 0,
        failing_since: None,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The failure threshold was reached; the outage started at `since`.
    WentDown { since: i64 },
    /// A successful check ended an outage.
    Recovered,
}

/// Applies one check result to the monitor's health.
pub fn apply(
    health: Health,
    ok: bool,
    checked_at: i64,
    failure_threshold: i64,
) -> (Health, Option<Transition>) {
    if ok {
        let transition = (health.status == MonitorStatus::Down).then_some(Transition::Recovered);
        let up = Health {
            status: MonitorStatus::Up,
            consecutive_failures: 0,
            failing_since: None,
        };
        return (up, transition);
    }

    let since = health.failing_since.unwrap_or(checked_at);
    let failures = health.consecutive_failures + 1;
    let goes_down = health.status != MonitorStatus::Down && failures >= failure_threshold;
    let next = Health {
        status: if goes_down {
            MonitorStatus::Down
        } else {
            health.status
        },
        consecutive_failures: failures,
        failing_since: Some(since),
    };
    (next, goes_down.then_some(Transition::WentDown { since }))
}

/// Delay between the start of a check and the start of the next one: the retry interval while a
/// failure series has not yet reached the threshold, the normal interval otherwise.
pub fn next_delay_s(
    health: Health,
    failure_threshold: i64,
    interval_s: i64,
    retry_interval_s: i64,
) -> i64 {
    let retrying = health.status != MonitorStatus::Down
        && (1..failure_threshold).contains(&health.consecutive_failures);
    if retrying {
        retry_interval_s
    } else {
        interval_s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MonitorStatus::{Down, Pending, Up};

    fn health(status: MonitorStatus, failures: i64, since: Option<i64>) -> Health {
        Health {
            status,
            consecutive_failures: failures,
            failing_since: since,
        }
    }

    #[test]
    fn success_from_pending_is_up_without_transition() {
        assert_eq!(
            apply(Health::PENDING, true, 10, 3),
            (health(Up, 0, None), None)
        );
    }

    #[test]
    fn success_resets_a_failure_series() {
        assert_eq!(
            apply(health(Up, 2, Some(5)), true, 10, 3),
            (health(Up, 0, None), None)
        );
    }

    #[test]
    fn success_while_down_recovers() {
        assert_eq!(
            apply(health(Down, 4, Some(5)), true, 10, 3),
            (health(Up, 0, None), Some(Transition::Recovered))
        );
    }

    #[test]
    fn first_failure_below_threshold_starts_the_series() {
        assert_eq!(
            apply(health(Up, 0, None), false, 10, 3),
            (health(Up, 1, Some(10)), None)
        );
        assert_eq!(
            apply(Health::PENDING, false, 10, 3),
            (health(Pending, 1, Some(10)), None)
        );
    }

    #[test]
    fn reaching_the_threshold_goes_down_from_the_first_failure() {
        assert_eq!(
            apply(health(Up, 2, Some(5)), false, 30, 3),
            (
                health(Down, 3, Some(5)),
                Some(Transition::WentDown { since: 5 })
            )
        );
        assert_eq!(
            apply(Health::PENDING, false, 10, 1),
            (
                health(Down, 1, Some(10)),
                Some(Transition::WentDown { since: 10 })
            )
        );
    }

    #[test]
    fn lowered_threshold_already_exceeded_goes_down() {
        assert_eq!(
            apply(health(Up, 4, Some(5)), false, 30, 2).1,
            Some(Transition::WentDown { since: 5 })
        );
    }

    #[test]
    fn failure_while_down_only_counts() {
        assert_eq!(
            apply(health(Down, 3, Some(5)), false, 40, 3),
            (health(Down, 4, Some(5)), None)
        );
    }

    #[test]
    fn retry_interval_only_while_below_threshold() {
        assert_eq!(next_delay_s(health(Up, 0, None), 3, 60, 20), 60);
        assert_eq!(next_delay_s(health(Up, 1, Some(5)), 3, 60, 20), 20);
        assert_eq!(next_delay_s(health(Up, 2, Some(5)), 3, 60, 20), 20);
        assert_eq!(next_delay_s(health(Down, 3, Some(5)), 3, 60, 20), 60);
        assert_eq!(next_delay_s(health(Pending, 1, Some(5)), 3, 60, 20), 20);
    }
}
