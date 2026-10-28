use crate::views::errors::ErrorTemplate;
use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

#[derive(Debug)]
pub enum AppError {
    NotFound,
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(Box::new(e))
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        Self::Internal(Box::new(e))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Page not found"),
            Self::Internal(e) => {
                tracing::error!(error = %e, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong. The error has been logged.",
                )
            }
        };

        let page = ErrorTemplate {
            status: status.as_u16(),
            message,
        };
        match page.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => status.into_response(),
        }
    }
}
