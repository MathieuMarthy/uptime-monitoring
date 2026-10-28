use crate::models::incident::Incident;
use crate::views::monitor_view::Monitor;
use crate::models::service::{Service, ServiceStatus};
use askama::Template;
use askama_web::WebTemplate;

#[derive(Template, WebTemplate)]
#[template(path = "monitors/index.html")]
pub struct MonitorIndexTemplate {
    pub(crate) monitors: Vec<Monitor>,
    pub(crate) up_count: i64,
    pub(crate) down_count: i64,
    pub(crate) paused_count: i64,
}

#[derive(Template, WebTemplate)]
#[template(path = "monitors/detail.html")]
pub struct MonitorDetailTemplate {
    pub(crate) monitor: Monitor,
    pub(crate) incidents: Vec<Incident>,
}

#[derive(Template, WebTemplate)]
#[template(path = "monitors/form.html")]
pub struct MonitorFormTemplate {
    pub(crate) service: Option<Service>,
    pub(crate) action_url: String,
    pub(crate) cancel_url: String,
    pub(crate) is_edit: bool,
    pub(crate) error_message: Option<String>,
}

impl MonitorFormTemplate {
    pub fn current_method(&self) -> &str {
        match &self.service {
            Some(s) => s.method.as_str(),
            None => "GET",
        }
    }

    pub fn is_enabled(&self) -> bool {
        match &self.service {
            Some(s) => s.is_enabled,
            None => true,
        }
    }
}
