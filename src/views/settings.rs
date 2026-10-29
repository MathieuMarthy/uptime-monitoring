use crate::forms::monitor::FieldErrors;
use crate::forms::settings::{DataForm, timezones};
use crate::models::{Settings, StorageUse};
use askama::Template;
use askama_web::WebTemplate;

/// Bytes on disk per raw check and per daily stat, primary key included: measured with
/// `dbstat` on 100 monitors inserted in time order (30 and 34), rounded up.
pub const CHECK_BYTES: f64 = 32.0;
pub const DAILY_STAT_BYTES: f64 = 36.0;

#[derive(Template, WebTemplate)]
#[template(path = "settings/index.html")]
pub struct SettingsTemplate {
    pub csrf_token: String,
    pub timezones: Vec<String>,
    pub timezone: String,
    pub data: DataForm,
    pub errors: FieldErrors,
    /// Banner after a successful save.
    pub saved: Option<&'static str>,
    pub storage: StorageUse,
}

impl SettingsTemplate {
    /// The page showing the stored settings.
    pub fn new(settings: &Settings, storage: StorageUse, csrf_token: String) -> Self {
        Self {
            csrf_token,
            timezones: timezones(),
            timezone: settings.timezone.clone(),
            data: DataForm {
                checks_retention_days: settings.checks_retention_days.to_string(),
                history_retention_days: settings
                    .history_retention_days
                    .map(|days| days.to_string())
                    .unwrap_or_default(),
                block_private_targets: settings.block_private_targets.then(|| "on".to_string()),
            },
            errors: FieldErrors::new(),
            saved: None,
            storage,
        }
    }

    pub fn error(&self, field: &str) -> Option<&str> {
        self.errors.get(field).map(String::as_str)
    }

    pub fn checks_per_day(&self) -> i64 {
        self.storage.checks_per_day.round() as i64
    }

    pub fn checks_estimate(&self) -> String {
        match self.data.checks_retention_days.trim().parse::<f64>() {
            Ok(days) if days > 0.0 => {
                format_bytes(self.storage.checks_per_day * days * CHECK_BYTES)
            }
            _ => "—".to_string(),
        }
    }

    pub fn history_estimate(&self) -> String {
        let per_day = self.storage.monitors as f64 * DAILY_STAT_BYTES;
        match self.data.history_retention_days.trim() {
            "" => format!("{} per year, kept forever", format_bytes(per_day * 365.0)),
            raw => match raw.parse::<f64>() {
                Ok(days) if days > 0.0 => format_bytes(per_day * days),
                _ => "—".to_string(),
            },
        }
    }

    pub fn database_size(&self) -> String {
        format_bytes(self.storage.database_bytes as f64)
    }
}

/// `950 B`, `9.5 KiB`, `315 KiB`, `1.2 GiB`. `static/js/settings.js` formats the same way.
pub fn format_bytes(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024.0 {
        return format!("{} B", bytes.round());
    }
    let mut value = bytes / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{} {}", value.round(), UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_readable() {
        assert_eq!(format_bytes(0.0), "0 B");
        assert_eq!(format_bytes(1000.0), "1000 B");
        assert_eq!(format_bytes(9728.0), "9.5 KiB");
        assert_eq!(format_bytes(322_560.0), "315 KiB");
        assert_eq!(format_bytes(1.3 * 1024.0 * 1024.0 * 1024.0), "1.3 GiB");
    }

    #[test]
    fn estimates_follow_the_typed_values() {
        let settings = Settings {
            timezone: "UTC".to_string(),
            checks_retention_days: 7,
            history_retention_days: None,
            block_private_targets: false,
        };
        let storage = StorageUse {
            checks_per_day: 100.0 * 1440.0,
            monitors: 100,
            database_bytes: 0,
        };
        let mut page = SettingsTemplate::new(&settings, storage, String::new());
        // 144 000 checks × 7 days × 32 bytes ≈ 30.8 MiB.
        assert_eq!(page.checks_estimate(), "31 MiB");
        assert_eq!(page.history_estimate(), "1.3 MiB per year, kept forever");
        page.data.history_retention_days = "730".to_string();
        assert_eq!(page.history_estimate(), "2.5 MiB");
        page.data.checks_retention_days = "x".to_string();
        assert_eq!(page.checks_estimate(), "—");
    }
}
