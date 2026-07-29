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
pub mod vision;

use crate::config::{Config, VisionTagger as VisionTaggerConfig};
use anyhow::{bail, Context, Result};
use chrono_tz::Tz;
use db::TimelineDb;
use places::{CachedPlaceResolver, NominatimPlaceResolver, PlaceResolver};
use scan::ScanReport;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RescanReport {
    pub scan: ScanReport,
    pub albums_count: usize,
    pub enrichment: enrichment::EnrichmentReport,
}

struct VisionRuntime {
    tagger: Box<dyn vision::VisionTagger + Send>,
    tagset_version: String,
}

/// SQLite-backed orchestration for timeline mode.
pub struct TimelineService {
    config: Config,
    db: TimelineDb,
    timezone: Tz,
    vision: Option<Arc<StdMutex<VisionRuntime>>>,
    ai: Option<Arc<dyn enrichment::AiDescriptionGenerator>>,
    ai_schedule: Arc<AiScheduleState>,
    scan_lock: Mutex<()>,
}

impl TimelineService {
    /// Open and migrate the timeline database, then fully index the photo root.
    pub async fn open(config: Config) -> Result<Arc<Self>> {
        let timezone = parse_timezone(&config.timeline_timezone)?;
        let db = TimelineDb::open(config.data_path.join("lumiflow.sqlite"))?;
        let vision = build_vision_runtime(&config)?.map(|runtime| Arc::new(StdMutex::new(runtime)));
        let ai = config
            .ai
            .enabled
            .then(|| ai::ResponsesAiClient::from_config(&config.ai))
            .transpose()?
            .map(|client| Arc::new(client) as Arc<dyn enrichment::AiDescriptionGenerator>);
        let service = Arc::new(Self {
            config,
            db,
            timezone,
            vision,
            ai,
            ai_schedule: Arc::new(AiScheduleState::default()),
            scan_lock: Mutex::new(()),
        });
        service.rescan().await?;
        Ok(service)
    }

    #[cfg(test)]
    pub(crate) fn from_db_for_test(config: Config, db: TimelineDb) -> Result<Self> {
        let timezone = parse_timezone(&config.timeline_timezone)?;
        let vision = build_vision_runtime(&config)?.map(|runtime| Arc::new(StdMutex::new(runtime)));
        let ai = config
            .ai
            .enabled
            .then(|| ai::ResponsesAiClient::from_config(&config.ai))
            .transpose()?
            .map(|client| Arc::new(client) as Arc<dyn enrichment::AiDescriptionGenerator>);
        Ok(Self {
            config,
            db,
            timezone,
            vision,
            ai,
            ai_schedule: Arc::new(AiScheduleState::default()),
            scan_lock: Mutex::new(()),
        })
    }

    pub fn db(&self) -> &TimelineDb {
        &self.db
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Run blocking filesystem, EXIF, and SQLite work off the async runtime.
    /// The mutex prevents manual, periodic, and notify-triggered scans from overlapping.
    pub async fn rescan(&self) -> Result<RescanReport> {
        let _guard = self.scan_lock.lock().await;
        let config = self.config.clone();
        let db = self.db.clone();
        let blocking_db = db.clone();
        let timezone = self.timezone;
        let vision = self.vision.clone();
        let prepare_ai = self.ai.is_some();
        let (report, ai_inputs) = tokio::task::spawn_blocking(move || {
            if let Some(vision) = vision {
                let mut runtime = vision
                    .lock()
                    .map_err(|_| anyhow::anyhow!("vision runtime lock is poisoned"))?;
                let tagset_version = runtime.tagset_version.clone();
                rescan_local_blocking(
                    &config,
                    &blocking_db,
                    timezone,
                    Some(runtime.tagger.as_mut()),
                    &tagset_version,
                    prepare_ai,
                )
            } else {
                rescan_local_blocking(
                    &config,
                    &blocking_db,
                    timezone,
                    None,
                    "disabled",
                    prepare_ai,
                )
            }
        })
        .await
        .context("timeline rescan task failed")??;
        if let Some(ai) = &self.ai {
            schedule_ai_enrichment(db, ai.clone(), self.ai_schedule.clone(), ai_inputs);
        }
        Ok(report)
    }
}

fn rescan_local_blocking(
    config: &Config,
    db: &TimelineDb,
    timezone: Tz,
    tagger: Option<&mut dyn vision::VisionTagger>,
    tagset_version: &str,
    prepare_ai: bool,
) -> Result<(RescanReport, Vec<ai::AiDescriptionInput>)> {
    let scan = scan::scan_with_exclude(
        &config.photos_path,
        db,
        timezone,
        &scan::ExifAnalyzer,
        &config.exclude_regex,
    )?;
    let places = build_place_resolver(config, db)?;
    let albums = albums::rebuild_daily_albums(db, timezone, places.as_ref())?;
    let (enrichment, ai_inputs) =
        enrichment::enrich_local(config, db, tagger, tagset_version, prepare_ai)?;
    Ok((
        RescanReport {
            scan,
            albums_count: albums.len(),
            enrichment,
        },
        ai_inputs,
    ))
}

fn build_place_resolver(config: &Config, db: &TimelineDb) -> Result<Box<dyn PlaceResolver>> {
    match config.place_provider.as_deref() {
        Some("nominatim") => Ok(Box::new(NominatimPlaceResolver::with_default_timeout(
            db.clone(),
            config
                .place_base_url
                .as_deref()
                .context("LUMIFLOW_PLACE_BASE_URL is required for nominatim")?,
        )?)),
        None => Ok(Box::new(CachedPlaceResolver::new(db.clone()))),
        Some(provider) => bail!("unsupported place provider `{provider}`"),
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

fn schedule_ai_enrichment(
    db: TimelineDb,
    generator: Arc<dyn enrichment::AiDescriptionGenerator>,
    state: Arc<AiScheduleState>,
    inputs: Vec<ai::AiDescriptionInput>,
) {
    if inputs.is_empty() {
        return;
    }
    let mut schedule = match state.inner.lock() {
        Ok(schedule) => schedule,
        Err(_) => {
            tracing::error!("AI schedule lock is poisoned");
            return;
        }
    };
    schedule.pending = inputs;
    if schedule.running {
        return;
    }
    schedule.running = true;
    drop(schedule);
    tokio::spawn(async move {
        loop {
            let inputs = match state.inner.lock() {
                Ok(mut schedule) if schedule.pending.is_empty() => {
                    schedule.running = false;
                    return;
                }
                Ok(mut schedule) => std::mem::take(&mut schedule.pending),
                Err(_) => {
                    tracing::error!("AI schedule lock is poisoned");
                    return;
                }
            };
            let mut report = enrichment::EnrichmentReport::default();
            enrichment::enrich_ai(&db, Some(generator.as_ref()), &inputs, &mut report).await;
            tracing::info!(
                generated_or_cached = report.ai_generated_or_cached,
                errors = report.ai_errors,
                "album AI enrichment pass finished"
            );
        }
    });
}

fn build_vision_runtime(config: &Config) -> Result<Option<VisionRuntime>> {
    match config.vision_tagger {
        VisionTaggerConfig::None => Ok(None),
        VisionTaggerConfig::OnnxMobileClip => {
            #[cfg(feature = "vision-onnx")]
            {
                let model_path = config
                    .vision_model_path
                    .as_deref()
                    .context("LUMIFLOW_VISION_MODEL_PATH is required for onnx-mobileclip")?;
                let labels_path = config
                    .vision_labels_path
                    .as_deref()
                    .context("LUMIFLOW_VISION_LABELS_PATH is required for onnx-mobileclip")?;
                let tagger = vision::OnnxMobileClipTagger::load_with_threads(
                    model_path,
                    labels_path,
                    config.vision_workers,
                )?;
                let tagset_version = tagger.tagset_version().to_owned();
                Ok(Some(VisionRuntime {
                    tagger: Box::new(tagger),
                    tagset_version,
                }))
            }
            #[cfg(not(feature = "vision-onnx"))]
            {
                bail!(
                    "onnx-mobileclip requires a build with the `vision-onnx` Cargo feature; no inference was attempted"
                )
            }
        }
        VisionTaggerConfig::OpenVinoMobileClip => bail!(
            "openvino-mobileclip is not available in this build; select `none` or `onnx-mobileclip`"
        ),
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
    use crate::timeline::vision::{VisionTag, VisionTagger};
    use std::path::{Path, PathBuf};

    struct FixedTagger;

    impl VisionTagger for FixedTagger {
        fn model_id(&self) -> &str {
            "fixed-model"
        }

        fn tag(&mut self, _thumbnail_path: &Path) -> Result<Vec<VisionTag>> {
            Ok(vec![VisionTag {
                label: "garden".into(),
                score: 1.0,
            }])
        }
    }

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
    async fn open_fully_indexes_nested_photos_into_sqlite_before_returning() {
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
            .await
            .expect("open timeline service");

        assert!(data.0.join("lumiflow.sqlite").is_file());
        assert_eq!(
            service
                .db()
                .list_active_photos()
                .expect("indexed photos")
                .len(),
            3
        );
        assert_eq!(
            service
                .db()
                .list_albums()
                .expect("generated albums")
                .iter()
                .map(|album| album.photo_count)
                .sum::<usize>(),
            3,
        );
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
        let mut tagger = FixedTagger;

        let (report, ai_inputs) = rescan_local_blocking(
            &config,
            &db,
            chrono_tz::UTC,
            Some(&mut tagger),
            "fixed-tags-v1",
            true,
        )
        .expect("rescan with enrichment");
        assert_eq!(report.scan.found, 1);
        assert_eq!(report.albums_count, 1);
        assert_eq!(report.enrichment.thumbnails_generated, 1);
        assert_eq!(report.enrichment.vision_tagged, 1);
        assert_eq!(report.enrichment.contact_sheets_generated, 1);
        assert_eq!(ai_inputs.len(), 1);
        assert!(ai_inputs[0].contact_sheet_path.is_file());
        assert_eq!(ai_inputs[0].vision_tag_summary, vec!["garden (1.00)"]);
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
