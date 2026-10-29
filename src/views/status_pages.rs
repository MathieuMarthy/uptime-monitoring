use crate::error::AppError;
use crate::forms::monitor::FieldErrors;
use crate::forms::status_page::StatusPageForm;
use crate::models::{
    Incident, MonitorStatus, MonitorSummary, Settings, StatusPage, StatusPageSummary,
};
use crate::time::{DAY_MS, format_datetime, format_duration, now_ms};
use crate::views::heartbeat::{DAYS, DailyHistory};
use crate::views::monitors::{status_class, status_label};
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
    pub any_down: bool,
    pub monitors: Vec<PublicMonitor>,
    pub incidents: Vec<PublicIncident>,
    pub timezone: String,
}

pub struct PublicMonitor {
    pub name: String,
    pub status_label: &'static str,
    pub status_class: &'static str,
    pub daily: DailyHistory,
}

pub struct PublicIncident {
    pub monitor: String,
    pub started: String,
    /// `None` while the incident is open.
    pub duration: Option<String>,
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
        let incidents =
            Incident::recent_for_monitors(pool, &ids, now - DAYS * DAY_MS, RECENT_INCIDENTS)
                .await?
                .into_iter()
                .map(|i| PublicIncident {
                    monitor: names
                        .get(&i.monitor_id)
                        .copied()
                        .unwrap_or_default()
                        .to_string(),
                    started: format_datetime(&tz, i.started_at),
                    duration: i.resolved_at.map(|at| format_duration(at - i.started_at)),
                })
                .collect();

        let any_down = shown
            .iter()
            .any(|m| !m.paused && m.status == MonitorStatus::Down);
        let monitors = shown
            .into_iter()
            .map(|m| PublicMonitor {
                status_label: status_label(m.paused, m.status),
                status_class: status_class(m.paused, m.status),
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
            any_down,
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
