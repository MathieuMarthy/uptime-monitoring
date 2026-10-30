use crate::error::AppError;
use crate::forms::incident::NoteForm;
use crate::models::Incident;
use crate::routes::{PathId, form_or_invalid};
use crate::state::AppState;
use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::Redirect;
use axum::routing::post;
use axum::{Form, Router};

pub fn router() -> Router<AppState> {
    Router::new().route("/incidents/{id}/note", post(save_note))
}

/// Saves the note and drops the cached status pages, so a note published or taken back shows
/// at once. Then goes back to the page the form was on (detail or Activity, with its filters),
/// at the incident; its monitor's page when that page is unknown.
async fn save_note(
    State(state): State<AppState>,
    PathId(id): PathId,
    headers: HeaderMap,
    form: Result<Form<NoteForm>, FormRejection>,
) -> Result<Redirect, AppError> {
    let input = form_or_invalid(form)?
        .validate()
        .map_err(|_| AppError::InvalidRequest(StatusCode::UNPROCESSABLE_ENTITY))?;
    let monitor_id = Incident::set_note(&state.db, id, input.note.as_deref(), input.public)
        .await?
        .ok_or(AppError::NotFound)?;
    state.public_pages.clear();
    let back = referer_path(&headers).unwrap_or_else(|| format!("/monitors/{monitor_id}"));
    Ok(Redirect::to(&format!("{back}#incident-{id}")))
}

/// The path and query of the `Referer`, never its host: the redirect stays on this site.
fn referer_path(headers: &HeaderMap) -> Option<String> {
    let uri: Uri = headers.get(header::REFERER)?.to_str().ok()?.parse().ok()?;
    let path = uri.path_and_query()?.as_str();
    // `//host` would be read as another site.
    (path.starts_with('/') && !path.starts_with("//")).then(|| path.to_string())
}
