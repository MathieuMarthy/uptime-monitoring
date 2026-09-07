use crate::models::monitor::Monitor;
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
