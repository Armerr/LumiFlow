use crate::api::{self, SharedState};
use crate::config::Config;
use crate::embedded::Frontend;
use crate::scanner::manifest::Manifest;
use crate::scanner::walk;
use crate::thumbnail::ThumbnailPool;
use anyhow::Context;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use regex::Regex;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

/// Build the Axum router and start the server.
pub async fn serve(config: Config) -> anyhow::Result<()> {
    // Scan and load manifest on startup.
    let exclude = Regex::new(&config.exclude_regex).context("invalid exclude regex")?;
    let manifest_path = config.data_path.join("manifest.json");

    let manifest = Arc::new(RwLock::new(match Manifest::load(&manifest_path) {
        Some(cached) => {
            tracing::info!(
                "loaded cached manifest: {} albums, updated {}",
                cached.albums.len(),
                cached.updated.to_rfc3339()
            );
            cached
        }
        None => {
            tracing::info!("no cached manifest found, scanning...");
            let m = walk::scan(&config.photos_path, &config.exclude_regex)
                .context("initial scan failed")?;
            if let Err(e) = m.save(&manifest_path) {
                tracing::warn!("failed to save initial manifest: {}", e);
            }
            tracing::info!("initial scan complete: {} albums", m.albums.len());
            m
        }
    }));

    // Start filesystem watcher (auto-detect changes).
    crate::scanner::watcher::start(
        config.photos_path.clone(),
        config.data_path.clone(),
        config.exclude_regex.clone(),
        manifest.clone(),
    );

    let pool = ThumbnailPool::new(&config);

    let state: SharedState = Arc::new(api::AppState {
        config: config.clone(),
        manifest,
        exclude,
        thumbnails: pool.clone(),
    });

    // Start background thumbnail pre-generation.
    pool.pregenerate_all(
        config.photos_path.clone(),
        config.data_path.clone(),
        config.exclude_regex.clone(),
    );

    let app = build_router(state);

    let addr = format!("{}:{}", config.bind_address, config.port);
    tracing::info!("listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn build_router(state: SharedState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    Router::new()
        // API routes
        .route("/api/albums", get(api::list_albums))
        .route("/api/albums/{name}", get(api::get_album))
        .route("/api/rescan", get(api::rescan).post(api::rescan))
        // Thumbnails & EXIF
        .route("/api/thumbs/{*path}", get(api::serve_thumbnail))
        .route("/api/exif/{*path}", get(api::serve_exif))
        // Photo serving
        .route("/api/photos/{*path}", get(serve_photo))
        .fallback(get(serve_frontend))
        .layer(cors)
        .with_state(state)
}

// --- Photo serving ---

#[derive(Debug, Default, Deserialize)]
struct PhotoQuery {
    download: Option<String>,
}

async fn serve_photo(
    State(state): State<SharedState>,
    axum::extract::Path(path): axum::extract::Path<String>,
    Query(query): Query<PhotoQuery>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    // Path is already URL-decoded by axum's Path extractor.
    let file_path = state.config.photos_path.join(&path);

    // Security: prevent directory traversal
    let canonical = file_path
        .canonicalize()
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let photos_canonical = state
        .config
        .photos_path
        .canonicalize()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !canonical.starts_with(&photos_canonical) {
        return Err(StatusCode::FORBIDDEN);
    }

    if !canonical.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }

    let download_name = query
        .download
        .as_deref()
        .filter(|v| *v == "1" || v.eq_ignore_ascii_case("true"))
        .and_then(|_| canonical.file_name())
        .and_then(|name| name.to_str());
    serve_file_with_range(&canonical, &headers, download_name).await
}

async fn serve_file_with_range(
    path: &std::path::Path,
    headers: &HeaderMap,
    download_name: Option<&str>,
) -> Result<Response, StatusCode> {
    use std::io::SeekFrom;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let metadata = std::fs::metadata(path).map_err(|_| StatusCode::NOT_FOUND)?;
    let file_size = metadata.len();

    let mime = mime_guess(path);
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    resp_headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap());
    if let Some(name) = download_name.and_then(content_disposition_for_download) {
        resp_headers.insert(header::CONTENT_DISPOSITION, name);
    }

    if let Some(range_header) = headers.get(header::RANGE) {
        let range_str = range_header.to_str().unwrap_or("");
        if let Some(range) = parse_range(range_str, file_size) {
            let (start, end) = range;
            let length = end - start + 1;

            resp_headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end, file_size)).unwrap(),
            );
            resp_headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));

            let file_path = path.to_path_buf();
            let body = Body::from_stream(async_stream::stream! {
                let mut file = tokio::fs::File::open(&file_path).await.unwrap();
                file.seek(SeekFrom::Start(start)).await.unwrap();
                let mut buf = vec![0u8; length as usize];
                file.read_exact(&mut buf).await.unwrap();
                yield Ok::<_, std::io::Error>(bytes::Bytes::from(buf));
            });

            return Ok((StatusCode::PARTIAL_CONTENT, resp_headers, body).into_response());
        }
    }

    // No range: serve full file.
    resp_headers.insert(header::CONTENT_LENGTH, HeaderValue::from(file_size));

    let file_path = path.to_path_buf();
    let body = Body::from_stream(async_stream::stream! {
        let mut file = tokio::fs::File::open(&file_path).await.unwrap();
        let mut buf = vec![0u8; 8192];
        loop {
            match file.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => yield Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(&buf[..n])),
                Err(e) => {
                    yield Err(e);
                    break;
                }
            }
        }
    });

    Ok((StatusCode::OK, resp_headers, body).into_response())
}

fn parse_range(range_str: &str, file_size: u64) -> Option<(u64, u64)> {
    let range_str = range_str.strip_prefix("bytes=")?;
    let parts: Vec<&str> = range_str.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start: u64 = if parts[0].is_empty() {
        let suffix: i64 = parts[1].parse().ok()?;
        file_size.saturating_sub(suffix as u64)
    } else {
        parts[0].parse().ok()?
    };

    let end: u64 = if parts[1].is_empty() {
        file_size - 1
    } else {
        parts[1].parse().ok()?
    };

    if start > end || end >= file_size {
        None
    } else {
        Some((start, end))
    }
}

fn mime_guess(path: &std::path::Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg".into(),
        Some("png") => "image/png".into(),
        Some("webp") => "image/webp".into(),
        Some("gif") => "image/gif".into(),
        Some("heic") => "image/heic".into(),
        Some("heif") => "image/heif".into(),
        Some("avif") => "image/avif".into(),
        Some("tif") | Some("tiff") => "image/tiff".into(),
        _ => "application/octet-stream".into(),
    }
}

fn content_disposition_for_download(filename: &str) -> Option<HeaderValue> {
    let fallback = filename
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\r' | '\n' => '_',
            c if c.is_ascii_graphic() || c == ' ' => c,
            _ => '_',
        })
        .collect::<String>();
    let encoded = filename.bytes().fold(String::new(), |mut out, b| {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => out.push(char::from(b)),
            _ => out.push_str(&format!("%{b:02X}")),
        }
        out
    });
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"
    ))
    .ok()
}

// --- Frontend serving ---

async fn serve_frontend(uri: Uri) -> Result<Response, StatusCode> {
    let path = uri.path().trim_start_matches('/');
    let is_spa_route = path.is_empty() || path.starts_with("album/");

    // Try exact path match in embedded files.
    let file_path = if path.is_empty() { "index.html" } else { path };

    if let Some(content) = Frontend::get(file_path) {
        let mime = mime_from_ext(file_path);
        return Ok((StatusCode::OK, [(header::CONTENT_TYPE, mime)], content.data).into_response());
    }

    // Check for .html variant (clean URLs).
    let html_path = format!("{}.html", file_path);
    if let Some(content) = Frontend::get(&html_path) {
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html")],
            content.data,
        )
            .into_response());
    }

    // SPA fallback: serve index.html only for known client-side routes.
    if is_spa_route {
        if let Some(content) = Frontend::get("index.html") {
            return Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html")],
                content.data,
            )
                .into_response());
        }
    }

    Err(StatusCode::NOT_FOUND)
}

fn mime_from_ext(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".mjs") {
        "application/javascript"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else {
        "application/octet-stream"
    }
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");
    tracing::info!("shutting down...");
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::Request;
    use tower::ServiceExt;

    fn router_test_state() -> SharedState {
        let config = Config {
            photos_path: std::path::PathBuf::from("."),
            data_path: std::path::PathBuf::from("."),
            bind_address: "127.0.0.1".into(),
            port: 4320,
            builder_workers: 1,
            exclude_regex: r"$^".into(),
            album_mode: crate::config::AlbumMode::Folders,
            timeline_timezone: "Asia/Shanghai".into(),
            calendar_region: "CN_COMMON".into(),
            place_provider: None,
            vision_tagger: crate::config::VisionTagger::None,
            vision_model_path: None,
            vision_labels_path: None,
            vision_workers: 1,
            ai: crate::config::AiConfig {
                enabled: false,
                base_url: None,
                api_key: None,
                model: None,
                language: "zh-CN".into(),
            },
        };
        let thumbnails = ThumbnailPool::new(&config);

        Arc::new(api::AppState {
            config,
            manifest: Arc::new(RwLock::new(Manifest {
                updated: chrono::Utc::now(),
                albums: Vec::new(),
            })),
            exclude: Regex::new(r"$^").expect("test regex"),
            thumbnails,
        })
    }

    #[tokio::test]
    async fn health_path_is_not_registered() {
        let response = build_router(router_test_state())
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    #[test]
    fn content_disposition_for_download_encodes_original_filename() {
        let header =
            content_disposition_for_download("DSCF 6138.HIF").expect("content disposition");

        assert_eq!(
            header.to_str().expect("header string"),
            "attachment; filename=\"DSCF 6138.HIF\"; filename*=UTF-8''DSCF%206138.HIF"
        );
    }

    #[test]
    fn content_disposition_for_download_uses_ascii_fallback_for_unicode_filename() {
        let header = content_disposition_for_download("东京 01.jpg").expect("content disposition");

        assert_eq!(
            header.to_str().expect("header string"),
            "attachment; filename=\"__ 01.jpg\"; filename*=UTF-8''%E4%B8%9C%E4%BA%AC%2001.jpg"
        );
    }
}
