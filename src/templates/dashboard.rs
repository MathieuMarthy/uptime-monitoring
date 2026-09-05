use askama::Template;
use askama_web::WebTemplate;

#[derive(Template, WebTemplate)]
#[template(path = "dashboard/index.html")]
pub struct DashboardIndexTemplate {}
