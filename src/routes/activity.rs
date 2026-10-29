use crate::error::AppError;
use crate::models::Session;
use crate::state::AppState;
use crate::views::activity::{Filter, IndexTemplate};
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Extension, Router};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/activity", get(index))
}

/// Every field as text: a malformed value falls back to its default instead of a 400.
#[derive(Deserialize)]
struct ActivityQuery {
    #[serde(rename = "type")]
    kind: Option<String>,
    monitor: Option<String>,
    days: Option<String>,
    before: Option<String>,
}

async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    query: Result<Query<ActivityQuery>, QueryRejection>,
) -> Result<IndexTemplate, AppError> {
    let filter = match query {
        Ok(Query(q)) => Filter::parse(
            q.kind.as_deref(),
            q.monitor.as_deref(),
            q.days.as_deref(),
            q.before.as_deref(),
        ),
        Err(_) => Filter::parse(None, None, None, None),
    };
    IndexTemplate::load(&state.db, filter, session.csrf_token).await
}
