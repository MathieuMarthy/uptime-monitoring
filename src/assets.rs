use crate::error::AppError;
use crate::state::AppState;
use axum::Router;
use axum::extract::Path;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

#[derive(rust_embed::Embed)]
#[folder = "static/"]
struct Static;

pub fn router() -> Router<AppState> {
    Router::new().route("/static/{*path}", get(serve))
}

async fn serve(Path(path): Path<String>) -> Result<impl IntoResponse, AppError> {
    let file = Static::get(&path).ok_or(AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
            (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
        ],
        file.data,
    ))
}
