use crate::error::AppError;
use crate::forms::incident::NoteForm;
use crate::models::Incident;
use crate::routes::{PathId, form_or_invalid};
use crate::state::AppState;
use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::http::StatusCode;
use axum::response::Redirect;
use axum::routing::post;
use axum::{Form, Router};

pub fn router() -> Router<AppState> {
    Router::new().route("/incidents/{id}/note", post(save_note))
}

/// Saves the note, then shows the incident on its monitor's page. Status pages pick a public
/// note up within their one-minute cache.
async fn save_note(
    State(state): State<AppState>,
    PathId(id): PathId,
    form: Result<Form<NoteForm>, FormRejection>,
) -> Result<Redirect, AppError> {
    let input = form_or_invalid(form)?
        .validate()
        .map_err(|_| AppError::InvalidRequest(StatusCode::UNPROCESSABLE_ENTITY))?;
    let monitor_id = Incident::set_note(&state.db, id, input.note.as_deref(), input.public)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Redirect::to(&format!(
        "/monitors/{monitor_id}#incident-{id}"
    )))
}
