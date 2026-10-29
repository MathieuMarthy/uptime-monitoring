use crate::error::AppError;
use crate::forms::monitor::FieldErrors;
use crate::forms::status_page::StatusPageForm;
use crate::models::{MonitorSummary, Session, StatusPage, is_unique_violation};
use crate::page_cache::TTL;
use crate::routes::{PathId, form_or_invalid};
use crate::state::AppState;
use crate::views::status_pages::{FormTemplate, IndexTemplate, PublicTemplate, form_monitors};
use axum::extract::rejection::FormRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};

const DUPLICATE_SLUG: &str = "This slug is already used.";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public-pages", get(index))
        .route("/public-pages/new", get(new_form).post(create))
        .route("/public-pages/{id}/edit", get(edit_form).post(update))
        .route("/public-pages/{id}/preview", get(preview))
        .route("/public-pages/{id}/delete", post(delete))
}

/// Reachable without a session.
pub fn public_router() -> Router<AppState> {
    Router::new().route("/status/{slug}", get(public_page))
}

async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<IndexTemplate, AppError> {
    IndexTemplate::load(&state.db, session.csrf_token).await
}

async fn new_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<FormTemplate, AppError> {
    let form = StatusPageForm {
        published: Some("on".to_string()),
        ..StatusPageForm::default()
    };
    Ok(page(
        None,
        form,
        FieldErrors::new(),
        session.csrf_token,
        form_monitors(&state.db).await?,
    ))
}

async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    form: Result<Form<StatusPageForm>, FormRejection>,
) -> Result<Response, AppError> {
    let form = form_or_invalid(form)?;
    let monitors = form_monitors(&state.db).await?;
    let input = match form.validate(&ids(&monitors)) {
        Ok(input) => input,
        Err(errors) => {
            return Ok(page(None, form, errors, session.csrf_token, monitors).into_response());
        }
    };
    match StatusPage::create(&state.db, &input).await {
        Ok(_) => Ok(Redirect::to("/public-pages").into_response()),
        Err(e) if is_unique_violation(&e) => {
            Ok(page(None, form, slug_taken(), session.csrf_token, monitors).into_response())
        }
        Err(e) => Err(e.into()),
    }
}

async fn edit_form(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
) -> Result<FormTemplate, AppError> {
    let stored = load(&state, id).await?;
    let shown = StatusPage::monitors(&state.db, id).await?;
    let form = StatusPageForm::from_page(
        &stored.title,
        &stored.slug,
        stored.description.as_deref(),
        stored.published,
        &shown,
    );
    Ok(page(
        Some(id),
        form,
        FieldErrors::new(),
        session.csrf_token,
        form_monitors(&state.db).await?,
    ))
}

async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    PathId(id): PathId,
    form: Result<Form<StatusPageForm>, FormRejection>,
) -> Result<Response, AppError> {
    load(&state, id).await?;
    let form = form_or_invalid(form)?;
    let monitors = form_monitors(&state.db).await?;
    let input = match form.validate(&ids(&monitors)) {
        Ok(input) => input,
        Err(errors) => {
            return Ok(page(Some(id), form, errors, session.csrf_token, monitors).into_response());
        }
    };
    let result = StatusPage::update(&state.db, id, &input).await;
    state.public_pages.invalidate(id);
    match result {
        Ok(true) => Ok(Redirect::to("/public-pages").into_response()),
        Ok(false) => Err(AppError::NotFound),
        Err(e) if is_unique_violation(&e) => {
            Ok(page(Some(id), form, slug_taken(), session.csrf_token, monitors).into_response())
        }
        Err(e) => Err(e.into()),
    }
}

async fn delete(State(state): State<AppState>, PathId(id): PathId) -> Result<Redirect, AppError> {
    let deleted = StatusPage::delete(&state.db, id).await?;
    state.public_pages.invalidate(id);
    if deleted {
        Ok(Redirect::to("/public-pages"))
    } else {
        Err(AppError::NotFound)
    }
}

/// The page as visitors will see it, published or not; never cached.
async fn preview(
    State(state): State<AppState>,
    PathId(id): PathId,
) -> Result<Html<String>, AppError> {
    let stored = load(&state, id).await?;
    Ok(Html(
        PublicTemplate::load(&state.db, &stored)
            .await?
            .render_html()?,
    ))
}

/// A published page; an unpublished one is a 404, like an unknown slug.
async fn public_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, AppError> {
    let stored = StatusPage::find_by_slug(&state.db, &slug)
        .await?
        .filter(|p| p.published)
        .ok_or(AppError::NotFound)?;
    let html = state
        .public_pages
        .get_or_render(stored.id, || async {
            PublicTemplate::load(&state.db, &stored)
                .await?
                .render_html()
        })
        .await?;
    let max_age = format!("public, max-age={}", TTL.as_secs());
    Ok((
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_str(&max_age).expect("ASCII header"),
        )],
        Html(html),
    )
        .into_response())
}

async fn load(state: &AppState, id: i64) -> Result<StatusPage, AppError> {
    StatusPage::find(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)
}

fn ids(monitors: &[MonitorSummary]) -> Vec<i64> {
    monitors.iter().map(|m| m.id).collect()
}

fn page(
    id: Option<i64>,
    form: StatusPageForm,
    errors: FieldErrors,
    csrf_token: String,
    monitors: Vec<MonitorSummary>,
) -> FormTemplate {
    FormTemplate {
        form,
        errors,
        csrf_token,
        action_url: match id {
            Some(id) => format!("/public-pages/{id}/edit"),
            None => "/public-pages/new".to_string(),
        },
        page_id: id,
        monitors,
    }
}

fn slug_taken() -> FieldErrors {
    FieldErrors::from([("slug", DUPLICATE_SLUG.to_string())])
}
