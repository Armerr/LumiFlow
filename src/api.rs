use crate::config::{AlbumMode, Config};
use crate::scanner::manifest::{AlbumDetail, Manifest};
use crate::scanner::walk;
use crate::thumbnail::{self, ThumbnailPool};
use crate::timeline::db::TimelineFilter;
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

#[derive(Debug, Default, Deserialize)]
pub struct AlbumListQuery {
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
    #[serde(default)]
    person: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

impl AlbumListQuery {
    fn limit(&self) -> usize {
        self.limit
            .unwrap_or(DEFAULT_ALBUM_PAGE_SIZE)
            .clamp(1, MAX_ALBUM_PAGE_SIZE)
    }

    fn filter(&self) -> TimelineFilter {
        TimelineFilter {
            person: self.person.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
        }
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

pub async fn status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    match &state.timeline {
        Some(service) => Json(serde_json::to_value(service.status()).unwrap_or_default()),
        None => Json(serde_json::json!({
            "state": "ready",
            "phase": "ready",
            "found": 0,
            "processed": 0,
            "errors": 0,
            "workers": 0,
            "elapsed_seconds": 0,
            "error": null,
        })),
    }
}

// --- GET /api/albums ---

pub async fn list_albums(
    State(state): State<SharedState>,
    Query(query): Query<AlbumListQuery>,
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
            let albums = service.db().list_albums_filtered(query.filter()).map_err(internal_error)?;
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
    Query(page): Query<AlbumListQuery>,
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
                .get_album_page_filtered(&name, page.offset, limit, page.filter())
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
        if source_mtime.is_some() && thumb_mtime.is_some() && thumb_mtime >= source_mtime {
            return serve_cached_file(&thumb_path, "image/webp").await;
        }
    }

    // On-demand generation
    match ThumbnailPool::generate_on_demand(&source, &thumb_path) {
        Ok(_) => serve_cached_file(&thumb_path, "image/webp").await,
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn serve_thumbnail_by_id(
    State(state): State<SharedState>,
    Path(photo_id): Path<String>,
) -> Result<axum::response::Response, StatusCode> {
    let service = state.timeline.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let photo = service
        .db()
        .get_photo(&photo_id)
        .map_err(|error| {
            tracing::error!("timeline photo lookup failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let thumb_path =
        crate::thumbnail::timeline_thumb_path(&state.config.data_path, &photo.id);
    if !thumb_path.is_file() {
        let source = resolve_timeline_photo_path(service, &photo.relative_path)?;
        ThumbnailPool::generate_on_demand(&source, &thumb_path)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        crate::thumbnail::write_timeline_thumb_fingerprint(
            &state.config.data_path,
            &photo.id,
            &photo.fingerprint,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    serve_cached_file(&thumb_path, "image/webp").await
}

async fn serve_cached_file(
    path: &std::path::Path,
    mime: &str,
) -> Result<axum::response::Response, StatusCode> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok((
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, mime.to_owned()),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=31536000, immutable".to_owned(),
            ),
        ],
        bytes,
    )
        .into_response())
}

// --- GET /api/exif/{*path} ---

pub async fn serve_exif(
    State(state): State<SharedState>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let (album, filename) = path.split_once('/').ok_or(StatusCode::BAD_REQUEST)?;
    let source = state.config.photos_path.join(album).join(filename);
    crate::exif::extract_exif(&source)
        .map_err(|_| StatusCode::NOT_FOUND)
        .and_then(|exif| serde_json::to_value(exif).map(Json).map_err(internal_error))
}

pub async fn serve_exif_by_id(
    State(state): State<SharedState>,
    Path(photo_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let service = state.timeline.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    service
        .db()
        .get_photo_exif(&photo_id)
        .map_err(|error| {
            tracing::error!("timeline EXIF lookup failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(|exif| Json(exif))
        .ok_or(StatusCode::NOT_FOUND)
}

pub(crate) fn resolve_timeline_photo_path(
    service: &TimelineService,
    relative_path: &str,
) -> Result<std::path::PathBuf, StatusCode> {
    let source = service.config().photos_path.join(relative_path);
    let canonical = source.canonicalize().map_err(|_| StatusCode::NOT_FOUND)?;
    let root = service
        .config()
        .photos_path
        .canonicalize()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if canonical.starts_with(&root) && canonical.is_file() {
        Ok(canonical)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn timeline_service(state: &AppState) -> Result<&Arc<TimelineService>, StatusCode> {
    state.timeline.as_ref().ok_or(StatusCode::NOT_FOUND)
}

fn internal_error(error: impl std::fmt::Display) -> StatusCode {
    tracing::error!("internal API error: {error}");
    StatusCode::INTERNAL_SERVER_ERROR
}
