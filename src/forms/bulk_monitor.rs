//! Many HTTP monitors from a pasted list of URLs, one per line, with the add form's defaults.

use crate::forms::monitor::{
    MonitorForm, MonitorInput, channel_ids_in, clean_group, has_channel_in, set_channels_in,
};
use crate::models::name_key;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

/// About twice the instance's target: a paste beyond that is likely a mistake.
pub const BULK_MAX_URLS: usize = 200;
/// Room left under the 100-character name limit for a ` (12)` suffix.
const BASE_NAME_MAX_CHARS: usize = 90;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BulkMonitorForm {
    pub urls: String,
    pub group: String,
    /// The channel checkboxes (`channel_3`), as on the add form.
    #[serde(flatten)]
    pub other: BTreeMap<String, String>,
}

/// A pasted line that would not make a monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineError {
    /// Counted from 1, blank lines included, as the textarea shows them.
    pub line: usize,
    pub url: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BulkErrors {
    pub urls: Option<String>,
    pub group: Option<String>,
    pub lines: Vec<LineError>,
}

impl BulkMonitorForm {
    pub fn has_channel(&self, id: i64) -> bool {
        has_channel_in(&self.other, id)
    }

    pub fn set_channels(&mut self, ids: &[i64]) {
        set_channels_in(&mut self.other, ids);
    }

    /// One monitor per non-blank line, named after its host (without `www.`), numbered when the
    /// name is taken: `taken` holds the stored names' keys. A line without a scheme gets
    /// `https://`. Every line is checked; any error and nothing is returned.
    pub fn validate(&self, taken: &HashSet<String>) -> Result<Vec<MonitorInput>, BulkErrors> {
        let mut errors = BulkErrors::default();
        // Checked again with each line; reported once.
        if let Err(error) = clean_group(&self.group) {
            errors.group = Some(error.to_string());
        }

        let lines: Vec<(usize, &str)> = self
            .urls
            .lines()
            .enumerate()
            .map(|(i, line)| (i + 1, line.trim()))
            .filter(|(_, line)| !line.is_empty())
            .collect();
        if lines.is_empty() {
            errors.urls = Some("Paste at least one URL.".to_string());
        } else if lines.len() > BULK_MAX_URLS {
            errors.urls = Some(format!("At most {BULK_MAX_URLS} URLs at once."));
        }
        if errors.urls.is_some() {
            return Err(errors);
        }

        // Only the channel boxes: the other fields (CSRF token…) are not copied per line.
        let mut template = MonitorForm {
            group: self.group.clone(),
            ..MonitorForm::new_defaults()
        };
        set_channels_in(&mut template.other, &channel_ids_in(&self.other));
        let mut used = taken.clone();
        let mut inputs = Vec::with_capacity(lines.len());
        for (line, raw) in lines {
            let url = if has_scheme(raw) {
                raw.to_string()
            } else {
                format!("https://{raw}")
            };
            let name = free_name(&base_name(&url), &used);
            let form = MonitorForm {
                name: name.clone(),
                url,
                ..template.clone()
            };
            match form.validate() {
                Ok(input) => {
                    used.insert(name_key(&name));
                    inputs.push(input);
                }
                Err(mut field_errors) => {
                    field_errors.remove("group");
                    let Some(message) = field_errors
                        .remove("url")
                        .or_else(|| field_errors.into_values().next())
                    else {
                        continue;
                    };
                    errors.lines.push(LineError {
                        line,
                        url: raw.to_string(),
                        message,
                    });
                }
            }
        }
        if errors == BulkErrors::default() {
            Ok(inputs)
        } else {
            Err(errors)
        }
    }
}

/// Whether the line starts with `scheme://`; a `://` further on (`?next=https://…`) does not count.
fn has_scheme(line: &str) -> bool {
    line.split_once("://").is_some_and(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// The URL's host without `www.`, international letters shown (`café.fr`, not punycode); a URL
/// that does not parse gets a placeholder, as its line is refused anyway.
fn base_name(url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| idna::domain_to_unicode(h).0))
        .unwrap_or_default();
    let host = host
        .strip_prefix("www.")
        .filter(|h| !h.is_empty())
        .unwrap_or(&host);
    let name: String = host.chars().take(BASE_NAME_MAX_CHARS).collect();
    if name.is_empty() {
        "monitor".to_string()
    } else {
        name
    }
}

/// `base`, or `base (2)`, `base (3)`… whichever no monitor uses yet.
fn free_name(base: &str, used: &HashSet<String>) -> String {
    let mut name = base.to_string();
    let mut n = 1;
    while used.contains(&name_key(&name)) {
        n += 1;
        name = format!("{base} ({n})");
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(urls: &str) -> BulkMonitorForm {
        BulkMonitorForm {
            urls: urls.to_string(),
            ..BulkMonitorForm::default()
        }
    }

    #[test]
    fn names_follow_the_host() {
        assert_eq!(base_name("https://www.example.com/a"), "example.com");
        assert_eq!(base_name("http://10.0.0.1:8080"), "10.0.0.1");
        assert_eq!(base_name("https://[::1]/"), "[::1]");
        assert_eq!(base_name("nonsense"), "monitor");
        assert_eq!(
            base_name(&format!("https://{}.com", "a".repeat(120))).len(),
            90
        );
    }

    #[test]
    fn scheme_is_added_unless_the_line_starts_with_one() {
        let inputs = form("example.com/login?next=https://example.com/\nHTTP://a.example")
            .validate(&HashSet::new())
            .unwrap();
        assert_eq!(
            inputs[0].url,
            "https://example.com/login?next=https://example.com/"
        );
        assert_eq!(inputs[1].url, "HTTP://a.example");
    }

    #[test]
    fn international_hosts_keep_their_letters() {
        assert_eq!(base_name("https://www.café.fr/"), "café.fr");
    }

    #[test]
    fn taken_names_are_numbered_case_insensitively() {
        let used: HashSet<String> =
            ["example.com".to_string(), "example.com (2)".to_string()].into();
        assert_eq!(free_name("Example.com", &used), "Example.com (3)");
        assert_eq!(free_name("other", &used), "other");
    }

    #[test]
    fn line_numbers_count_blank_lines() {
        let errors = form("a.example\n\nhttps://\n")
            .validate(&HashSet::new())
            .unwrap_err();
        assert_eq!(errors.lines.len(), 1);
        assert_eq!(errors.lines[0].line, 3);
        assert_eq!(errors.lines[0].url, "https://");
    }

    #[test]
    fn same_host_twice_in_one_paste() {
        let inputs = form("a.example/x\na.example/y")
            .validate(&HashSet::new())
            .unwrap();
        let names: Vec<&str> = inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["a.example", "a.example (2)"]);
    }
}
