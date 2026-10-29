use crate::forms::auth::check_new_password;
use crate::forms::monitor::FieldErrors;
use serde::Deserialize;

pub const CHECKS_RETENTION_DAYS: std::ops::RangeInclusive<i64> = 2..=365;
/// About a century: beyond, leave the field blank to keep history forever.
pub const HISTORY_RETENTION_MAX_DAYS: i64 = 36_500;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GeneralForm {
    pub timezone: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DataForm {
    pub checks_retention_days: String,
    pub history_retention_days: String,
    pub block_private_targets: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataInput {
    pub checks_retention_days: i64,
    pub history_retention_days: Option<i64>,
    pub block_private_targets: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PasswordForm {
    pub current_password: String,
    pub password: String,
    pub password_confirm: String,
}

/// IANA names of the embedded tz database, sorted.
pub fn timezones() -> Vec<String> {
    let mut names: Vec<String> = jiff::tz::db()
        .available()
        .map(|name| name.as_str().to_string())
        .collect();
    names.sort_unstable();
    names
}

impl GeneralForm {
    /// The timezone, exactly as listed by `timezones()`.
    pub fn validate(&self) -> Result<String, FieldErrors> {
        let timezone = self.timezone.trim();
        if jiff::tz::db()
            .available()
            .any(|name| name.as_str() == timezone)
        {
            Ok(timezone.to_string())
        } else {
            let mut errors = FieldErrors::new();
            errors.insert("timezone", "Unknown timezone.".to_string());
            Err(errors)
        }
    }
}

impl DataForm {
    pub fn validate(&self) -> Result<DataInput, FieldErrors> {
        let mut errors = FieldErrors::new();

        let checks = self
            .checks_retention_days
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|days| CHECKS_RETENTION_DAYS.contains(days));
        if checks.is_none() {
            errors.insert(
                "checks_retention_days",
                format!(
                    "Enter a number of days between {} and {}.",
                    CHECKS_RETENTION_DAYS.start(),
                    CHECKS_RETENTION_DAYS.end()
                ),
            );
        }

        let history = match self.history_retention_days.trim() {
            "" => None,
            raw => match raw.parse::<i64>() {
                Ok(days) if (1..=HISTORY_RETENTION_MAX_DAYS).contains(&days) => Some(days),
                _ => {
                    errors.insert(
                        "history_retention_days",
                        format!(
                            "Enter a number of days up to {HISTORY_RETENTION_MAX_DAYS}, or leave it blank."
                        ),
                    );
                    None
                }
            },
        };
        if let (Some(checks), Some(history)) = (checks, history)
            && history < checks
        {
            errors.insert(
                "history_retention_days",
                "History must be kept at least the checks retention.".to_string(),
            );
        }

        match checks {
            Some(checks) if errors.is_empty() => Ok(DataInput {
                checks_retention_days: checks,
                history_retention_days: history,
                block_private_targets: self.block_private_targets.is_some(),
            }),
            _ => Err(errors),
        }
    }
}

impl PasswordForm {
    /// Checks the new password; the current one is verified by the route, against the hash.
    pub fn validate(&self) -> Result<(), FieldErrors> {
        let mut errors = FieldErrors::new();
        if self.current_password.is_empty() {
            errors.insert(
                "current_password",
                "Enter your current password.".to_string(),
            );
        }
        check_new_password(&self.password, &self.password_confirm, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(checks: &str, history: &str) -> DataForm {
        DataForm {
            checks_retention_days: checks.to_string(),
            history_retention_days: history.to_string(),
            block_private_targets: None,
        }
    }

    #[test]
    fn timezone_must_be_listed() {
        let zones = timezones();
        assert!(zones.iter().any(|z| z == "Europe/Paris"));
        assert!(zones.is_sorted());
        let form = |tz: &str| GeneralForm {
            timezone: tz.to_string(),
        };
        assert_eq!(form(" UTC ").validate().unwrap(), "UTC");
        for tz in ["", "europe/paris", "Mars/Olympus", "../etc"] {
            assert!(form(tz).validate().is_err(), "{tz:?}");
        }
    }

    #[test]
    fn retention_bounds() {
        let input = data(" 30 ", "").validate().unwrap();
        assert_eq!(input.checks_retention_days, 30);
        assert_eq!(input.history_retention_days, None);
        assert!(!input.block_private_targets);
        assert_eq!(
            data("7", "7").validate().unwrap().history_retention_days,
            Some(7)
        );
        for (checks, history) in [
            ("1", ""),
            ("366", ""),
            ("", ""),
            ("7", "6"),
            ("7", "0"),
            ("7", "36501"),
            ("7", "1e3"),
        ] {
            assert!(
                data(checks, history).validate().is_err(),
                "{checks} {history}"
            );
        }
    }

    #[test]
    fn password_change_needs_the_current_one() {
        let form = PasswordForm {
            current_password: String::new(),
            password: "a long password".to_string(),
            password_confirm: "a long password".to_string(),
        };
        assert!(
            form.validate()
                .unwrap_err()
                .contains_key("current_password")
        );
    }
}
