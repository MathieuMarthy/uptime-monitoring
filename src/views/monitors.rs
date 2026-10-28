use crate::error::AppError;
use crate::forms::monitor::{FieldErrors, METHODS, MonitorForm};
use crate::models::{Monitor, MonitorState, MonitorStatus};
use askama::Template;
use askama_web::WebTemplate;
use sqlx::SqlitePool;
use std::collections::HashMap;

/// A monitor's configuration together with its runtime state.
pub struct MonitorView {
    pub monitor: Monitor,
    pub state: MonitorState,
}

impl MonitorView {
    pub async fn load_all(pool: &SqlitePool) -> Result<Vec<Self>, AppError> {
        let mut states: HashMap<i64, MonitorState> = MonitorState::find_all(pool)
            .await?
            .into_iter()
            .map(|s| (s.monitor_id, s))
            .collect();

        Monitor::find_all(pool)
            .await?
            .into_iter()
            .map(|monitor| {
                let state = states
                    .remove(&monitor.id)
                    .ok_or_else(|| missing_state(monitor.id))?;
                Ok(Self { monitor, state })
            })
            .collect()
    }

    pub async fn load(pool: &SqlitePool, id: i64) -> Result<Self, AppError> {
        let monitor = Monitor::find(pool, id).await?.ok_or(AppError::NotFound)?;
        let state = MonitorState::find(pool, id)
            .await?
            .ok_or_else(|| missing_state(id))?;
        Ok(Self { monitor, state })
    }

    pub fn status_label(&self) -> &'static str {
        if self.monitor.paused {
            return "Paused";
        }
        match self.state.status {
            MonitorStatus::Pending => "Pending",
            MonitorStatus::Up => "Up",
            MonitorStatus::Down => "Down",
        }
    }

    pub fn status_class(&self) -> &'static str {
        if self.monitor.paused {
            return "text-blue";
        }
        match self.state.status {
            MonitorStatus::Pending => "text-muted",
            MonitorStatus::Up => "text-green",
            MonitorStatus::Down => "text-red",
        }
    }
}

fn missing_state(monitor_id: i64) -> AppError {
    AppError::Internal(format!("monitor {monitor_id} has no monitor_state row").into())
}

#[derive(Template, WebTemplate)]
#[template(path = "monitors/index.html")]
pub struct IndexTemplate {
    pub monitors: Vec<MonitorView>,
    pub up: usize,
    pub down: usize,
    pub pending: usize,
    pub paused: usize,
    pub csrf_token: String,
}

impl IndexTemplate {
    pub fn new(monitors: Vec<MonitorView>, csrf_token: String) -> Self {
        let active_with = |status| {
            monitors
                .iter()
                .filter(|v| !v.monitor.paused && v.state.status == status)
                .count()
        };
        let up = active_with(MonitorStatus::Up);
        let down = active_with(MonitorStatus::Down);
        let pending = active_with(MonitorStatus::Pending);
        let paused = monitors.iter().filter(|v| v.monitor.paused).count();

        Self {
            monitors,
            up,
            down,
            pending,
            paused,
            csrf_token,
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "monitors/detail.html")]
pub struct DetailTemplate {
    pub view: MonitorView,
    pub csrf_token: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "monitors/form.html")]
pub struct FormTemplate {
    pub form: MonitorForm,
    pub errors: FieldErrors,
    pub csrf_token: String,
    pub action_url: String,
    pub cancel_url: String,
    pub monitor_id: Option<i64>,
}

impl FormTemplate {
    pub fn is_edit(&self) -> bool {
        self.monitor_id.is_some()
    }

    pub fn error(&self, field: &str) -> Option<&str> {
        self.errors.get(field).map(String::as_str)
    }

    pub fn methods(&self) -> &'static [&'static str] {
        &METHODS
    }
}
