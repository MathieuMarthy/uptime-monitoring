use crate::forms::monitor::FieldErrors;
use askama::Template;
use askama_web::WebTemplate;

/// The setup page: the account form when the URL carries the right token, otherwise a pointer
/// to the server logs.
#[derive(Template, WebTemplate)]
#[template(path = "auth/setup.html")]
pub struct SetupTemplate {
    pub token: Option<String>,
    pub username: String,
    pub errors: FieldErrors,
}

impl SetupTemplate {
    pub fn error(&self, field: &str) -> Option<&str> {
        self.errors.get(field).map(String::as_str)
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "auth/login.html")]
pub struct LoginTemplate {
    pub username: String,
    pub error: Option<&'static str>,
}
