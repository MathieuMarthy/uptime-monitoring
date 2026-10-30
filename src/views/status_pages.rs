use crate::error::AppError;
use crate::forms::monitor::FieldErrors;
use crate::forms::status_page::StatusPageForm;
use crate::models::{
    Incident, MonitorStatus, MonitorSummary, Settings, ShownMonitor, StatusPage, StatusPageSummary,
};
use crate::time::{DAY_MS, format_datetime, format_duration, now_ms};
use crate::views::heartbeat::{DAYS, DailyHistory};
use crate::views::monitors::{status_key, status_label};
use askama::Template;
use askama_web::WebTemplate;
use sqlx::SqlitePool;
use std::collections::HashMap;

/// Incidents listed on a public page.
const RECENT_INCIDENTS: i64 = 10;

#[derive(Template, WebTemplate)]
#[template(path = "status_pages/index.html")]
pub struct IndexTemplate {
    pub pages: Vec<StatusPageSummary>,
    pub csrf_token: String,
}

impl IndexTemplate {
    pub async fn load(pool: &SqlitePool, csrf_token: String) -> Result<Self, AppError> {
        Ok(Self {
            pages: StatusPage::summaries(pool).await?,
            csrf_token,
        })
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "status_pages/form.html")]
pub struct FormTemplate {
    pub form: StatusPageForm,
    pub errors: FieldErrors,
    pub csrf_token: String,
    pub action_url: String,
    pub page_id: Option<i64>,
    /// Every monitor, by name: the rows of the form.
    pub monitors: Vec<MonitorSummary>,
}

impl FormTemplate {
    pub fn is_edit(&self) -> bool {
        self.page_id.is_some()
    }

    pub fn error(&self, field: &str) -> Option<&str> {
        self.errors.get(field).map(String::as_str)
    }
}

/// The page visitors see, also shown to the admin as a preview.
#[derive(Template)]
#[template(path = "status_pages/public.html")]
pub struct PublicTemplate {
    pub title: String,
    pub description: Option<String>,
    /// Shown to the admin before the page is published.
    pub draft: bool,
    pub headline: PublicHeadline,
    pub monitors: Vec<PublicMonitor>,
    pub incidents: Vec<PublicIncident>,
    pub timezone: String,
}

/// The sentence on top of a public page.
pub struct PublicHeadline {
    /// Suffix of the lamp class.
    pub state: &'static str,
    pub text: String,
    pub detail: String,
}

impl PublicHeadline {
    /// `down` names the services down, in page order; `since` is when the only one went down.
    pub fn new(listed: usize, down: &[&str], since: Option<&str>) -> Self {
        let (state, text, detail) = match down {
            _ if listed == 0 => ("paused", "No service listed yet".to_string(), String::new()),
            [] => ("up", "All services are running".to_string(), String::new()),
            [name] => (
                "down",
                format!("{name} is down"),
                since.map_or_else(String::new, |at| format!("Unavailable since {at}.")),
            ),
            [first @ .., last] => (
                "down",
                format!("{} services are down", down.len()),
                format!("{} and {last}.", first.join(", ")),
            ),
        };
        Self {
            state,
            text,
            detail,
        }
    }
}

pub struct PublicMonitor {
    pub name: String,
    pub status_label: &'static str,
    pub status_key: &'static str,
    pub daily: DailyHistory,
}

pub struct PublicIncident {
    pub monitor_id: i64,
    pub monitor: String,
    pub started: String,
    /// `None` while the incident is open.
    pub duration: Option<String>,
    /// The admin's note, when made public.
    pub note: Option<String>,
}

impl PublicTemplate {
    /// Never shows a monitor's URL or errors: they can reveal internal hosts.
    pub async fn load(pool: &SqlitePool, page: &StatusPage) -> Result<Self, AppError> {
        let settings = Settings::load(pool).await?;
        let tz = settings.tz();
        let now = now_ms();
        let shown = StatusPage::shown_monitors(pool, page.id).await?;
        let ids: Vec<i64> = shown.iter().map(|m| m.monitor_id).collect();
        let mut daily = DailyHistory::load_many(pool, &ids, &tz, now).await?;

        let names: HashMap<i64, &str> = shown
            .iter()
            .map(|m| (m.monitor_id, m.name.as_str()))
            .collect();
        let incidents: Vec<PublicIncident> =
            Incident::recent_for_monitors(pool, &ids, now - DAYS * DAY_MS, RECENT_INCIDENTS)
                .await?
                .into_iter()
                .map(|i| PublicIncident {
                    monitor_id: i.monitor_id,
                    monitor: names
                        .get(&i.monitor_id)
                        .copied()
                        .unwrap_or_default()
                        .to_string(),
                    started: format_datetime(&tz, i.started_at),
                    duration: i.resolved_at.map(|at| format_duration(at - i.started_at)),
                    note: i.note.filter(|_| i.note_public),
                })
                .collect();

        let down: Vec<&ShownMonitor> = shown
            .iter()
            .filter(|m| !m.paused && m.status == MonitorStatus::Down)
            .collect();
        let since = match down.as_slice() {
            [only] => incidents
                .iter()
                .find(|i| i.monitor_id == only.monitor_id && i.duration.is_none())
                .map(|i| i.started.as_str()),
            _ => None,
        };
        let names: Vec<&str> = down.iter().map(|m| m.name.as_str()).collect();
        let headline = PublicHeadline::new(shown.len(), &names, since);
        let monitors = shown
            .into_iter()
            .map(|m| PublicMonitor {
                status_label: status_label(m.paused, m.status),
                status_key: status_key(m.paused, m.status),
                daily: daily
                    .remove(&m.monitor_id)
                    .expect("one history per monitor"),
                name: m.name,
            })
            .collect();

        Ok(Self {
            title: page.title.clone(),
            description: page.description.clone(),
            draft: !page.published,
            headline,
            monitors,
            incidents,
            timezone: tz.iana_name().unwrap_or("UTC").to_string(),
        })
    }

    pub fn render_html(&self) -> Result<String, AppError> {
        self.render()
            .map_err(|e| AppError::Internal(format!("public page render: {e}").into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_headline_names_what_is_down() {
        let h = PublicHeadline::new(3, &["Billing"], Some("14:21"));
        assert_eq!(h.text, "Billing is down");
        assert_eq!(h.detail, "Unavailable since 14:21.");
        let h = PublicHeadline::new(3, &["A", "B", "C"], None);
        assert_eq!(
            (h.text.as_str(), h.detail.as_str()),
            ("3 services are down", "A, B and C.")
        );
        assert_eq!(
            PublicHeadline::new(3, &[], None).text,
            "All services are running"
        );
        assert_eq!(
            PublicHeadline::new(0, &[], None).text,
            "No service listed yet"
        );
    }
}
