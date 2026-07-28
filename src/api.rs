use crate::config::Config;
use crate::scanner::manifest::{AlbumDetail, Manifest};
use crate::scanner::walk;
use crate::thumbnail::{self, ThumbnailPool};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use regex::Regex;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared application state.
pub struct AppState {
    pub config: Config,
    pub manifest: Arc<RwLock<Manifest>>,
    pub exclude: Regex,
    #[allow(dead_code)]
    pub thumbnails: ThumbnailPool,
}

pub type SharedState = Arc<AppState>;

// --- GET /api/albums ---

pub async fn list_albums(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let manifest = state.manifest.read().await;
    Json(serde_json::json!({
        "albums": &manifest.albums,
        "updated": manifest.updated.to_rfc3339(),
    }))
}

// --- GET /api/albums/:name ---

pub async fn get_album(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> Result<Json<AlbumDetail>, StatusCode> {
    let photos = walk::get_album_detail(&state.config.photos_path, &name, &state.exclude)
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(AlbumDetail { name, photos }))
}

// --- POST /api/rescan ---

pub async fn rescan(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let manifest_path = state.config.data_path.join("manifest.json");

    match walk::scan(
        &state.config.photos_path,
        state.config.exclude_regex.as_str(),
    ) {
        Ok(new_manifest) => {
            let mut old = state.manifest.write().await;

            // Log what changed
            let diff = new_manifest.diff(&old);
            if diff.has_changes() {
                tracing::info!(
                    "rescan: {} new albums, {} removed albums",
                    diff.new_albums.len(),
                    diff.removed_albums.len()
                );
            }

            *old = new_manifest.clone();

            if let Err(e) = new_manifest.save(&manifest_path) {
                tracing::error!("failed to save manifest: {}", e);
            }

            Json(serde_json::json!({
                "status": "ok",
                "albums_count": new_manifest.albums.len(),
                "updated": new_manifest.updated.to_rfc3339(),
            }))
        }
        Err(e) => {
            tracing::error!("rescan failed: {}", e);
            Json(serde_json::json!({
                "status": "error",
                "message": e.to_string(),
            }))
        }
    }
}

// --- GET /api/thumbs/{*path} ---

pub async fn serve_thumbnail(
    State(state): State<SharedState>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<axum::response::Response, StatusCode> {
    // Path format: <album>/<filename>
    let (album, filename) = path.split_once('/').ok_or(StatusCode::BAD_REQUEST)?;

    let source = state.config.photos_path.join(album).join(filename);
    let thumb_path = thumbnail::thumb_path(&state.config.data_path, album, filename);

    // Serve cached thumb if fresh
    if thumb_path.exists() {
        let source_mtime = source.metadata().ok().and_then(|m| m.modified().ok());
        let thumb_mtime = thumb_path.metadata().ok().and_then(|m| m.modified().ok());
        if let (Some(s), Some(t)) = (source_mtime, thumb_mtime) {
            if t >= s {
                return serve_cached_file(&thumb_path, "image/webp").await;
            }
        }
    }

    // Generate on demand
    match ThumbnailPool::generate_on_demand(&source, &thumb_path) {
        Ok(data) => Ok((
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "image/webp"),
                (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            data,
        )
            .into_response()),
        Err(e) => {
            tracing::warn!("thumbnail generation failed for {}: {}", path, e);
            Err(StatusCode::NOT_FOUND)
        }
    }
}

async fn serve_cached_file(
    path: &std::path::Path,
    mime: &str,
) -> Result<axum::response::Response, StatusCode> {
    let data = tokio::fs::read(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok((
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, mime),
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        data,
    )
        .into_response())
}

// --- GET /api/exif/{*path} ---

pub async fn serve_exif(
    State(state): State<SharedState>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let file_path = state.config.photos_path.join(&path);

    if !file_path.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }

    match crate::exif::extract_exif(&file_path) {
        Ok(data) => Ok(Json(serde_json::to_value(data).unwrap_or_default())),
        Err(e) => {
            tracing::warn!("EXIF extraction failed for {}: {}", path, e);
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        }
    }
}
