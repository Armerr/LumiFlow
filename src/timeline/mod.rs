pub mod ai;
pub mod albums;
pub mod contact_sheet;
pub mod db;
pub mod enrichment;
pub mod holidays;
pub mod models;
pub mod places;
pub mod scan;
pub mod time;

use crate::config::Config;
use anyhow::{bail, Context, Result};
use chrono_tz::Tz;
use db::TimelineDb;
use places::{CachedPlaceResolver, NominatimPlaceResolver, PlaceResolver};
use scan::ScanReport;
use serde::Serialize;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RescanReport {
    pub scan: ScanReport,
    pub albums_count: usize,
    pub enrichment: enrichment::EnrichmentReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    Starting,
    Scanning,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanStatus {
    pub state: ScanState,
    pub phase: String,
    pub found: usize,
    pub processed: usize,
    pub errors: usize,
    pub workers: usize,
    pub elapsed_seconds: u64,
    pub error: Option<String>,
    #[serde(skip)]
    started_at: Option<Instant>,
}

impl ScanStatus {
    fn starting(workers: usize) -> Self {
        Self {
            state: ScanState::Starting,
            phase: "starting".into(),
            found: 0,
            processed: 0,
            errors: 0,
            workers,
            elapsed_seconds: 0,
            error: None,
            started_at: None,
        }
    }

    fn update_elapsed(&mut self) {
        self.elapsed_seconds = self
            .started_at
            .map_or(0, |started| started.elapsed().as_secs());
    }
}




#[derive(Default)]
struct AiScheduleState {
    inner: StdMutex<AiScheduleInner>,
}

#[derive(Default)]
struct AiScheduleInner {
    running: bool,
    pending: Vec<ai::AiDescriptionInput>,
}
/// SQLite-backed orchestration for timeline mode.
pub struct TimelineService {
    config: Config,
    db: TimelineDb,
    timezone: Tz,
    ai: Option<Arc<dyn enrichment::AiDescriptionGenerator>>,
    ai_schedule: Arc<AiScheduleState>,
    scan_lock: Mutex<()>,
    status: Arc<StdMutex<ScanStatus>>,
}

impl TimelineService {
    pub fn open(config: Config) -> Result<Arc<Self>> {
        let timezone = parse_timezone(&config.timeline_timezone)?;
        let db = TimelineDb::open(config.data_path.join("lumiflow.sqlite"))?;
        let has_completed_scan = db.has_completed_scan()?;
        let ai = config
            .ai
            .enabled
            .then(|| ai::ResponsesAiClient::from_config(&config.ai))
            .transpose()?
            .map(|client| Arc::new(client) as Arc<dyn enrichment::AiDescriptionGenerator>);
        let workers = config.scan_workers;
        Ok(Arc::new(Self {
            config,
            db,
            timezone,
            ai,
            ai_schedule: Arc::new(AiScheduleState::default()),
            scan_lock: Mutex::new(()),
            status: Arc::new(StdMutex::new(if has_completed_scan {
                ScanStatus {
                    state: ScanState::Ready,
                    phase: "ready".into(),
                    workers,
                    ..ScanStatus::starting(workers)
                }
            } else {
                ScanStatus::starting(workers)
            })),
        }))
    }

    #[cfg(test)]
    pub(crate) fn from_db_for_test(config: Config, db: TimelineDb) -> Result<Self> {
        let timezone = parse_timezone(&config.timeline_timezone)?;
        let ai = config
            .ai
            .enabled
            .then(|| ai::ResponsesAiClient::from_config(&config.ai))
            .transpose()?
            .map(|client| Arc::new(client) as Arc<dyn enrichment::AiDescriptionGenerator>);
        let workers = config.scan_workers;
        Ok(Self {
            config,
            db,
            timezone,
            ai,
            ai_schedule: Arc::new(AiScheduleState::default()),
            scan_lock: Mutex::new(()),
            status: Arc::new(StdMutex::new(ScanStatus::starting(workers))),
        })
    }

    pub fn db(&self) -> &TimelineDb { &self.db }
    pub fn config(&self) -> &Config { &self.config }

    pub fn needs_initial_scan(&self) -> bool {
        self.status().state == ScanState::Starting
    }

    pub fn status(&self) -> ScanStatus {
        let mut status = self.status.lock().expect("scan status lock").clone();
        status.update_elapsed();
        status
    }

    pub fn start_initial_scan(self: &Arc<Self>) -> tokio::task::JoinHandle<Result<RescanReport>> {
        let service = self.clone();
        tokio::spawn(async move { service.rescan().await })
    }

    pub async fn rescan(&self) -> Result<RescanReport> {
        let _guard = self.scan_lock.lock().await;
        let config = self.config.clone();
        let db = self.db.clone();
        let timezone = self.timezone;
        let prepare_ai = self.ai.is_some();
        let status = self.status.clone();
        {
            let mut current = status.lock().expect("scan status lock");
            *current = ScanStatus {
                state: ScanState::Scanning,
                phase: "indexing".into(),
                workers: config.scan_workers,
                started_at: Some(Instant::now()),
                ..ScanStatus::starting(config.scan_workers)
            };
        }
        let result = tokio::task::spawn_blocking(move || {
            rescan_local_blocking(&config, &db, timezone, prepare_ai, Some(status.clone()))
        })
        .await
        .context("timeline rescan task failed")?;
        let (report, ai_inputs) = match result {
            Ok(result) => result,
            Err(error) => {
                let mut current = self.status.lock().expect("scan status lock");
                current.state = ScanState::Error;
                current.phase = "error".into();
                current.error = Some(format!("{error:#}"));
                current.update_elapsed();
                return Err(error);
            }
        };
        self.db.mark_scan_completed()?;
        if let Some(ai) = &self.ai {
            schedule_ai_enrichment(self.db.clone(), ai.clone(), self.ai_schedule.clone(), ai_inputs);
        }
        {
            let mut current = self.status.lock().expect("scan status lock");
            current.state = ScanState::Ready;
            current.phase = "ready".into();
            current.found = report.scan.found;
            current.processed = report.scan.analyzed + report.scan.reused + report.scan.errors;
            current.errors = report.scan.errors;
            current.update_elapsed();
        }
        Ok(report)
    }
}

fn rescan_local_blocking(
    config: &Config,
    db: &TimelineDb,
    timezone: Tz,
    prepare_ai: bool,
    status: Option<Arc<StdMutex<ScanStatus>>>,
) -> Result<(RescanReport, Vec<ai::AiDescriptionInput>)> {
    let scan = scan::scan_parallel(
        &config.photos_path,
        db,
        timezone,
        &scan::ExifAnalyzer,
        &config.exclude_regex,
        config.scan_workers,
        |report| {
            let processed = report.analyzed + report.reused + report.errors;
            if processed > 0 && processed % 100 == 0 {
                tracing::info!(
                    found = report.found,
                    processed,
                    errors = report.errors,
                    workers = config.scan_workers,
                    "initial photo index progress"
                );
            }
            if let Some(status) = &status {
                let mut current = status.lock().expect("scan status lock");
                current.found = report.found;
                current.processed = processed;
                current.errors = report.errors;
                current.update_elapsed();
            }
        },
    )?;
    if let Some(status) = &status {
        status.lock().expect("scan status lock").phase = "building_albums".into();
    }
    let places = build_place_resolver(config, db)?;
    let albums = albums::rebuild_daily_albums(db, timezone, places.as_ref())?;
    let (enrichment, ai_inputs) = enrichment::enrich_local(config, db, prepare_ai)?;
    Ok((
        RescanReport { scan, albums_count: albums.len(), enrichment },
        ai_inputs,
    ))
}

fn schedule_ai_enrichment(
    db: TimelineDb,
    generator: Arc<dyn enrichment::AiDescriptionGenerator>,
    state: Arc<AiScheduleState>,
    inputs: Vec<ai::AiDescriptionInput>,
) {
    if inputs.is_empty() { return; }
    let mut schedule = match state.inner.lock() {
        Ok(schedule) => schedule,
        Err(_) => { tracing::error!("AI schedule lock is poisoned"); return; }
    };
    schedule.pending = inputs;
    if schedule.running { return; }
    schedule.running = true;
    drop(schedule);
    tokio::spawn(async move {
        loop {
            let inputs = match state.inner.lock() {
                Ok(mut schedule) if schedule.pending.is_empty() => { schedule.running = false; return; }
                Ok(mut schedule) => std::mem::take(&mut schedule.pending),
                Err(_) => { tracing::error!("AI schedule lock is poisoned"); return; }
            };
            let mut report = enrichment::EnrichmentReport::default();
            enrichment::enrich_ai(&db, Some(generator.as_ref()), &inputs, &mut report).await;
            tracing::info!(generated_or_cached = report.ai_generated_or_cached, errors = report.ai_errors, "album AI enrichment pass finished");
        }
    });
}

fn build_place_resolver(config: &Config, db: &TimelineDb) -> Result<Box<dyn PlaceResolver>> {
    match config.place_provider.as_deref() {
        Some("nominatim") => Ok(Box::new(NominatimPlaceResolver::with_default_timeout(
            db.clone(),
            config.place_base_url.as_deref().context("LUMIFLOW_PLACE_BASE_URL is required for nominatim")?,
        )?)),
        None => Ok(Box::new(CachedPlaceResolver::new(db.clone()))),
        Some(provider) => bail!("unsupported place provider `{provider}`"),
    }
}

fn parse_timezone(value: &str) -> Result<Tz> {
    value
        .parse()
        .with_context(|| format!("invalid timeline timezone `{value}`"))
}

#[cfg(test)]
mod ai_scheduling_tests {
    use super::*;
    use crate::timeline::models::{AlbumAiDescription, TimelineAlbum};
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;
    use tokio::sync::Notify;

    struct GatedAi {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl enrichment::AiDescriptionGenerator for GatedAi {
        fn generate_or_reuse<'a>(
            &'a self,
            _db: &'a TimelineDb,
            input: &'a ai::AiDescriptionInput,
        ) -> Pin<Box<dyn Future<Output = Result<AlbumAiDescription>> + Send + 'a>> {
            Box::pin(async move {
                self.started.notify_one();
                self.release.notified().await;
                Ok(AlbumAiDescription {
                    album_id: input.album.id.clone(),
                    input_fingerprint: "fingerprint".into(),
                    model: "gated".into(),
                    description: "done".into(),
                    keywords: Vec::new(),
                    confidence: 1.0,
                    generated_at: "now".into(),
                    error: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn scheduling_ai_returns_before_remote_generation_finishes() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let generator: Arc<dyn enrichment::AiDescriptionGenerator> = Arc::new(GatedAi {
            started: started.clone(),
            release: release.clone(),
        });
        let input = ai::AiDescriptionInput {
            album: TimelineAlbum {
                id: "album".into(),
                name: "Album".into(),
                description: None,
                date_start: None,
                date_end: None,
                place: None,
                holiday: None,
                photo_count: 1,
                cover_photo_id: None,
            },
            time_range: None,
            camera_summary: Vec::new(),
            vision_tag_summary: Vec::new(),
            selected_photos: Vec::new(),
            contact_sheet_path: PathBuf::from("unused.jpg"),
        };

        schedule_ai_enrichment(
            TimelineDb::open_in_memory().expect("db"),
            generator,
            Arc::new(AiScheduleState::default()),
            vec![input],
        );
        tokio::time::timeout(std::time::Duration::from_millis(100), started.notified())
            .await
            .expect("AI task starts asynchronously");
        release.notify_one();
    }

    #[tokio::test]
    async fn scheduling_during_active_pass_processes_the_pending_batch() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let generator: Arc<dyn enrichment::AiDescriptionGenerator> = Arc::new(GatedAi {
            started: started.clone(),
            release: release.clone(),
        });
        let state = Arc::new(AiScheduleState::default());
        let db = TimelineDb::open_in_memory().expect("db");
        let input = |id: &str| ai::AiDescriptionInput {
            album: TimelineAlbum {
                id: id.into(),
                name: id.into(),
                description: None,
                date_start: None,
                date_end: None,
                place: None,
                holiday: None,
                photo_count: 1,
                cover_photo_id: None,
            },
            time_range: None,
            camera_summary: Vec::new(),
            vision_tag_summary: Vec::new(),
            selected_photos: Vec::new(),
            contact_sheet_path: PathBuf::from("unused.jpg"),
        };

        schedule_ai_enrichment(
            db.clone(),
            generator.clone(),
            state.clone(),
            vec![input("first")],
        );
        tokio::time::timeout(std::time::Duration::from_millis(100), started.notified())
            .await
            .expect("first starts");
        schedule_ai_enrichment(db, generator, state, vec![input("second")]);
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_millis(100), started.notified())
            .await
            .expect("pending batch starts after first");
        release.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiConfig, AlbumMode, VisionTagger as VisionTaggerConfig};
    use std::path::{Path, PathBuf};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "lumiflow-rescan-enrichment-{label}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn timeline_test_config(photos: &Path, data: &Path) -> Config {
        Config {
            photos_path: photos.to_path_buf(),
            data_path: data.to_path_buf(),
            bind_address: "127.0.0.1".into(),
            port: 4320,
            builder_workers: 1,
            scan_workers: 2,
            exclude_regex: r"$^".into(),
            album_mode: AlbumMode::Timeline,
            timeline_timezone: "UTC".into(),
            calendar_region: "CN_COMMON".into(),
            place_provider: None,
            place_base_url: None,
            vision_tagger: VisionTaggerConfig::None,
            vision_model_path: None,
            vision_labels_path: None,
            vision_workers: 1,
            ai: AiConfig {
                enabled: false,
                base_url: None,
                api_key: None,
                model: None,
                language: "zh-CN".into(),
            },
        }
    }

    #[tokio::test]
    async fn open_returns_before_initial_scan_and_reports_completion() {
        let photos = TestDir::new("initial-scan-photos");
        let data = TestDir::new("initial-scan-data");
        for relative in ["first/a.png", "first/nested/b.png", "second/c.png"] {
            let path = photos.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("photo parent"))
                .expect("create photo parent");
            image::RgbImage::from_pixel(2, 2, image::Rgb([20, 80, 160]))
                .save(path)
                .expect("save photo");
        }

        let service = TimelineService::open(timeline_test_config(&photos.0, &data.0))
            .expect("open timeline service");

        assert!(data.0.join("lumiflow.sqlite").is_file());
        assert_eq!(service.status().state, ScanState::Starting);
        let scan = service.start_initial_scan();
        scan.await.expect("scan task").expect("initial scan");
        assert_eq!(service.status().state, ScanState::Ready);
        assert_eq!(service.status().found, 3);
        assert_eq!(service.status().processed, 3);
        assert_eq!(
            service
                .db()
                .list_active_photos()
                .expect("indexed photos")
                .len(),
            3
        );

        let restarted = TimelineService::open(timeline_test_config(&photos.0, &data.0))
            .expect("reopen timeline service");
        assert_eq!(restarted.status().state, ScanState::Ready);
        assert!(!restarted.needs_initial_scan());
    }

    #[test]
    fn rescan_runs_local_enrichment_after_rebuilding_albums() {
        let photos = TestDir::new("photos");
        let data = TestDir::new("data");
        let photo_path = photos.0.join("nested/photo.png");
        std::fs::create_dir_all(photo_path.parent().expect("photo parent"))
            .expect("create photo parent");
        image::RgbImage::from_pixel(4, 3, image::Rgb([20, 80, 160]))
            .save(&photo_path)
            .expect("save photo");
        let config = Config {
            photos_path: photos.0.clone(),
            data_path: data.0.clone(),
            bind_address: "127.0.0.1".into(),
            port: 4320,
            builder_workers: 1,
            scan_workers: 2,
            exclude_regex: r"$^".into(),
            album_mode: AlbumMode::Timeline,
            timeline_timezone: "UTC".into(),
            calendar_region: "CN_COMMON".into(),
            place_provider: None,
            place_base_url: None,
            vision_tagger: VisionTaggerConfig::None,
            vision_model_path: None,
            vision_labels_path: None,
            vision_workers: 1,
            ai: AiConfig {
                enabled: false,
                base_url: None,
                api_key: None,
                model: None,
                language: "zh-CN".into(),
            },
        };
        let db = TimelineDb::open_in_memory().expect("db");
        let (report, ai_inputs) = rescan_local_blocking(
            &config,
            &db,
            chrono_tz::UTC,
            true,
            None,
        )
        .expect("rescan");
        assert_eq!(report.scan.found, 1);
        assert_eq!(report.albums_count, 1);
        assert_eq!(report.enrichment.thumbnails_generated, 1);
        assert!(ai_inputs.len() <= 1);
    }

    #[test]
    fn configured_place_resolver_rejects_invalid_nominatim_url_at_startup() {
        let photos = TestDir::new("place-photos");
        let data = TestDir::new("place-data");
        let mut config = Config {
            photos_path: photos.0.clone(),
            data_path: data.0.clone(),
            bind_address: "127.0.0.1".into(),
            port: 4320,
            builder_workers: 1,
            scan_workers: 2,
            exclude_regex: r"$^".into(),
            album_mode: AlbumMode::Timeline,
            timeline_timezone: "UTC".into(),
            calendar_region: "CN_COMMON".into(),
            place_provider: Some("nominatim".into()),
            place_base_url: Some("not a URL".into()),
            vision_tagger: VisionTaggerConfig::None,
            vision_model_path: None,
            vision_labels_path: None,
            vision_workers: 1,
            ai: AiConfig {
                enabled: false,
                base_url: None,
                api_key: None,
                model: None,
                language: "zh-CN".into(),
            },
        };
        let db = TimelineDb::open_in_memory().expect("db");

        let error = match build_place_resolver(&config, &db) {
            Ok(_) => panic!("invalid URL must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("invalid Nominatim base URL"));

        config.place_provider = None;
        config.place_base_url = Some("not a URL".into());
        assert!(build_place_resolver(&config, &db).is_ok());
    }
}
