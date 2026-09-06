use askama::Template;
use askama_web::WebTemplate;

#[derive(Template, WebTemplate)]
#[template(path = "monitors/index.html")]
pub struct MonitorIndexTemplate {}
