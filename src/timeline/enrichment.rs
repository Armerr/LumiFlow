use crate::config::Config;
use crate::thumbnail::{
    timeline_thumb_is_fresh, timeline_thumb_path, write_timeline_thumb_fingerprint, ThumbnailPool,
};
use crate::timeline::ai::{AiDescriptionInput, ResponsesAiClient, SelectedPhotoSignature};
use crate::timeline::contact_sheet::{render_contact_sheet, representative_indices};
use crate::timeline::db::TimelineDb;
use crate::timeline::models::{AlbumAiDescription, TimelinePhoto};
use anyhow::{Context, Result};
use chrono::{DateTime, Timelike};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub fn enrich_local(
    config: &Config,
    db: &TimelineDb,
    prepare_ai: bool,
) -> Result<(EnrichmentReport, Vec<AiDescriptionInput>)> {
    if !prepare_ai {
        let photos = db
            .list_active_photos()
            .context("failed to list photos for enrichment")?;
        let canonical_root = config.photos_path.canonicalize().with_context(|| {
            format!(
                "failed to resolve photo root {}",
                config.photos_path.display()
            )
        })?;
        let mut report = EnrichmentReport::default();
        for photo in &photos {
            let thumbnail = timeline_thumb_path(&config.data_path, &photo.id);
            if timeline_thumb_is_fresh(&config.data_path, &photo.id, &photo.fingerprint) {
                report.thumbnails_reused += 1;
                continue;
            }
            let generated = resolve_source(&canonical_root, &photo.relative_path).and_then(|source| {
                ThumbnailPool::generate_on_demand(&source, &thumbnail).and_then(|_| {
                    write_timeline_thumb_fingerprint(
                        &config.data_path,
                        &photo.id,
                        &photo.fingerprint,
                    )
                    .context("failed to write thumbnail fingerprint")
                })
            });
            match generated {
                Ok(()) => report.thumbnails_generated += 1,
                Err(error) => {
                    report.thumbnail_errors += 1;
                    tracing::warn!(photo_id = %photo.id, error = %error, "timeline thumbnail enrichment failed");
                }
            }
        }
        return Ok((report, Vec::new()));
    }

    let photos = db
        .list_active_photos()
        .context("failed to list photos for enrichment")?;
    let canonical_root = config.photos_path.canonicalize().with_context(|| {
        format!(
            "failed to resolve photo root {}",
            config.photos_path.display()
        )
    })?;
    let mut report = EnrichmentReport::default();
    for photo in &photos {
        let thumbnail = timeline_thumb_path(&config.data_path, &photo.id);
        if timeline_thumb_is_fresh(&config.data_path, &photo.id, &photo.fingerprint) {
            report.thumbnails_reused += 1;
        } else {
            let generated = resolve_source(&canonical_root, &photo.relative_path).and_then(|source| {
                ThumbnailPool::generate_on_demand(&source, &thumbnail).and_then(|_| {
                    write_timeline_thumb_fingerprint(
                        &config.data_path,
                        &photo.id,
                        &photo.fingerprint,
                    )
                    .context("failed to write thumbnail fingerprint")
                })
            });
            match generated {
                Ok(()) => report.thumbnails_generated += 1,
                Err(error) => {
                    report.thumbnail_errors += 1;
                    tracing::warn!(photo_id = %photo.id, error = %error, "timeline thumbnail enrichment failed");
                    continue;
                }
            }
        }
    }

    let camera_by_photo = db
        .list_active_photo_cameras()
        .context("failed to list camera metadata for enrichment")?
        .into_iter()
        .map(|(id, make, model)| (id, camera_name(make.as_deref(), model.as_deref())))
        .collect::<HashMap<_, _>>();
    let mut inputs = Vec::new();
    for album in db
        .list_albums()
        .context("failed to list albums for enrichment")?
    {
        let Some(detail) = db
            .get_album(&album.id)
            .context("failed to read album for enrichment")?
        else {
            continue;
        };
        let indices = representative_indices(detail.photos.len(), 36);
        if indices.is_empty() {
            continue;
        }
        let selected = indices
            .iter()
            .map(|&index| &detail.photos[index])
            .collect::<Vec<_>>();
        let thumbnail_paths = selected
            .iter()
            .map(|photo| timeline_thumb_path(&config.data_path, &photo.id))
            .collect::<Vec<_>>();
        let sheet = contact_sheet_path(&config.data_path, &album.id);
        let fingerprint = contact_sheet_fingerprint(&selected);
        match ensure_contact_sheet(&thumbnail_paths, &sheet, &fingerprint) {
            Ok(true) => report.contact_sheets_generated += 1,
            Ok(false) => {}
            Err(error) => {
                report.contact_sheet_errors += 1;
                tracing::warn!(album_id = %album.id, error = %error, "album contact-sheet enrichment failed");
                continue;
            }
        }
        let signatures = selected
            .iter()
            .map(|photo| SelectedPhotoSignature {
                photo_id: photo.id.clone(),
                photo_fingerprint: photo.fingerprint.clone(),
                vision_input_fingerprint: None,
            })
            .collect();
        inputs.push(AiDescriptionInput {
            album: detail.album,
            time_range: time_range(&detail.photos),
            camera_summary: camera_summary(&detail.photos, &camera_by_photo),
            vision_tag_summary: Vec::new(),
            selected_photos: signatures,
            contact_sheet_path: sheet,
        });
    }
    Ok((report, inputs))
}

fn resolve_source(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let source = root.join(relative_path);
    let canonical = source
        .canonicalize()
        .with_context(|| format!("failed to resolve enrichment source {}", source.display()))?;
    anyhow::ensure!(
        canonical.starts_with(root) && canonical.is_file(),
        "enrichment source escaped photo root: {}",
        source.display()
    );
    Ok(canonical)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EnrichmentReport {
    pub thumbnails_generated: usize,
    pub thumbnail_errors: usize,
    pub thumbnails_reused: usize,
    pub vision_tagged: usize,
    pub vision_cached: usize,
    pub vision_errors: usize,
    pub contact_sheets_generated: usize,
    pub contact_sheet_errors: usize,
    pub ai_generated_or_cached: usize,
    pub ai_errors: usize,
}

pub trait AiDescriptionGenerator: Send + Sync + 'static {
    fn generate_or_reuse<'a>(
        &'a self,
        db: &'a TimelineDb,
        input: &'a AiDescriptionInput,
    ) -> Pin<Box<dyn Future<Output = Result<AlbumAiDescription>> + Send + 'a>>;
}

impl AiDescriptionGenerator for ResponsesAiClient {
    fn generate_or_reuse<'a>(
        &'a self,
        db: &'a TimelineDb,
        input: &'a AiDescriptionInput,
    ) -> Pin<Box<dyn Future<Output = Result<AlbumAiDescription>> + Send + 'a>> {
        Box::pin(crate::timeline::ai::generate_or_reuse(db, self, input))
    }
}

fn contact_sheet_path(data_path: &Path, album_id: &str) -> PathBuf {
    data_path
        .join("ai/contact-sheets")
        .join(format!("{}.jpg", sha1_hex(album_id.as_bytes())))
}

fn ensure_contact_sheet(thumbnails: &[PathBuf], output: &Path, fingerprint: &str) -> Result<bool> {
    let sidecar = output.with_extension("fingerprint");
    if output.is_file() && std::fs::read_to_string(&sidecar).is_ok_and(|value| value == fingerprint)
    {
        return Ok(false);
    }
    render_contact_sheet(thumbnails, output)?;
    std::fs::write(&sidecar, fingerprint)
        .with_context(|| format!("failed to write {}", sidecar.display()))?;
    Ok(true)
}

fn contact_sheet_fingerprint(photos: &[&TimelinePhoto]) -> String {
    let mut hasher = Sha1::new();
    for photo in photos {
        hasher.update(photo.id.len().to_be_bytes());
        hasher.update(photo.id.as_bytes());
        hasher.update(photo.fingerprint.len().to_be_bytes());
        hasher.update(photo.fingerprint.as_bytes());
    }
    hex_digest(hasher.finalize())
}

fn camera_name(make: Option<&str>, model: Option<&str>) -> Option<String> {
    let make = make.map(str::trim).filter(|value| !value.is_empty());
    let model = model.map(str::trim).filter(|value| !value.is_empty());
    match (make, model) {
        (Some(make), Some(model)) if model.starts_with(make) => Some(model.to_owned()),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (Some(value), None) | (None, Some(value)) => Some(value.to_owned()),
        (None, None) => None,
    }
}

fn camera_summary(
    photos: &[TimelinePhoto],
    cameras: &HashMap<String, Option<String>>,
) -> Vec<String> {
    let mut counts = BTreeMap::new();
    for photo in photos {
        if let Some(Some(camera)) = cameras.get(&photo.id) {
            *counts.entry(camera.as_str()).or_insert(0usize) += 1;
        }
    }
    let mut values = counts.into_iter().collect::<Vec<_>>();
    values.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    values
        .into_iter()
        .map(|(camera, count)| format!("{camera} × {count}"))
        .collect()
}

fn time_range(photos: &[TimelinePhoto]) -> Option<String> {
    let mut times = photos
        .iter()
        .filter_map(|photo| {
            photo
                .taken_at
                .as_deref()?
                .parse::<DateTime<chrono::FixedOffset>>()
                .ok()
        })
        .map(|time| (time.hour(), time.minute()));
    let first = times.next()?;
    let (minimum, maximum) = times.fold((first, first), |(minimum, maximum), value| {
        (minimum.min(value), maximum.max(value))
    });
    Some(format!(
        "{:02}:{:02}–{:02}:{:02}",
        minimum.0, minimum.1, maximum.0, maximum.1
    ))
}

fn sha1_hex(value: &[u8]) -> String {
    hex_digest(Sha1::digest(value))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let mut result = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

pub async fn enrich_ai(
    db: &TimelineDb,
    generator: Option<&dyn AiDescriptionGenerator>,
    inputs: &[AiDescriptionInput],
    report: &mut EnrichmentReport,
) {
    let Some(generator) = generator else {
        return;
    };
    for input in inputs {
        match generator.generate_or_reuse(db, input).await {
            Ok(_) => report.ai_generated_or_cached += 1,
            Err(error) => {
                report.ai_errors += 1;
                tracing::warn!(album_id = %input.album.id, error = %error, "album AI enrichment failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiConfig, AlbumMode, VisionTagger as VisionTaggerConfig};
    use crate::timeline::models::{
        AnalysisDecision, DailyAlbumBuild, PhotoAnalysis, PhotoCandidate, TimeSource, TimelineAlbum,
    };
    use chrono::NaiveDate;
    use image::{ImageBuffer, Rgb};
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "lumiflow-enrichment-{label}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct RecordingAi {
        album_ids: Mutex<Vec<String>>,
        fail_album: Option<String>,
    }

    impl AiDescriptionGenerator for RecordingAi {
        fn generate_or_reuse<'a>(
            &'a self,
            _db: &'a TimelineDb,
            input: &'a AiDescriptionInput,
        ) -> Pin<Box<dyn Future<Output = Result<AlbumAiDescription>> + Send + 'a>> {
            Box::pin(async move {
                self.album_ids
                    .lock()
                    .expect("AI calls")
                    .push(input.album.id.clone());
                if self.fail_album.as_deref() == Some(input.album.id.as_str()) {
                    anyhow::bail!("intentional AI failure");
                }
                Ok(AlbumAiDescription {
                    album_id: input.album.id.clone(),
                    input_fingerprint: "fp".into(),
                    model: "fake".into(),
                    description: "description".into(),
                    keywords: vec![],
                    confidence: 1.0,
                    generated_at: "now".into(),
                    error: None,
                })
            })
        }
    }

    #[test]
    fn local_enrichment_generates_reuses_isolates_and_aggregates_inputs() {
        let photos = TestDir::new("photos");
        let data = TestDir::new("data");
        let db = TimelineDb::open_in_memory().expect("db");
        insert_photo(
            &db,
            photos.path(),
            "early",
            "2024-04-12T09:00:00+00:00",
            [255, 0, 0],
            "Canon",
            "R5",
        );
        insert_photo(
            &db,
            photos.path(),
            "broken",
            "2024-04-12T10:00:00+00:00",
            [0, 255, 0],
            "Canon",
            "R5",
        );
        insert_photo(
            &db,
            photos.path(),
            "late",
            "2024-04-12T11:00:00+00:00",
            [0, 0, 255],
            "Nikon",
            "Z8",
        );
        db.replace_daily_albums(&[album("day", &["early", "broken", "late"])])
            .expect("album");
        let config = config(photos.path(), data.path());

        let (first, inputs) = enrich_local(&config, &db, true).expect("first enrichment");

        assert_eq!(first.thumbnails_generated, 3);
        assert_eq!(first.thumbnails_reused, 0);
        assert_eq!(first.contact_sheets_generated, 1);
        assert_eq!(first.contact_sheet_errors, 0);
        assert_eq!(inputs.len(), 1);
        let input = &inputs[0];
        assert_eq!(input.time_range.as_deref(), Some("09:00–11:00"));
        assert_eq!(input.camera_summary, ["Canon R5 × 2", "Nikon Z8 × 1"]);

        let (second, _) = enrich_local(&config, &db, true).expect("second enrichment");
        assert_eq!(second.thumbnails_reused, 3);
        assert_eq!(second.thumbnails_generated, 0);
        assert_eq!(second.contact_sheets_generated, 0);
    }

    #[cfg(unix)]
    #[test]
    fn enrichment_never_follows_indexed_path_outside_photo_root() {
        use std::os::unix::fs::symlink;

        let photos = TestDir::new("containment-photos");
        let outside = TestDir::new("containment-outside");
        let data = TestDir::new("containment-data");
        let db = TimelineDb::open_in_memory().expect("db");
        insert_photo(
            &db,
            photos.path(),
            "escape/photo",
            "2024-04-12T09:00:00+00:00",
            [1, 2, 3],
            "",
            "",
        );
        let indexed_directory = photos.path().join("escape");
        std::fs::remove_dir_all(&indexed_directory).expect("remove indexed directory");
        let outside_photo = outside.path().join("photo.png");
        ImageBuffer::from_pixel(8, 8, Rgb([250_u8, 10, 20]))
            .save(&outside_photo)
            .expect("outside photo");
        symlink(outside.path(), &indexed_directory).expect("replace directory with symlink");
        db.replace_daily_albums(&[album("day", &["escape/photo"])])
            .expect("album");

        let (report, inputs) = enrich_local(
            &config(photos.path(), data.path()),
            &db,
            true,
        )
        .expect("isolated enrichment failure");

        assert_eq!(report.thumbnail_errors, 1);
        assert!(inputs.is_empty());
        assert!(!timeline_thumb_path(data.path(), "escape/photo").exists());
    }

    #[test]
    fn local_enrichment_selects_at_most_thirty_six_chronological_photos() {
        let photos = TestDir::new("selection-photos");
        let data = TestDir::new("selection-data");
        let db = TimelineDb::open_in_memory().expect("db");
        let mut ids = Vec::new();
        for index in 0..50 {
            let id = format!("p{index:02}");
            insert_photo(
                &db,
                photos.path(),
                &id,
                &format!("2024-04-12T{:02}:00:00+00:00", index % 24),
                [index as u8, 0, 0],
                "",
                "",
            );
            ids.push(id);
        }
        let refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
        db.replace_daily_albums(&[album("many", &refs)])
            .expect("album");
        let config = config(photos.path(), data.path());

        let (report, inputs) = enrich_local(&config, &db, true).expect("enrich");
        assert_eq!(inputs[0].selected_photos.len(), 36);
        let expected = crate::timeline::contact_sheet::representative_indices(50, 36)
            .into_iter()
            .map(|index| ids[index].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            inputs[0]
                .selected_photos
                .iter()
                .map(|photo| photo.photo_id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(inputs[0]
            .contact_sheet_path
            .starts_with(data.path().join("ai/contact-sheets")));
        assert_eq!(
            report.vision_tagged + report.vision_cached + report.vision_errors,
            0
        );
    }

    #[test]
    fn disabled_local_enrichment_does_no_work() {
        let photos = TestDir::new("disabled-photos");
        let data = TestDir::new("disabled-data");
        let db = TimelineDb::open_in_memory().expect("db");
        insert_photo(
            &db,
            photos.path(),
            "ignored",
            "2024-04-12T09:00:00+00:00",
            [1, 2, 3],
            "",
            "",
        );
        let config = config(photos.path(), data.path());
        let (report, inputs) = enrich_local(&config, &db, false).expect("disabled");
        assert_eq!(report, EnrichmentReport::default());
        assert!(inputs.is_empty());
        assert!(!data.path().join("thumbs").exists());
    }

    #[tokio::test]
    async fn ai_enrichment_is_optional_and_isolates_album_failures() {
        let db = TimelineDb::open_in_memory().expect("db");
        let mut report = EnrichmentReport::default();
        enrich_ai(&db, None, &[], &mut report).await;
        assert_eq!(report, EnrichmentReport::default());

        let inputs = [ai_input("good"), ai_input("bad")];
        let ai = RecordingAi {
            album_ids: Mutex::new(vec![]),
            fail_album: Some("bad".into()),
        };
        enrich_ai(&db, Some(&ai), &inputs, &mut report).await;
        assert_eq!(report.ai_generated_or_cached, 1);
        assert_eq!(report.ai_errors, 1);
        assert_eq!(*ai.album_ids.lock().expect("calls"), ["good", "bad"]);
    }

    fn config(photos: &Path, data: &Path) -> Config {
        Config {
            photos_path: photos.to_owned(),
            data_path: data.to_owned(),
            bind_address: "127.0.0.1".into(),
            port: 4320,
            builder_workers: 1,
            scan_workers: 2,
            exclude_regex: String::new(),
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
                language: "English".into(),
            },
        }
    }

    fn insert_photo(
        db: &TimelineDb,
        root: &Path,
        id: &str,
        taken_at: &str,
        color: [u8; 3],
        make: &str,
        model: &str,
    ) {
        let relative_path = format!("{id}.png");
        std::fs::create_dir_all(
            root.join(&relative_path)
                .parent()
                .expect("photo fixture parent"),
        )
        .expect("create photo fixture parent");
        ImageBuffer::<Rgb<u8>, _>::from_pixel(2, 2, Rgb(color))
            .save(root.join(&relative_path))
            .expect("photo");
        assert_eq!(
            db.upsert_candidate(&PhotoCandidate {
                id: id.into(),
                relative_path: relative_path.clone(),
                filename: relative_path,
                extension: "png".into(),
                size_bytes: 12,
                mtime_ns: 1,
                fingerprint: format!("fp-{id}"),
                scan_id: "scan".into()
            })
            .expect("candidate"),
            AnalysisDecision::Analyze
        );
        db.save_analysis(&PhotoAnalysis {
            id: id.into(),
            taken_at: Some(taken_at.into()),
            time_source: TimeSource::Exif,
            timezone: Some("UTC".into()),
            gps_lat: None,
            gps_lon: None,
            width: 2,
            height: 2,
            camera_make: (!make.is_empty()).then(|| make.into()),
            camera_model: (!model.is_empty()).then(|| model.into()),
            lens: None,
            exif_json: json!({"make": make, "model": model}),
        })
        .expect("analysis");
    }

    fn album(id: &str, photo_ids: &[&str]) -> DailyAlbumBuild {
        DailyAlbumBuild {
            album: TimelineAlbum {
                id: id.into(),
                name: id.into(),
                description: None,
                date_start: NaiveDate::from_ymd_opt(2024, 4, 12),
                date_end: NaiveDate::from_ymd_opt(2024, 4, 12),
                place: None,
                holiday: None,
                photo_count: photo_ids.len(),
                cover_photo_id: photo_ids.first().map(|id| (*id).into()),
            },
            photo_ids: photo_ids.iter().map(|id| (*id).into()).collect(),
        }
    }

    fn ai_input(id: &str) -> AiDescriptionInput {
        AiDescriptionInput {
            album: album(id, &[]).album,
            time_range: None,
            camera_summary: vec![],
            vision_tag_summary: vec![],
            selected_photos: vec![],
            contact_sheet_path: PathBuf::new(),
        }
    }
}
