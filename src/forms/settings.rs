use crate::forms::auth::check_new_password;
use crate::forms::monitor::FieldErrors;
use crate::i18n::{Lang, t, tf};
use serde::Deserialize;

pub const CHECKS_RETENTION_DAYS: std::ops::RangeInclusive<i64> = 2..=365;
/// About a century: beyond, leave the field blank to keep history forever.
pub const HISTORY_RETENTION_MAX_DAYS: i64 = 36_500;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GeneralForm {
    pub timezone: String,
    pub language: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DataForm {
    pub checks_retention_days: String,
    pub history_retention_days: String,
}

/// The private-target blocking checkbox, alone in its form.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BlockingForm {
    pub block_private_targets: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataInput {
    pub checks_retention_days: i64,
    pub history_retention_days: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PasswordForm {
    pub current_password: String,
    pub password: String,
    pub password_confirm: String,
}

/// Turns two-factor authentication on: the secret shown on the setup page and its current code.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TotpEnableForm {
    pub secret: String,
    pub current_password: String,
    pub code: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TotpDisableForm {
    pub current_password: String,
    pub code: String,
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
    /// The timezone, exactly as listed by `timezones()`, and the language.
    pub fn validate(&self) -> Result<(String, Lang), FieldErrors> {
        let mut errors = FieldErrors::new();
        let timezone = self.timezone.trim();
        if !jiff::tz::db()
            .available()
            .any(|name| name.as_str() == timezone)
        {
            errors.insert("timezone", t("Unknown timezone.").to_string());
        }
        // Blank: the language stays as it is.
        let language = match self.language.trim() {
            "" => Some(crate::i18n::current()),
            code => Lang::from_code(code),
        };
        if language.is_none() {
            errors.insert("language", t("Unknown language.").to_string());
        }
        match language {
            Some(language) if errors.is_empty() => Ok((timezone.to_string(), language)),
            _ => Err(errors),
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
                tf(
                    "Enter a number of days between {} and {}.",
                    &[&CHECKS_RETENTION_DAYS.start(), &CHECKS_RETENTION_DAYS.end()],
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
                        tf(
                            "Enter a number of days up to {}, or leave it blank.",
                            &[&HISTORY_RETENTION_MAX_DAYS],
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
                t("History must be kept at least the checks retention.").to_string(),
            );
        }

        match checks {
            Some(checks) if errors.is_empty() => Ok(DataInput {
                checks_retention_days: checks,
                history_retention_days: history,
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
                t("Enter your current password.").to_string(),
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
        }
    }

    #[test]
    fn timezone_must_be_listed() {
        let zones = timezones();
        assert!(zones.iter().any(|z| z == "Europe/Paris"));
        assert!(zones.is_sorted());
        let form = |tz: &str| GeneralForm {
            timezone: tz.to_string(),
            language: "en".to_string(),
        };
        assert_eq!(
            form(" UTC ").validate().unwrap(),
            ("UTC".to_string(), Lang::En)
        );
        for tz in ["", "europe/paris", "Mars/Olympus", "../etc"] {
            assert!(form(tz).validate().is_err(), "{tz:?}");
        }
    }

    #[test]
    fn retention_bounds() {
        let input = data(" 30 ", "").validate().unwrap();
        assert_eq!(input.checks_retention_days, 30);
        assert_eq!(input.history_retention_days, None);
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
