use crate::forms::has_visible_char;
use crate::forms::monitor::FieldErrors;
use serde::Deserialize;

const USERNAME_MAX_CHARS: usize = 64;
const PASSWORD_MIN_CHARS: usize = 12;
const PASSWORD_MAX_BYTES: usize = 1024;
const TIMEZONE_MAX_BYTES: usize = 64;
const DEFAULT_TIMEZONE: &str = "UTC";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SetupForm {
    pub token: String,
    pub username: String,
    pub password: String,
    pub password_confirm: String,
    pub timezone: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetupInput {
    pub username: String,
    pub password: String,
    pub timezone: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

impl SetupForm {
    pub fn validate(&self) -> Result<SetupInput, FieldErrors> {
        let mut errors = FieldErrors::new();

        let username = self.username.trim();
        if !has_visible_char(username) {
            errors.insert("username", "Username is required.".to_string());
        } else if username.chars().count() > USERNAME_MAX_CHARS {
            errors.insert(
                "username",
                format!("Username must be at most {USERNAME_MAX_CHARS} characters."),
            );
        } else if username.chars().any(char::is_control) {
            errors.insert(
                "username",
                "Username contains invalid characters.".to_string(),
            );
        }

        check_new_password(&self.password, &self.password_confirm, &mut errors);

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(SetupInput {
            username: username.to_string(),
            password: self.password.clone(),
            timezone: timezone_or_default(&self.timezone),
        })
    }
}

/// Rules of a new password (setup, change), reported on `password` and `password_confirm`.
pub fn check_new_password(password: &str, confirm: &str, errors: &mut FieldErrors) {
    if password.chars().count() < PASSWORD_MIN_CHARS {
        errors.insert(
            "password",
            format!("Password must be at least {PASSWORD_MIN_CHARS} characters."),
        );
    } else if password.len() > PASSWORD_MAX_BYTES {
        errors.insert(
            "password",
            format!("Password must be at most {PASSWORD_MAX_BYTES} bytes."),
        );
    } else if password != confirm {
        errors.insert("password_confirm", "Passwords do not match.".to_string());
    }
}

/// The timezone comes from the browser through a hidden field: anything that is not a known
/// IANA name (`Europe/Paris`, `Etc/GMT+1`) falls back to UTC. Settings can change it.
fn timezone_or_default(raw: &str) -> String {
    let raw = raw.trim();
    let looks_valid = !raw.is_empty()
        && raw.len() <= TIMEZONE_MAX_BYTES
        && raw.starts_with(|c: char| c.is_ascii_alphabetic())
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '+' | '-'))
        && jiff::tz::TimeZone::get(raw).is_ok();
    if looks_valid {
        raw.to_string()
    } else {
        DEFAULT_TIMEZONE.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(username: &str, password: &str, confirm: &str) -> SetupForm {
        SetupForm {
            username: username.to_string(),
            password: password.to_string(),
            password_confirm: confirm.to_string(),
            timezone: "Europe/Paris".to_string(),
            ..SetupForm::default()
        }
    }

    #[test]
    fn valid_form_trims_username_only() {
        let input = form("  admin ", " a long password ", " a long password ")
            .validate()
            .unwrap();
        assert_eq!(input.username, "admin");
        assert_eq!(input.password, " a long password ");
        assert_eq!(input.timezone, "Europe/Paris");
    }

    #[test]
    fn invalid_fields_are_all_reported() {
        let errors = form("\u{200B}", "short", "short").validate().unwrap_err();
        assert!(errors.contains_key("username"));
        assert!(errors.contains_key("password"));

        let errors = form("admin", "a long password", "a long passw0rd")
            .validate()
            .unwrap_err();
        assert!(errors.contains_key("password_confirm"));

        let long = "x".repeat(PASSWORD_MAX_BYTES + 1);
        assert!(form("admin", &long, &long).validate().is_err());
        assert!(
            form(&"a".repeat(65), "a long password", "a long password")
                .validate()
                .is_err()
        );
    }

    #[test]
    fn timezone_falls_back_to_utc() {
        for raw in [
            "",
            "<script>",
            "../etc",
            "Europe/Paris; x",
            "Mars/Olympus_Mons",
            &"A".repeat(65),
        ] {
            assert_eq!(timezone_or_default(raw), "UTC", "{raw:?}");
        }
        for raw in [
            "Europe/Paris",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+1",
            "UTC",
        ] {
            assert_eq!(timezone_or_default(raw), raw);
        }
    }
}
