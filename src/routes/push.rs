use crate::engine::verdict::{CheckOutcome, ErrorKind, Failure};
use crate::error::AppError;
use crate::models::Monitor;
use crate::state::AppState;
use crate::time::now_ms;
use axum::Router;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use serde::Deserialize;

/// Longest message a job can report, in characters.
const MESSAGE_MAX_CHARS: usize = 200;
/// Longer tokens are not looked up.
const TOKEN_MAX_LEN: usize = 128;

/// Public: the token is the credential. GET also answers HEAD.
pub fn router() -> Router<AppState> {
    Router::new().route("/push/{token}", get(push).post(push))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PushQuery {
    /// `down`: the job failed. Anything else is a success.
    status: String,
    msg: String,
}

async fn push(
    State(state): State<AppState>,
    Path(token): Path<String>,
    query: Result<Query<PushQuery>, QueryRejection>,
) -> Result<impl IntoResponse, AppError> {
    if token.len() > TOKEN_MAX_LEN {
        return Err(AppError::NotFound);
    }
    let (id, paused) = Monitor::find_push(&state.db, &token)
        .await?
        .ok_or(AppError::NotFound)?;
    if !paused {
        // Refused rather than read as a success: it may have been a failure report.
        let Query(query) = query.map_err(|r| AppError::InvalidRequest(r.status()))?;
        state.engine.ping(id, outcome(&query));
    }
    Ok(([(header::CACHE_CONTROL, "no-store")], "ok"))
}

fn outcome(query: &PushQuery) -> CheckOutcome {
    let failure = (query.status == "down").then(|| {
        let message: String = query
            .msg
            .chars()
            .filter(|c| !c.is_control())
            .take(MESSAGE_MAX_CHARS)
            .collect();
        Failure {
            kind: ErrorKind::Reported,
            message: if message.trim().is_empty() {
                "the job reported a failure".to_string()
            } else {
                message.trim().to_string()
            },
        }
    });
    CheckOutcome {
        checked_at: now_ms(),
        latency_ms: None,
        status_code: None,
        failure,
        tls_expires_at: None,
    }
}
