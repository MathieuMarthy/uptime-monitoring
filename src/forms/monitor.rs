use crate::forms::has_visible_char;
use crate::models::Monitor;
use serde::Deserialize;
use std::collections::BTreeMap;

pub const METHODS: [&str; 6] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];

const NAME_MAX_CHARS: usize = 100;
const URL_MAX_BYTES: usize = 2048;
const BODY_MAX_BYTES: usize = 64 * 1024;
const KEYWORD_MAX_BYTES: usize = 1024;

/// Validation messages keyed by form field name.
pub type FieldErrors = BTreeMap<&'static str, String>;

/// Raw monitor form, as submitted. Every field is a string so that a blank or
/// malformed number becomes a field error instead of a deserialization failure.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MonitorForm {
    pub name: String,
    pub url: String,
    pub method: String,
    pub accepted_statuses: String,
    pub interval_s: String,
    pub retry_interval_s: String,
    pub timeout_s: String,
    pub failure_threshold: String,
    pub tls_expiry_warn_days: String,
    pub headers: String,
    pub body: String,
    pub keyword: String,
    pub follow_redirects: Option<String>,
    pub ignore_tls_errors: Option<String>,
    pub active: Option<String>,
    /// Every other field. The checkbox of channel 3 is `channel_3`: a form cannot send a list.
    #[serde(flatten)]
    pub other: BTreeMap<String, String>,
}

const CHANNEL_PREFIX: &str = "channel_";

/// A validated monitor configuration, ready to be stored.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInput {
    pub name: String,
    pub url: String,
    pub method: String,
    pub headers: Option<String>,
    pub body: Option<String>,
    pub keyword: Option<String>,
    pub accepted_statuses: String,
    pub follow_redirects: bool,
    pub ignore_tls_errors: bool,
    pub interval_s: i64,
    pub retry_interval_s: i64,
    pub timeout_s: i64,
    pub failure_threshold: i64,
    pub tls_expiry_warn_days: Option<i64>,
    pub paused: bool,
    /// The notification channels its alerts go to.
    pub channel_ids: Vec<i64>,
}

impl MonitorForm {
    pub fn new_defaults() -> Self {
        Self {
            name: String::new(),
            url: "https://".to_string(),
            method: "GET".to_string(),
            accepted_statuses: "200-299".to_string(),
            interval_s: "60".to_string(),
            retry_interval_s: "20".to_string(),
            timeout_s: "10".to_string(),
            failure_threshold: "3".to_string(),
            tls_expiry_warn_days: "14".to_string(),
            headers: String::new(),
            body: String::new(),
            keyword: String::new(),
            follow_redirects: Some("on".to_string()),
            ignore_tls_errors: None,
            active: Some("on".to_string()),
            other: BTreeMap::new(),
        }
    }

    pub fn from_monitor(m: &Monitor) -> Self {
        let checkbox = |on: bool| on.then(|| "on".to_string());
        Self {
            name: m.name.clone(),
            url: m.url.clone(),
            method: m.method.clone(),
            accepted_statuses: m.accepted_statuses.clone(),
            interval_s: m.interval_s.to_string(),
            retry_interval_s: m.retry_interval_s.to_string(),
            timeout_s: m.timeout_s.to_string(),
            failure_threshold: m.failure_threshold.to_string(),
            tls_expiry_warn_days: m
                .tls_expiry_warn_days
                .map(|d| d.to_string())
                .unwrap_or_default(),
            headers: m.headers.clone().unwrap_or_default(),
            body: m.body.clone().unwrap_or_default(),
            keyword: m.keyword.clone().unwrap_or_default(),
            follow_redirects: checkbox(m.follow_redirects),
            ignore_tls_errors: checkbox(m.ignore_tls_errors),
            active: checkbox(!m.paused),
            other: BTreeMap::new(),
        }
    }

    /// Ids of the checked channels.
    pub fn channel_ids(&self) -> Vec<i64> {
        self.other
            .keys()
            .filter_map(|key| key.strip_prefix(CHANNEL_PREFIX)?.parse().ok())
            .filter(|id| *id > 0)
            .collect()
    }

    pub fn has_channel(&self, id: i64) -> bool {
        self.other.contains_key(&format!("{CHANNEL_PREFIX}{id}"))
    }

    /// Checks exactly these channels.
    pub fn set_channels(&mut self, ids: &[i64]) {
        self.other.retain(|key, _| !key.starts_with(CHANNEL_PREFIX));
        for id in ids {
            self.other
                .insert(format!("{CHANNEL_PREFIX}{id}"), "on".to_string());
        }
    }

    pub fn validate(&self) -> Result<MonitorInput, FieldErrors> {
        let mut errors = FieldErrors::new();

        let name = self.name.trim();
        if !has_visible_char(name) {
            errors.insert("name", "Name is required.".to_string());
        } else if name.chars().count() > NAME_MAX_CHARS {
            errors.insert(
                "name",
                format!("Name must be at most {NAME_MAX_CHARS} characters."),
            );
        }

        let url = self.url.trim();
        if let Err(message) = validate_url(url) {
            errors.insert("url", message);
        }

        let method = self.method.trim().to_ascii_uppercase();
        if !METHODS.contains(&method.as_str()) {
            errors.insert("method", "Choose a supported HTTP method.".to_string());
        }

        let accepted_statuses = match parse_status_ranges(&self.accepted_statuses) {
            Ok(ranges) => format_status_ranges(&ranges),
            Err(message) => {
                errors.insert("accepted_statuses", message);
                String::new()
            }
        };

        let interval_s = parse_bounded(&self.interval_s, 5, 86_400)
            .map_err(|m| errors.insert("interval_s", m))
            .ok();

        let retry_interval_s = parse_bounded(&self.retry_interval_s, 5, 86_400)
            .map_err(|m| errors.insert("retry_interval_s", m))
            .ok();
        if let (Some(interval), Some(retry)) = (interval_s, retry_interval_s)
            && retry > interval
        {
            errors.insert(
                "retry_interval_s",
                "Retry interval cannot be longer than the check interval.".to_string(),
            );
        }

        let timeout_s = parse_bounded(&self.timeout_s, 1, 120)
            .map_err(|m| errors.insert("timeout_s", m))
            .ok();
        if let (Some(interval), Some(timeout)) = (interval_s, timeout_s)
            && timeout > interval
        {
            errors.insert(
                "timeout_s",
                "Timeout cannot be longer than the check interval.".to_string(),
            );
        }

        let failure_threshold = parse_bounded(&self.failure_threshold, 1, 10)
            .map_err(|m| errors.insert("failure_threshold", m))
            .ok();

        let tls_expiry_warn_days = if self.tls_expiry_warn_days.trim().is_empty() {
            None
        } else {
            parse_bounded(&self.tls_expiry_warn_days, 1, 365)
                .map_err(|m| errors.insert("tls_expiry_warn_days", m))
                .ok()
        };

        let headers = non_empty(&self.headers);
        if let Some(headers) = &headers
            && let Err(message) = validate_headers(headers)
        {
            errors.insert("headers", message);
        }

        // Kept as typed: leading or trailing whitespace may matter to the target.
        let body = has_visible_char(&self.body).then(|| self.body.clone());
        if body.as_ref().is_some_and(|b| b.len() > BODY_MAX_BYTES) {
            errors.insert("body", "Request body must be at most 64 KiB.".to_string());
        }

        let keyword = non_empty(&self.keyword);
        if keyword
            .as_ref()
            .is_some_and(|k| k.len() > KEYWORD_MAX_BYTES)
        {
            errors.insert("keyword", "Keyword must be at most 1 KiB.".to_string());
        } else if keyword.is_some() && method == "HEAD" {
            errors.insert(
                "keyword",
                "A HEAD response has no body: choose another method or remove the keyword."
                    .to_string(),
            );
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(MonitorInput {
            name: name.to_string(),
            url: url.to_string(),
            method,
            headers,
            body,
            keyword,
            accepted_statuses,
            follow_redirects: self.follow_redirects.is_some(),
            ignore_tls_errors: self.ignore_tls_errors.is_some(),
            // Unwraps are safe: each `None` above recorded an error and returned early.
            interval_s: interval_s.unwrap(),
            retry_interval_s: retry_interval_s.unwrap(),
            timeout_s: timeout_s.unwrap(),
            failure_threshold: failure_threshold.unwrap(),
            tls_expiry_warn_days,
            paused: self.active.is_none(),
            channel_ids: self.channel_ids(),
        })
    }
}

/// Parses `"200-299,301"` into inclusive ranges. Every code must be 100–599.
pub fn parse_status_ranges(raw: &str) -> Result<Vec<(u16, u16)>, String> {
    let invalid =
        || "Use status codes or ranges between 100 and 599, e.g. 200-299,301.".to_string();
    let code = |s: &str| -> Result<u16, String> {
        let n: u16 = s.trim().parse().map_err(|_| invalid())?;
        if (100..=599).contains(&n) {
            Ok(n)
        } else {
            Err(invalid())
        }
    };

    let mut ranges = Vec::new();
    for part in raw.split(',') {
        let range = match part.split_once('-') {
            Some((low, high)) => (code(low)?, code(high)?),
            None => {
                let n = code(part)?;
                (n, n)
            }
        };
        if range.0 > range.1 {
            return Err(invalid());
        }
        ranges.push(range);
    }
    Ok(ranges)
}

/// Canonical form of parsed ranges: `200-299,301`.
fn format_status_ranges(ranges: &[(u16, u16)]) -> String {
    ranges
        .iter()
        .map(|&(low, high)| {
            if low == high {
                low.to_string()
            } else {
                format!("{low}-{high}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_bounded(raw: &str, min: i64, max: i64) -> Result<i64, String> {
    match raw.trim().parse::<i64>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(format!("Enter a whole number between {min} and {max}.")),
    }
}

fn non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(crate) fn validate_url(raw: &str) -> Result<(), String> {
    if raw.len() > URL_MAX_BYTES {
        return Err(format!("URL must be at most {URL_MAX_BYTES} characters."));
    }
    let parsed = url::Url::parse(raw)
        .map_err(|_| "Enter a valid URL, e.g. https://example.com/health.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL must start with http:// or https://.".to_string());
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("URL must include a host, e.g. https://example.com.".to_string());
    }
    Ok(())
}

fn validate_headers(raw: &str) -> Result<(), String> {
    let HeaderPairs(pairs) = serde_json::from_str(raw).map_err(|_| {
        r#"Headers must be a JSON object, e.g. {"Authorization": "Bearer token"}."#.to_string()
    })?;
    let mut seen = std::collections::HashSet::new();
    for (name, value) in &pairs {
        let header = http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("{name:?} is not a valid header name."))?;
        if !seen.insert(header) {
            return Err(format!("{name:?} appears more than once."));
        }
        let value = value
            .as_str()
            .ok_or_else(|| format!("The value of {name:?} must be a string."))?;
        http::HeaderValue::from_str(value)
            .map_err(|_| format!("The value of {name:?} is not a valid header value."))?;
    }
    Ok(())
}

/// A JSON object read as its entries in order, duplicates included
/// (`serde_json::Map` would silently keep only the last one).
struct HeaderPairs(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for HeaderPairs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PairsVisitor;

        impl<'de> serde::de::Visitor<'de> for PairsVisitor {
            type Value = HeaderPairs;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<HeaderPairs, A::Error> {
                let mut pairs = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    pairs.push(entry);
                }
                Ok(HeaderPairs(pairs))
            }
        }

        deserializer.deserialize_map(PairsVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> MonitorForm {
        MonitorForm {
            name: "API".to_string(),
            url: "https://example.com/health".to_string(),
            ..MonitorForm::new_defaults()
        }
    }

    fn errors_of(form: MonitorForm) -> FieldErrors {
        form.validate().expect_err("form should be invalid")
    }

    fn assert_field_error(form: MonitorForm, field: &str) {
        let errors = errors_of(form);
        assert!(
            errors.contains_key(field),
            "expected error on {field}, got {errors:?}"
        );
    }

    #[test]
    fn defaults_with_name_and_url_are_valid() {
        let i = valid().validate().unwrap();
        assert_eq!(
            (i.interval_s, i.timeout_s, i.failure_threshold),
            (60, 10, 3)
        );
        assert_eq!(i.method, "GET");
        assert_eq!(i.accepted_statuses, "200-299");
        assert_eq!(i.tls_expiry_warn_days, Some(14));
        assert!(i.follow_redirects && !i.paused);
        assert_eq!((i.headers, i.body, i.keyword), (None, None, None));
    }

    #[test]
    fn values_are_trimmed() {
        let i = MonitorForm {
            name: "  API  ".to_string(),
            url: " https://example.com ".to_string(),
            interval_s: " 60 ".to_string(),
            keyword: "  ok ".to_string(),
            ..valid()
        }
        .validate()
        .unwrap();
        assert_eq!(i.name, "API");
        assert_eq!(i.url, "https://example.com");
        assert_eq!(i.interval_s, 60);
        assert_eq!(i.keyword.as_deref(), Some("ok"));
    }

    #[test]
    fn name_rules() {
        assert_field_error(
            MonitorForm {
                name: "".into(),
                ..valid()
            },
            "name",
        );
        assert_field_error(
            MonitorForm {
                name: "   ".into(),
                ..valid()
            },
            "name",
        );
        assert_field_error(
            MonitorForm {
                name: "é".repeat(101),
                ..valid()
            },
            "name",
        );
        assert!(
            MonitorForm {
                name: "é".repeat(100),
                ..valid()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn url_rules() {
        let too_long = format!("https://example.com/{}", "a".repeat(2049 - 20));
        assert_eq!(too_long.len(), 2049);
        for bad in ["ftp://x", "https://", "not a url", too_long.as_str()] {
            assert_field_error(
                MonitorForm {
                    url: bad.into(),
                    ..valid()
                },
                "url",
            );
        }
    }

    #[test]
    fn method_must_be_known() {
        assert_field_error(
            MonitorForm {
                method: "TRACE".into(),
                ..valid()
            },
            "method",
        );
        for ok in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"] {
            assert!(
                MonitorForm {
                    method: ok.into(),
                    ..valid()
                }
                .validate()
                .is_ok(),
                "{ok}"
            );
        }
    }

    #[test]
    fn interval_bounds() {
        for bad in ["4", "86401", "", "abc"] {
            assert_field_error(
                MonitorForm {
                    interval_s: bad.into(),
                    ..valid()
                },
                "interval_s",
            );
        }
        for ok in ["10", "86400"] {
            assert!(
                MonitorForm {
                    interval_s: ok.into(),
                    retry_interval_s: "5".into(),
                    ..valid()
                }
                .validate()
                .is_ok(),
                "{ok}"
            );
        }
        let five = MonitorForm {
            interval_s: "5".into(),
            retry_interval_s: "5".into(),
            timeout_s: "5".into(),
            ..valid()
        };
        assert!(five.validate().is_ok());
    }

    #[test]
    fn timeout_bounds() {
        for bad in ["0", "121"] {
            assert_field_error(
                MonitorForm {
                    interval_s: "300".into(),
                    timeout_s: bad.into(),
                    ..valid()
                },
                "timeout_s",
            );
        }
        assert_field_error(
            MonitorForm {
                interval_s: "30".into(),
                timeout_s: "31".into(),
                ..valid()
            },
            "timeout_s",
        );
        let equal = MonitorForm {
            interval_s: "30".into(),
            timeout_s: "30".into(),
            ..valid()
        };
        assert!(equal.validate().is_ok());
    }

    #[test]
    fn failure_threshold_bounds() {
        for bad in ["0", "11"] {
            assert_field_error(
                MonitorForm {
                    failure_threshold: bad.into(),
                    ..valid()
                },
                "failure_threshold",
            );
        }
        for ok in ["1", "10"] {
            assert!(
                MonitorForm {
                    failure_threshold: ok.into(),
                    ..valid()
                }
                .validate()
                .is_ok()
            );
        }
    }

    #[test]
    fn tls_days_blank_disables() {
        let i = MonitorForm {
            tls_expiry_warn_days: " ".into(),
            ..valid()
        }
        .validate()
        .unwrap();
        assert_eq!(i.tls_expiry_warn_days, None);
        for bad in ["0", "366", "x"] {
            assert_field_error(
                MonitorForm {
                    tls_expiry_warn_days: bad.into(),
                    ..valid()
                },
                "tls_expiry_warn_days",
            );
        }
        for ok in ["1", "365"] {
            assert!(
                MonitorForm {
                    tls_expiry_warn_days: ok.into(),
                    ..valid()
                }
                .validate()
                .is_ok()
            );
        }
    }

    #[test]
    fn status_ranges() {
        assert_eq!(
            parse_status_ranges(" 200-299 , 301 ").unwrap(),
            vec![(200, 299), (301, 301)]
        );
        for bad in ["", "299-200", "99", "600", "2xx", "200-", ","] {
            assert!(parse_status_ranges(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn accepted_statuses_are_normalized() {
        let i = MonitorForm {
            accepted_statuses: " 200 , 301 ".into(),
            ..valid()
        }
        .validate()
        .unwrap();
        assert_eq!(i.accepted_statuses, "200,301");
        assert_field_error(
            MonitorForm {
                accepted_statuses: "2xx".into(),
                ..valid()
            },
            "accepted_statuses",
        );
    }

    #[test]
    fn headers_must_be_object_of_valid_headers() {
        for ok in ["", "{}", r#"{"Authorization":"Bearer x"}"#] {
            assert!(
                MonitorForm {
                    headers: ok.into(),
                    ..valid()
                }
                .validate()
                .is_ok(),
                "{ok}"
            );
        }
        for bad in [
            "[]",
            "nope",
            r#"{"Bad Name":"x"}"#,
            r#"{"X":"a\nb"}"#,
            r#"{"X":1}"#,
        ] {
            assert_field_error(
                MonitorForm {
                    headers: bad.into(),
                    ..valid()
                },
                "headers",
            );
        }
    }

    #[test]
    fn body_and_keyword_size_limits() {
        assert!(
            MonitorForm {
                body: "a".repeat(64 * 1024),
                ..valid()
            }
            .validate()
            .is_ok()
        );
        assert_field_error(
            MonitorForm {
                body: "a".repeat(64 * 1024 + 1),
                ..valid()
            },
            "body",
        );
        assert!(
            MonitorForm {
                keyword: "a".repeat(1024),
                ..valid()
            }
            .validate()
            .is_ok()
        );
        assert_field_error(
            MonitorForm {
                keyword: "a".repeat(1025),
                ..valid()
            },
            "keyword",
        );
    }

    #[test]
    fn retry_interval_and_tls_defaults() {
        let i = valid().validate().unwrap();
        assert_eq!(i.retry_interval_s, 20);
        assert_eq!(i.failure_threshold, 3);
        assert!(!i.ignore_tls_errors);
        let i = MonitorForm {
            ignore_tls_errors: Some("on".into()),
            ..valid()
        }
        .validate()
        .unwrap();
        assert!(i.ignore_tls_errors);
    }

    #[test]
    fn retry_interval_bounds() {
        for bad in ["4", "61", "", "x"] {
            assert_field_error(
                MonitorForm {
                    retry_interval_s: bad.into(),
                    ..valid()
                },
                "retry_interval_s",
            );
        }
        for ok in ["5", "60"] {
            assert!(
                MonitorForm {
                    retry_interval_s: ok.into(),
                    ..valid()
                }
                .validate()
                .is_ok(),
                "{ok}"
            );
        }
    }

    #[test]
    fn accepted_statuses_are_canonical() {
        let i = MonitorForm {
            accepted_statuses: "0200-0299, 301-301".into(),
            ..valid()
        }
        .validate()
        .unwrap();
        assert_eq!(i.accepted_statuses, "200-299,301");
    }

    #[test]
    fn duplicate_header_names_are_rejected() {
        assert_field_error(
            MonitorForm {
                headers: r#"{"X-Token":"a","x-token":"b"}"#.into(),
                ..valid()
            },
            "headers",
        );
    }

    #[test]
    fn body_is_kept_as_typed() {
        let i = MonitorForm {
            body: "  {\"a\": 1}\n".into(),
            ..valid()
        }
        .validate()
        .unwrap();
        assert_eq!(i.body.as_deref(), Some("  {\"a\": 1}\n"));
        let i = MonitorForm {
            body: " \n ".into(),
            ..valid()
        }
        .validate()
        .unwrap();
        assert_eq!(i.body, None);
    }

    #[test]
    fn keyword_needs_a_method_with_a_body() {
        assert_field_error(
            MonitorForm {
                method: "HEAD".into(),
                keyword: "OK".into(),
                ..valid()
            },
            "keyword",
        );
    }

    #[test]
    fn unchecked_boxes_mean_false() {
        let i = MonitorForm {
            follow_redirects: None,
            active: None,
            ..valid()
        }
        .validate()
        .unwrap();
        assert!(!i.follow_redirects);
        assert!(i.paused);
    }

    #[test]
    fn all_errors_reported_at_once() {
        let errors = errors_of(MonitorForm {
            name: "".into(),
            url: "x".into(),
            interval_s: "".into(),
            ..valid()
        });
        for field in ["name", "url", "interval_s"] {
            assert!(errors.contains_key(field), "{field}");
        }
    }
}
