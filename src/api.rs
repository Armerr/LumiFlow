use crate::config::{AlbumMode, Config};
use crate::scanner::manifest::{AlbumDetail, Manifest};
use crate::scanner::walk;
use crate::thumbnail::{self, ThumbnailPool};
use crate::timeline::TimelineService;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use regex::Regex;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

const DEFAULT_ALBUM_PAGE_SIZE: usize = 60;
const MAX_ALBUM_PAGE_SIZE: usize = 120;

#[derive(Debug, Deserialize)]
pub struct AlbumPageQuery {
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

impl AlbumPageQuery {
    fn limit(&self) -> usize {
        self.limit
            .unwrap_or(DEFAULT_ALBUM_PAGE_SIZE)
            .clamp(1, MAX_ALBUM_PAGE_SIZE)
    }
}

/// Shared application state.
pub struct AppState {
    pub config: Config,
    pub manifest: Option<Arc<RwLock<Manifest>>>,
    pub timeline: Option<Arc<TimelineService>>,
    pub exclude: Regex,
    #[allow(dead_code)]
    pub thumbnails: ThumbnailPool,
}

pub type SharedState = Arc<AppState>;

// --- GET /api/albums ---

pub async fn list_albums(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.config.album_mode {
        AlbumMode::Folders => {
            let manifest = state
                .manifest
                .as_ref()
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
                .read()
                .await;
            Ok(Json(serde_json::json!({
                "albums": &manifest.albums,
                "updated": manifest.updated.to_rfc3339(),
            })))
        }
        AlbumMode::Timeline => {
            let service = timeline_service(&state)?;
            let albums = service.db().list_albums().map_err(internal_error)?;
            Ok(Json(serde_json::json!({
                "albums": albums,
                "updated": chrono::Utc::now().to_rfc3339(),
            })))
        }
    }
}

// --- GET /api/albums/:name ---

pub async fn get_album(
    State(state): State<SharedState>,
    Path(name): Path<String>,
    Query(page): Query<AlbumPageQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let limit = page.limit();
    match state.config.album_mode {
        AlbumMode::Folders => {
            let photos = walk::get_album_detail(&state.config.photos_path, &name, &state.exclude)
                .ok_or(StatusCode::NOT_FOUND)?;
            let photo_count = photos.len();
            let photos = photos.into_iter().skip(page.offset).take(limit).collect();
            Ok(Json(
                serde_json::to_value(AlbumDetail {
                    name,
                    photo_count,
                    photos,
                })
                .map_err(internal_error)?,
            ))
        }
        AlbumMode::Timeline => {
            let detail = timeline_service(&state)?
                .db()
                .get_album_page(&name, page.offset, limit)
                .map_err(internal_error)?
                .ok_or(StatusCode::NOT_FOUND)?;
            Ok(Json(serde_json::to_value(detail).map_err(internal_error)?))
        }
    }
}

// --- POST /api/rescan ---

pub async fn rescan(State(state): State<SharedState>) -> Json<serde_json::Value> {
    if state.config.album_mode == AlbumMode::Timeline {
        return match timeline_service(&state) {
            Ok(service) => match service.rescan().await {
                Ok(report) => Json(serde_json::json!({
                    "status": "ok",
                    "found": report.scan.found,
                    "analyzed": report.scan.analyzed,
                    "reused": report.scan.reused,
                    "errors": report.scan.errors,
                    "marked_missing": report.scan.marked_missing,
                    "albums_count": report.albums_count,
                    "enrichment": report.enrichment,
                    "updated": chrono::Utc::now().to_rfc3339(),
                })),
                Err(error) => {
                    tracing::error!("timeline rescan failed: {error:#}");
                    Json(serde_json::json!({
                        "status": "error",
                        "message": error.to_string(),
                    }))
                }
            },
            Err(_) => Json(serde_json::json!({
                "status": "error",
                "message": "timeline service is unavailable",
            })),
        };
    }

    let manifest_path = state.config.data_path.join("manifest.json");
    match walk::scan(
        &state.config.photos_path,
        state.config.exclude_regex.as_str(),
    ) {
        Ok(new_manifest) => {
            let mut old = match &state.manifest {
                Some(manifest) => manifest.write().await,
                None => {
                    tracing::error!("folder manifest is unavailable");
                    return Json(serde_json::json!({
                        "status": "error",
                        "message": "folder manifest is unavailable",
                    }));
                }
            };
            let diff = new_manifest.diff(&old);
            if diff.has_changes() {
                tracing::info!(
                    "rescan: {} new albums, {} removed albums",
                    diff.new_albums.len(),
                    diff.removed_albums.len()
                );
            }
            *old = new_manifest.clone();
            if let Err(error) = new_manifest.save(&manifest_path) {
                tracing::error!("failed to save manifest: {error}");
            }
            Json(serde_json::json!({
                "status": "ok",
                "albums_count": new_manifest.albums.len(),
                "updated": new_manifest.updated.to_rfc3339(),
            }))
        }
        Err(error) => {
            tracing::error!("rescan failed: {error}");
            Json(serde_json::json!({
                "status": "error",
                "message": error.to_string(),
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

    // Generate on demand outside the async executor, bounded by configured workers.
    match state.thumbnails.generate(source, thumb_path).await {
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

pub async fn serve_thumbnail_by_id(
    State(state): State<SharedState>,
    Path(photo_id): Path<String>,
) -> Result<axum::response::Response, StatusCode> {
    let service = timeline_service(&state)?;
    let photo = service
        .db()
        .get_photo(&photo_id)
        .map_err(internal_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let source = resolve_timeline_photo_path(service, &photo.relative_path)?;
    let thumb_path = thumbnail::timeline_thumb_path(&state.config.data_path, &photo.id);

    if thumbnail::timeline_thumb_is_fresh(&state.config.data_path, &photo.id, &photo.fingerprint) {
        return serve_cached_file(&thumb_path, "image/webp").await;
    }

    let data = state
        .thumbnails
        .generate(source, thumb_path.clone())
        .await
        .map_err(|error| {
            tracing::warn!("timeline thumbnail generation failed for {photo_id}: {error:#}");
            StatusCode::NOT_FOUND
        })?;
    thumbnail::write_timeline_thumb_fingerprint(
        &state.config.data_path,
        &photo.id,
        &photo.fingerprint,
    )
    .map_err(internal_error)?;

    Ok((
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "image/webp"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        data,
    )
        .into_response())
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

pub async fn serve_exif_by_id(
    State(state): State<SharedState>,
    Path(photo_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let exif = timeline_service(&state)?
        .db()
        .get_photo_exif(&photo_id)
        .map_err(internal_error)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(exif))
}

pub(crate) fn resolve_timeline_photo_path(
    service: &TimelineService,
    relative_path: &str,
) -> Result<std::path::PathBuf, StatusCode> {
    let root = service
        .config()
        .photos_path
        .canonicalize()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let target = root
        .join(relative_path)
        .canonicalize()
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if !target.starts_with(&root) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !target.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(target)
}

fn timeline_service(state: &AppState) -> Result<&Arc<TimelineService>, StatusCode> {
    state
        .timeline
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
}

fn internal_error(error: impl std::fmt::Display) -> StatusCode {
    tracing::error!("API operation failed: {error}");
    StatusCode::INTERNAL_SERVER_ERROR
}
