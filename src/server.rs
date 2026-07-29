use crate::api::{self, SharedState};
use crate::config::{AlbumMode, Config};
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
    let exclude = Regex::new(&config.exclude_regex).context("invalid exclude regex")?;
    let pool = ThumbnailPool::new(&config);

    let (manifest, timeline) = match config.album_mode {
        AlbumMode::Folders => {
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
                    let manifest = walk::scan(&config.photos_path, &config.exclude_regex)
                        .context("initial scan failed")?;
                    if let Err(error) = manifest.save(&manifest_path) {
                        tracing::warn!("failed to save initial manifest: {error}");
                    }
                    tracing::info!("initial scan complete: {} albums", manifest.albums.len());
                    manifest
                }
            }));
            crate::scanner::watcher::start(
                config.photos_path.clone(),
                config.data_path.clone(),
                config.exclude_regex.clone(),
                manifest.clone(),
            );
            (Some(manifest), None)
        }
        AlbumMode::Timeline => {
            let service = crate::timeline::TimelineService::open(config.clone())
                .await
                .context("initial timeline scan failed")?;
            crate::scanner::watcher::start_timeline(config.photos_path.clone(), service.clone());
            (None, Some(service))
        }
    };

    let state: SharedState = Arc::new(api::AppState {
        config: config.clone(),
        manifest,
        timeline,
        exclude,
        thumbnails: pool,
    });
    let app = build_router(state);
    let addr = format!("{}:{}", config.bind_address, config.port);
    tracing::info!("listening on {addr}");
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
        .route("/api/thumbs/by-id/{id}", get(api::serve_thumbnail_by_id))
        .route("/api/exif/by-id/{id}", get(api::serve_exif_by_id))
        .route("/api/photos/by-id/{id}", get(serve_photo_by_id))
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

async fn serve_photo_by_id(
    State(state): State<SharedState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
    Query(query): Query<PhotoQuery>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let service = state.timeline.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let photo = service
        .db()
        .get_photo(&photo_id)
        .map_err(|error| {
            tracing::error!("timeline photo lookup failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let path = api::resolve_timeline_photo_path(service, &photo.relative_path)?;
    let download_name = query
        .download
        .as_deref()
        .filter(|value| *value == "1" || value.eq_ignore_ascii_case("true"))
        .and_then(|_| path.file_name())
        .and_then(|name| name.to_str());
    serve_file_with_range(&path, &headers, download_name).await
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
            place_base_url: None,
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
            manifest: Some(Arc::new(RwLock::new(Manifest {
                updated: chrono::Utc::now(),
                albums: Vec::new(),
            }))),
            timeline: None,
            exclude: Regex::new(r"$^").expect("test regex"),
            thumbnails,
        })
    }

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "lumiflow-server-{label}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn write(&self, relative_path: &str, bytes: &[u8]) -> std::path::PathBuf {
            let path = self.0.join(relative_path);
            std::fs::create_dir_all(path.parent().expect("file parent"))
                .expect("create file parent");
            std::fs::write(&path, bytes).expect("write test file");
            path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn timeline_config(photos: &std::path::Path, data: &std::path::Path) -> Config {
        let mut config = router_test_state().config.clone();
        config.album_mode = crate::config::AlbumMode::Timeline;
        config.photos_path = photos.to_path_buf();
        config.data_path = data.to_path_buf();
        config.timeline_timezone = "UTC".into();
        config
    }

    fn insert_timeline_photo(
        db: &crate::timeline::db::TimelineDb,
        id: &str,
        relative_path: &str,
        fingerprint: &str,
        exif: serde_json::Value,
    ) {
        use crate::timeline::models::{PhotoAnalysis, PhotoCandidate, TimeSource};

        db.upsert_candidate(&PhotoCandidate {
            id: id.into(),
            relative_path: relative_path.into(),
            filename: std::path::Path::new(relative_path)
                .file_name()
                .expect("filename")
                .to_string_lossy()
                .into_owned(),
            extension: "png".into(),
            size_bytes: 12,
            mtime_ns: 1,
            fingerprint: fingerprint.into(),
            scan_id: "test-scan".into(),
        })
        .expect("insert candidate");
        db.save_analysis(&PhotoAnalysis {
            id: id.into(),
            taken_at: Some("2024-02-10T09:00:00+00:00".into()),
            time_source: TimeSource::Exif,
            timezone: Some("+00:00".into()),
            gps_lat: None,
            gps_lon: None,
            width: 2,
            height: 2,
            camera_make: None,
            camera_model: None,
            lens: None,
            exif_json: exif,
        })
        .expect("save analysis");
    }

    fn timeline_test_state(config: Config, db: crate::timeline::db::TimelineDb) -> SharedState {
        let service = Arc::new(
            crate::timeline::TimelineService::from_db_for_test(config.clone(), db)
                .expect("timeline service"),
        );
        Arc::new(api::AppState {
            config: config.clone(),
            manifest: None,
            timeline: Some(service),
            exclude: Regex::new(r"$^").expect("test regex"),
            thumbnails: ThumbnailPool::new(&config),
        })
    }

    async fn response_bytes(response: Response) -> Vec<u8> {
        use axum::body::HttpBody;

        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) =
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
        {
            let frame = frame.expect("body frame");
            if let Ok(data) = frame.into_data() {
                bytes.extend_from_slice(&data);
            }
        }
        bytes
    }

    #[tokio::test]
    async fn folder_mode_keeps_manifest_album_detail_and_rescan_behavior() {
        let photos = TestDir::new("folder-mode-photos");
        let data = TestDir::new("folder-mode-data");
        photos.write("album/a.jpg", b"photo");
        photos.write("album/b.jpg", b"photo");
        let mut config = router_test_state().config.clone();
        config.photos_path = photos.path().to_path_buf();
        config.data_path = data.path().to_path_buf();
        let manifest = walk::scan(&config.photos_path, &config.exclude_regex).expect("manifest");
        let state = Arc::new(api::AppState {
            config: config.clone(),
            manifest: Some(Arc::new(RwLock::new(manifest))),
            timeline: None,
            exclude: Regex::new(&config.exclude_regex).expect("exclude"),
            thumbnails: ThumbnailPool::new(&config),
        });

        let list = build_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/albums")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let list_json: serde_json::Value =
            serde_json::from_slice(&response_bytes(list).await).expect("list json");
        assert_eq!(list_json["albums"][0]["name"], "album");
        assert_eq!(list_json["albums"][0]["count"], 2);

        let detail = build_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/albums/album?offset=1&limit=1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let detail_json: serde_json::Value =
            serde_json::from_slice(&response_bytes(detail).await).expect("detail json");
        assert_eq!(detail_json["photo_count"], 2);
        assert_eq!(detail_json["photos"].as_array().map(Vec::len), Some(1));
        assert_eq!(detail_json["photos"][0]["id"], 1);
        assert_eq!(detail_json["photos"][0]["name"], "b.jpg");

        let rescan = build_router(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/rescan")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let rescan_json: serde_json::Value =
            serde_json::from_slice(&response_bytes(rescan).await).expect("rescan json");
        assert_eq!(rescan_json["status"], "ok");
        assert_eq!(rescan_json["albums_count"], 1);
    }

    #[tokio::test]
    async fn timeline_mode_lists_sqlite_albums_and_detail() {
        use crate::timeline::models::{DailyAlbumBuild, TimelineAlbum};

        let photos = TestDir::new("timeline-albums-photos");
        let data = TestDir::new("timeline-albums-data");
        let config = timeline_config(photos.path(), data.path());
        let db = crate::timeline::db::TimelineDb::open(data.path().join("lumiflow.sqlite"))
            .expect("timeline db");
        insert_timeline_photo(&db, "p1", "nested/a.png", "fp-a", serde_json::json!({}));
        insert_timeline_photo(&db, "p2", "nested/b.png", "fp-b", serde_json::json!({}));
        insert_timeline_photo(&db, "p3", "nested/c.png", "fp-c", serde_json::json!({}));
        db.replace_daily_albums(&[DailyAlbumBuild {
            album: TimelineAlbum {
                id: "auto-day:2024-02-10".into(),
                name: "2024-02-10".into(),
                description: None,
                date_start: chrono::NaiveDate::from_ymd_opt(2024, 2, 10),
                date_end: chrono::NaiveDate::from_ymd_opt(2024, 2, 10),
                place: None,
                holiday: None,
                photo_count: 3,
                cover_photo_id: Some("p1".into()),
            },
            photo_ids: vec!["p1".into(), "p2".into(), "p3".into()],
        }])
        .expect("album");
        let state = timeline_test_state(config, db);

        let list = build_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/albums")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(list.status(), StatusCode::OK);
        let list_json: serde_json::Value =
            serde_json::from_slice(&response_bytes(list).await).expect("list json");
        assert_eq!(list_json["albums"][0]["id"], "auto-day:2024-02-10");

        let detail = build_router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/albums/auto-day%3A2024-02-10?offset=1&limit=1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(detail.status(), StatusCode::OK);
        let detail_json: serde_json::Value =
            serde_json::from_slice(&response_bytes(detail).await).expect("detail json");
        assert_eq!(detail_json["photo_count"], 3);
        assert_eq!(detail_json["photos"].as_array().map(Vec::len), Some(1));
        assert_eq!(detail_json["photos"][0]["id"], "p2");
    }

    #[tokio::test]
    async fn timeline_rescan_returns_scan_and_album_counts() {
        let photos = TestDir::new("timeline-rescan-photos");
        let data = TestDir::new("timeline-rescan-data");
        let image_path = photos.path().join("nested/a.png");
        std::fs::create_dir_all(image_path.parent().expect("image parent")).expect("parent");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(&image_path)
            .expect("write image");
        let config = timeline_config(photos.path(), data.path());
        let db = crate::timeline::db::TimelineDb::open(data.path().join("lumiflow.sqlite"))
            .expect("timeline db");
        let state = timeline_test_state(config, db);

        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/rescan")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&response_bytes(response).await).expect("rescan json");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["found"], 1);
        assert_eq!(json["analyzed"], 1);
        assert_eq!(json["errors"], 0);
        assert_eq!(json["albums_count"], 1);
        assert_eq!(json["enrichment"]["thumbnails_generated"], 0);
        assert_eq!(json["enrichment"]["ai_errors"], 0);
    }

    #[tokio::test]
    async fn by_id_original_serves_nested_file_range_download_and_unknown_id() {
        let photos = TestDir::new("by-id-original-photos");
        let data = TestDir::new("by-id-original-data");
        photos.write("nested/a.png", b"0123456789");
        let config = timeline_config(photos.path(), data.path());
        let db = crate::timeline::db::TimelineDb::open(data.path().join("lumiflow.sqlite"))
            .expect("timeline db");
        insert_timeline_photo(&db, "p1", "nested/a.png", "fp-a", serde_json::json!({}));
        let state = timeline_test_state(config, db);

        let full = build_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/photos/by-id/p1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(response_bytes(full).await, b"0123456789");

        let ranged = build_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/photos/by-id/p1")
                    .header(header::RANGE, "bytes=2-5")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response_bytes(ranged).await, b"2345");

        let download = build_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/photos/by-id/p1?download=1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(download.status(), StatusCode::OK);
        assert!(download.headers().contains_key(header::CONTENT_DISPOSITION));

        let missing = build_router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/photos/by-id/missing")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn by_id_original_rejects_database_path_that_escapes_photo_root() {
        use std::os::unix::fs::symlink;

        let photos = TestDir::new("by-id-escape-photos");
        let outside = TestDir::new("by-id-escape-outside");
        let data = TestDir::new("by-id-escape-data");
        outside.write("secret.png", b"secret");
        symlink(outside.path(), photos.path().join("linked")).expect("symlink");
        let config = timeline_config(photos.path(), data.path());
        let db = crate::timeline::db::TimelineDb::open(data.path().join("lumiflow.sqlite"))
            .expect("timeline db");
        insert_timeline_photo(
            &db,
            "escape",
            "linked/secret.png",
            "fp-secret",
            serde_json::json!({}),
        );

        let response = build_router(timeline_test_state(config, db))
            .oneshot(
                Request::builder()
                    .uri("/api/photos/by-id/escape")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn by_id_thumbnail_generates_webp_and_persists_source_fingerprint() {
        let photos = TestDir::new("by-id-thumb-photos");
        let data = TestDir::new("by-id-thumb-data");
        let image_path = photos.path().join("nested/a.png");
        std::fs::create_dir_all(image_path.parent().expect("image parent")).expect("parent");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([100, 50, 20, 255]))
            .save(&image_path)
            .expect("write image");
        let config = timeline_config(photos.path(), data.path());
        let db = crate::timeline::db::TimelineDb::open(data.path().join("lumiflow.sqlite"))
            .expect("timeline db");
        insert_timeline_photo(&db, "p1", "nested/a.png", "exact-fp", serde_json::json!({}));

        let response = build_router(timeline_test_state(config, db))
            .oneshot(
                Request::builder()
                    .uri("/api/thumbs/by-id/p1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            HeaderValue::from_static("image/webp")
        );
        let bytes = response_bytes(response).await;
        assert!(image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).is_ok());
        assert!(crate::thumbnail::timeline_thumb_is_fresh(
            data.path(),
            "p1",
            "exact-fp"
        ));
    }

    #[tokio::test]
    async fn by_id_exif_returns_stored_json_after_original_is_removed() {
        let photos = TestDir::new("by-id-exif-photos");
        let data = TestDir::new("by-id-exif-data");
        let original = photos.write("nested/a.png", b"not-image");
        let config = timeline_config(photos.path(), data.path());
        let db = crate::timeline::db::TimelineDb::open(data.path().join("lumiflow.sqlite"))
            .expect("timeline db");
        insert_timeline_photo(
            &db,
            "p1",
            "nested/a.png",
            "fp-a",
            serde_json::json!({"iso": 640, "camera": "stored"}),
        );
        std::fs::remove_file(original).expect("remove original");

        let response = build_router(timeline_test_state(config, db))
            .oneshot(
                Request::builder()
                    .uri("/api/exif/by-id/p1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&response_bytes(response).await).expect("exif json");
        assert_eq!(json, serde_json::json!({"iso": 640, "camera": "stored"}));
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
