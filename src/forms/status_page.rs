use crate::forms::monitor::FieldErrors;
use crate::forms::{has_visible_char, text_len};
use serde::Deserialize;
use std::collections::BTreeMap;

const TITLE_MAX_CHARS: usize = 100;
const SLUG_MAX_CHARS: usize = 64;
const DESCRIPTION_MAX_CHARS: usize = 500;
const DISPLAY_NAME_MAX_CHARS: usize = 100;
const POSITION_MAX: i64 = 9999;

const MONITOR_PREFIX: &str = "monitor_";
const DISPLAY_NAME_PREFIX: &str = "display_name_";
const POSITION_PREFIX: &str = "position_";

/// Raw public page form, as submitted.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatusPageForm {
    pub title: String,
    pub slug: String,
    pub description: String,
    pub published: Option<String>,
    /// Every other field. Monitor 3 is shown when `monitor_3` is checked, under
    /// `display_name_3`, at `position_3`: a form cannot send a list.
    #[serde(flatten)]
    pub other: BTreeMap<String, String>,
}

/// A validated public page, ready to be stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPageInput {
    pub title: String,
    pub slug: String,
    pub description: Option<String>,
    pub published: bool,
    /// In display order.
    pub monitors: Vec<PageMonitorInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageMonitorInput {
    pub monitor_id: i64,
    /// `None` shows the monitor's own name.
    pub display_name: Option<String>,
}

impl StatusPageForm {
    /// The form of a stored page; positions are numbered from 1.
    pub fn from_page(
        title: &str,
        slug: &str,
        description: Option<&str>,
        published: bool,
        monitors: &[PageMonitorInput],
    ) -> Self {
        let mut other = BTreeMap::new();
        for (i, m) in monitors.iter().enumerate() {
            let id = m.monitor_id;
            other.insert(format!("{MONITOR_PREFIX}{id}"), "on".to_string());
            other.insert(
                format!("{DISPLAY_NAME_PREFIX}{id}"),
                m.display_name.clone().unwrap_or_default(),
            );
            other.insert(format!("{POSITION_PREFIX}{id}"), (i + 1).to_string());
        }
        Self {
            title: title.to_string(),
            slug: slug.to_string(),
            description: description.unwrap_or_default().to_string(),
            published: published.then(|| "on".to_string()),
            other,
        }
    }

    pub fn includes(&self, monitor_id: i64) -> bool {
        self.other
            .contains_key(&format!("{MONITOR_PREFIX}{monitor_id}"))
    }

    pub fn display_name(&self, monitor_id: i64) -> &str {
        self.field(DISPLAY_NAME_PREFIX, monitor_id)
    }

    pub fn position(&self, monitor_id: i64) -> &str {
        self.field(POSITION_PREFIX, monitor_id)
    }

    fn field(&self, prefix: &str, monitor_id: i64) -> &str {
        self.other
            .get(&format!("{prefix}{monitor_id}"))
            .map_or("", String::as_str)
    }

    /// `monitor_ids` lists the existing monitors in the order of the form: it breaks ties
    /// between equal or blank positions, and ids not in it are ignored.
    pub fn validate(&self, monitor_ids: &[i64]) -> Result<StatusPageInput, FieldErrors> {
        let mut errors = FieldErrors::new();

        let title = self.title.trim();
        if !has_visible_char(title) {
            errors.insert("title", "Title is required.".to_string());
        } else if text_len(title) > TITLE_MAX_CHARS {
            errors.insert(
                "title",
                format!("Title must be at most {TITLE_MAX_CHARS} characters."),
            );
        }

        let slug = self.slug.trim();
        if !valid_slug(slug) {
            errors.insert(
                "slug",
                format!(
                    "Up to {SLUG_MAX_CHARS} lowercase letters, digits and single hyphens, e.g. my-services."
                ),
            );
        }

        let description = self.description.trim();
        if text_len(description) > DESCRIPTION_MAX_CHARS {
            errors.insert(
                "description",
                format!("Description must be at most {DESCRIPTION_MAX_CHARS} characters."),
            );
        }

        let mut shown = Vec::new();
        for (order, &id) in monitor_ids.iter().enumerate() {
            if !self.includes(id) {
                continue;
            }
            let name = self.display_name(id).trim();
            if text_len(name) > DISPLAY_NAME_MAX_CHARS {
                add_error(
                    &mut errors,
                    format!("Display names must be at most {DISPLAY_NAME_MAX_CHARS} characters."),
                );
            }
            let position = match self.position(id).trim() {
                "" => POSITION_MAX + 1,
                raw => match raw.parse::<i64>() {
                    Ok(p) if (0..=POSITION_MAX).contains(&p) => p,
                    _ => {
                        add_error(
                            &mut errors,
                            format!("Order must be a whole number from 0 to {POSITION_MAX}."),
                        );
                        continue;
                    }
                },
            };
            let display_name = has_visible_char(name).then(|| name.to_string());
            shown.push((
                position,
                order,
                PageMonitorInput {
                    monitor_id: id,
                    display_name,
                },
            ));
        }
        shown.sort_by_key(|(position, order, _)| (*position, *order));

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(StatusPageInput {
            title: title.to_string(),
            slug: slug.to_string(),
            description: has_visible_char(description).then(|| description.to_string()),
            published: self.published.is_some(),
            monitors: shown.into_iter().map(|(_, _, m)| m).collect(),
        })
    }
}

/// The monitor rows share one error: each distinct problem is added once.
fn add_error(errors: &mut FieldErrors, message: String) {
    match errors.get_mut("monitors") {
        Some(existing) if existing.contains(&message) => {}
        Some(existing) => {
            existing.push(' ');
            existing.push_str(&message);
        }
        None => {
            errors.insert("monitors", message);
        }
    }
}

/// Lowercase ASCII letters and digits in groups joined by single hyphens.
fn valid_slug(slug: &str) -> bool {
    slug.len() <= SLUG_MAX_CHARS
        && slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(fields: &[(&str, &str)]) -> StatusPageForm {
        StatusPageForm {
            title: "Status".to_string(),
            slug: "status".to_string(),
            other: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..StatusPageForm::default()
        }
    }

    fn ids(input: &StatusPageInput) -> Vec<i64> {
        input.monitors.iter().map(|m| m.monitor_id).collect()
    }

    #[test]
    fn slug_is_lowercase_words_joined_by_hyphens() {
        for slug in ["a", "my-services", "2026", "x-1-y"] {
            assert!(valid_slug(slug), "{slug}");
        }
        let long = "a".repeat(SLUG_MAX_CHARS + 1);
        for slug in [
            "", "-a", "a-", "a--b", "My", "é", "a b", "a_b", "a/b", &long,
        ] {
            assert!(!valid_slug(slug), "{slug}");
        }
    }

    #[test]
    fn title_and_slug_are_required() {
        let mut f = form(&[]);
        f.title = "\u{200B}".to_string();
        f.slug = " ".to_string();
        let errors = f.validate(&[]).unwrap_err();
        assert!(errors.contains_key("title"));
        assert!(errors.contains_key("slug"));
    }

    #[test]
    fn trims_and_drops_blank_text() {
        let mut f = form(&[("monitor_1", "on"), ("display_name_1", "  ")]);
        f.title = " Status ".to_string();
        f.slug = " status ".to_string();
        f.description = " ".to_string();
        f.published = Some("on".to_string());
        let input = f.validate(&[1]).unwrap();
        assert_eq!(
            input,
            StatusPageInput {
                title: "Status".to_string(),
                slug: "status".to_string(),
                description: None,
                published: true,
                monitors: vec![PageMonitorInput {
                    monitor_id: 1,
                    display_name: None
                }],
            }
        );
    }

    #[test]
    fn orders_by_position_then_form_order() {
        let f = form(&[
            ("monitor_1", "on"),
            ("monitor_2", "on"),
            ("position_2", "5"),
            ("monitor_3", "on"),
            ("position_3", "1"),
            ("monitor_4", "on"),
            ("monitor_5", "on"),
            ("position_5", "5"),
            ("position_6", "0"),
            ("monitor_9", "on"),
        ]);
        // 6 is not checked, 9 no longer exists; 4 comes before 1 in the form.
        let input = f.validate(&[4, 1, 2, 3, 5, 6]).unwrap();
        assert_eq!(ids(&input), [3, 2, 5, 4, 1]);
    }

    #[test]
    fn rejects_bad_positions_and_long_names() {
        for (key, value) in [
            ("position_1", "-1"),
            ("position_1", "10000"),
            ("position_1", "1.5"),
            ("display_name_1", &"x".repeat(101)),
        ] {
            let f = form(&[("monitor_1", "on"), (key, value)]);
            assert!(
                f.validate(&[1]).unwrap_err().contains_key("monitors"),
                "{key}={value}"
            );
        }
        let f = form(&[
            ("monitor_1", "on"),
            ("display_name_1", &"x".repeat(101)),
            ("monitor_2", "on"),
            ("position_2", "-1"),
            ("monitor_3", "on"),
            ("position_3", "-1"),
        ]);
        let errors = f.validate(&[1, 2, 3]).unwrap_err();
        assert_eq!(
            errors["monitors"],
            "Display names must be at most 100 characters. Order must be a whole number from 0 to 9999.",
            "every distinct problem is reported once"
        );
        let f = form(&[("monitor_2", "on"), ("position_1", "x")]);
        assert!(f.validate(&[1, 2]).is_ok(), "unchecked rows are not read");
    }

    #[test]
    fn stored_page_round_trips() {
        let monitors = vec![
            PageMonitorInput {
                monitor_id: 7,
                display_name: Some("API".to_string()),
            },
            PageMonitorInput {
                monitor_id: 2,
                display_name: None,
            },
        ];
        let f =
            StatusPageForm::from_page("Status", "status", Some("Our services"), true, &monitors);
        assert_eq!(f.position(7), "1");
        assert_eq!(f.position(2), "2");
        assert_eq!(f.display_name(7), "API");
        assert!(!f.includes(3));
        let input = f.validate(&[2, 7]).unwrap();
        assert_eq!(input.monitors, monitors);
        assert_eq!(input.description.as_deref(), Some("Our services"));
        assert!(input.published);
    }
}
