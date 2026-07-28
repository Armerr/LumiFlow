use crate::exif::ExifData;
use crate::timeline::db::TimelineDb;
use crate::timeline::models::{AnalysisDecision, PhotoAnalysis, PhotoCandidate};
use crate::timeline::time::{resolve_taken_at, TimeInput};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use sha1::{Digest, Sha1};
use std::ffi::OsStr;
use std::fs::Metadata;
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::{DirEntry, WalkDir};

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "heic", "heif", "avif", "tif", "tiff",
];
const EXCLUDED_COMPONENTS: &[&str] = &["@eaDir", "#recycle"];

pub trait Analyzer {
    fn analyze(&self, path: &Path, photo_id: &str, timezone: Tz) -> Result<PhotoAnalysis>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExifAnalyzer;

impl Analyzer for ExifAnalyzer {
    fn analyze(&self, path: &Path, photo_id: &str, timezone: Tz) -> Result<PhotoAnalysis> {
        let metadata = path
            .metadata()
            .with_context(|| format!("failed to stat photo {}", path.display()))?;
        let exif = crate::exif::extract_exif(path)
            .with_context(|| format!("failed to analyze photo {}", path.display()))?;
        analysis_from_exif(
            photo_id,
            path.file_name()
                .and_then(OsStr::to_str)
                .context("photo filename is not valid UTF-8")?,
            metadata.modified().context("photo mtime is unavailable")?,
            exif,
            timezone,
        )
    }
}

pub fn scan(
    root: &Path,
    db: &TimelineDb,
    timezone: Tz,
    analyzer: &impl Analyzer,
) -> Result<ScanReport> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve timeline root {}", root.display()))?;
    anyhow::ensure!(
        canonical_root.is_dir(),
        "timeline root is not a directory: {}",
        root.display()
    );

    let scan_id = make_scan_id();
    let mut report = ScanReport::default();
    let walker = WalkDir::new(&canonical_root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(should_descend);

    for result in walker {
        let entry = result.with_context(|| {
            format!(
                "incomplete timeline walk under {}",
                canonical_root.display()
            )
        })?;
        if !entry.file_type().is_file() || !is_supported(entry.path()) {
            continue;
        }

        let canonical_path = entry
            .path()
            .canonicalize()
            .with_context(|| format!("failed to resolve photo {}", entry.path().display()))?;
        anyhow::ensure!(
            canonical_path.starts_with(&canonical_root),
            "photo escaped timeline root: {}",
            entry.path().display()
        );
        let relative_path = normalized_relative_path(&canonical_root, &canonical_path)?;
        let metadata = canonical_path
            .metadata()
            .with_context(|| format!("failed to stat photo {}", canonical_path.display()))?;
        let candidate = candidate(&relative_path, &metadata, &scan_id)?;
        report.found += 1;

        match db.upsert_candidate(&candidate)? {
            AnalysisDecision::Reuse => report.reused += 1,
            AnalysisDecision::Analyze => {
                let analysis = analyzer.analyze(&canonical_path, &candidate.id, timezone)?;
                anyhow::ensure!(
                    analysis.id == candidate.id,
                    "analyzer returned mismatched photo id `{}` for `{}`",
                    analysis.id,
                    candidate.id
                );
                db.save_analysis(&analysis)?;
                report.analyzed += 1;
            }
        }
    }

    report.marked_missing = db.mark_missing_except(&scan_id)?;
    Ok(report)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanReport {
    pub found: usize,
    pub analyzed: usize,
    pub reused: usize,
    pub marked_missing: usize,
}

fn should_descend(entry: &DirEntry) -> bool {
    entry.depth() == 0
        || (!entry.file_type().is_symlink()
            && !EXCLUDED_COMPONENTS
                .iter()
                .any(|excluded| entry.file_name() == OsStr::new(excluded)))
}

fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn normalized_relative_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .context("photo is outside timeline root")?;
    let mut normalized = String::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            anyhow::bail!("photo path contains a non-normal component");
        };
        let component = component
            .to_str()
            .context("photo path is not valid UTF-8")?;
        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(component);
    }
    anyhow::ensure!(!normalized.is_empty(), "photo relative path is empty");
    Ok(normalized)
}

fn candidate(relative_path: &str, metadata: &Metadata, scan_id: &str) -> Result<PhotoCandidate> {
    let filename = relative_path
        .rsplit('/')
        .next()
        .context("photo filename is missing")?;
    let extension = Path::new(filename)
        .extension()
        .and_then(OsStr::to_str)
        .context("photo extension is not valid UTF-8")?
        .to_ascii_lowercase();
    let mtime_ns = modified_ns(metadata)?;
    let size_bytes = metadata.len();
    Ok(PhotoCandidate {
        id: sha1_hex(relative_path.as_bytes()),
        relative_path: relative_path.into(),
        filename: filename.into(),
        extension,
        size_bytes,
        mtime_ns,
        fingerprint: photo_fingerprint(relative_path, size_bytes, mtime_ns),
        scan_id: scan_id.into(),
    })
}

fn modified_ns(metadata: &Metadata) -> Result<i64> {
    let modified = metadata.modified().context("photo mtime is unavailable")?;
    let nanoseconds = modified
        .duration_since(UNIX_EPOCH)
        .context("photo mtime predates Unix epoch")?
        .as_nanos();
    i64::try_from(nanoseconds).context("photo mtime exceeds supported range")
}

fn photo_fingerprint(relative_path: &str, size_bytes: u64, mtime_ns: i64) -> String {
    let mut hasher = Sha1::new();
    hasher.update(relative_path.as_bytes());
    hasher.update([0]);
    hasher.update(size_bytes.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(mtime_ns.to_string().as_bytes());
    hex_digest(hasher.finalize())
}

fn sha1_hex(bytes: &[u8]) -> String {
    hex_digest(Sha1::digest(bytes))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(digest.len() * 2);
    for &byte in digest {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn make_scan_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{timestamp}", std::process::id())
}

fn analysis_from_exif(
    id: &str,
    filename: &str,
    modified: SystemTime,
    exif: ExifData,
    timezone: Tz,
) -> Result<PhotoAnalysis> {
    let mtime: DateTime<Utc> = modified.into();
    let resolved = resolve_taken_at(
        &TimeInput {
            exif_datetime: exif.date_taken.clone(),
            exif_offset: exif.timezone.clone(),
            filename: filename.into(),
            mtime,
        },
        timezone,
    );
    let gps = exif.gps.as_ref();
    let exif_json = serde_json::to_value(&exif)?;
    Ok(PhotoAnalysis {
        id: id.into(),
        taken_at: resolved
            .as_ref()
            .map(|resolved| resolved.timestamp.to_rfc3339()),
        time_source: resolved
            .as_ref()
            .map_or(crate::timeline::models::TimeSource::Unknown, |resolved| {
                resolved.source
            }),
        timezone: exif.timezone.clone(),
        gps_lat: gps.map(|gps| gps.lat),
        gps_lon: gps.map(|gps| gps.lon),
        width: exif.dimensions.width,
        height: exif.dimensions.height,
        camera_make: exif.make.clone(),
        camera_model: exif.model.clone(),
        lens: exif.lens.clone(),
        exif_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::models::TimeSource;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let sequence = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "lumiflow-timeline-scan-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative_path: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(relative_path);
            fs::create_dir_all(path.parent().expect("file parent")).expect("create parent");
            fs::write(&path, bytes).expect("write test file");
            path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct CountingAnalyzer {
        calls: AtomicUsize,
    }

    impl CountingAnalyzer {
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Analyzer for CountingAnalyzer {
        fn analyze(&self, path: &Path, photo_id: &str, _timezone: Tz) -> Result<PhotoAnalysis> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (width, height) = crate::thumbnail::get_dimensions(path).unwrap_or((0, 0));
            Ok(PhotoAnalysis {
                id: photo_id.into(),
                taken_at: Some("2024-02-10T09:13:00+08:00".into()),
                time_source: TimeSource::Exif,
                timezone: Some("+08:00".into()),
                gps_lat: None,
                gps_lon: None,
                width,
                height,
                camera_make: None,
                camera_model: None,
                lens: None,
                exif_json: json!({"path": path.file_name()}),
            })
        }
    }

    struct FailOnNameAnalyzer<'a> {
        inner: &'a CountingAnalyzer,
        filename: &'a str,
    }

    impl Analyzer for FailOnNameAnalyzer<'_> {
        fn analyze(&self, path: &Path, photo_id: &str, timezone: Tz) -> Result<PhotoAnalysis> {
            if path.file_name() == Some(OsStr::new(self.filename)) {
                anyhow::bail!("intentional analysis failure");
            }
            self.inner.analyze(path, photo_id, timezone)
        }
    }

    fn active_paths(db: &TimelineDb) -> Vec<String> {
        db.list_active_photos()
            .expect("list active photos")
            .into_iter()
            .map(|photo| photo.relative_path)
            .collect()
    }

    #[test]
    fn recursively_finds_nested_supported_photos() {
        let root = TestDir::new();
        root.write("2024/trip/day-1/photo.JPG", b"not a real image");
        root.write("2024/trip/notes.txt", b"ignore");
        let db = TimelineDb::open_in_memory().expect("db");
        let analyzer = CountingAnalyzer::default();

        let report =
            scan(root.path(), &db, chrono_tz::Asia::Shanghai, &analyzer).expect("successful scan");

        assert_eq!(report.found, 1);
        assert_eq!(active_paths(&db), ["2024/trip/day-1/photo.JPG"]);
    }

    #[test]
    fn excludes_nas_metadata_and_recycle_directories() {
        let root = TestDir::new();
        root.write("album/keep.jpg", b"keep");
        root.write("album/@eaDir/hidden.jpg", b"hidden");
        root.write("#recycle/deleted.png", b"deleted");
        let db = TimelineDb::open_in_memory().expect("db");

        scan(
            root.path(),
            &db,
            chrono_tz::UTC,
            &CountingAnalyzer::default(),
        )
        .expect("successful scan");

        assert_eq!(active_paths(&db), ["album/keep.jpg"]);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlink_directories() {
        use std::os::unix::fs::symlink;

        let root = TestDir::new();
        let external = TestDir::new();
        external.write("outside.jpg", b"outside");
        symlink(external.path(), root.path().join("linked")).expect("create symlink");
        root.write("inside.jpg", b"inside");
        let db = TimelineDb::open_in_memory().expect("db");

        scan(
            root.path(),
            &db,
            chrono_tz::UTC,
            &CountingAnalyzer::default(),
        )
        .expect("successful scan");

        assert_eq!(active_paths(&db), ["inside.jpg"]);
    }

    #[test]
    fn stable_ids_include_the_full_relative_path() {
        let root = TestDir::new();
        root.write("one/duplicate.jpg", b"one");
        root.write("two/duplicate.jpg", b"two");
        let db = TimelineDb::open_in_memory().expect("db");

        scan(
            root.path(),
            &db,
            chrono_tz::UTC,
            &CountingAnalyzer::default(),
        )
        .expect("successful scan");

        let photos = db.list_active_photos().expect("active photos");
        assert_eq!(photos.len(), 2);
        assert_ne!(photos[0].id, photos[1].id);
        assert!(photos.iter().all(|photo| photo.id.len() == 40));
    }

    #[test]
    fn unchanged_second_scan_reuses_database_analysis() {
        let root = TestDir::new();
        root.write("nested/photo.jpg", b"unchanged");
        let db = TimelineDb::open_in_memory().expect("db");
        let analyzer = CountingAnalyzer::default();

        let first = scan(root.path(), &db, chrono_tz::UTC, &analyzer).expect("first scan");
        let second = scan(root.path(), &db, chrono_tz::UTC, &analyzer).expect("second scan");

        assert_eq!(first.analyzed, 1);
        assert_eq!(second.reused, 1);
        assert_eq!(second.analyzed, 0);
        assert_eq!(analyzer.calls(), 1);
    }

    #[test]
    fn changed_file_fingerprint_triggers_reanalysis() {
        let root = TestDir::new();
        let path = root.write("nested/photo.jpg", b"first");
        let db = TimelineDb::open_in_memory().expect("db");
        let analyzer = CountingAnalyzer::default();
        scan(root.path(), &db, chrono_tz::UTC, &analyzer).expect("first scan");

        fs::write(path, b"changed and longer").expect("change photo");
        let report = scan(root.path(), &db, chrono_tz::UTC, &analyzer).expect("second scan");

        assert_eq!(report.analyzed, 1);
        assert_eq!(analyzer.calls(), 2);
    }

    #[test]
    fn failed_root_scan_does_not_mark_existing_rows_missing() {
        let root = TestDir::new();
        root.write("photo.jpg", b"photo");
        let db = TimelineDb::open_in_memory().expect("db");
        let analyzer = CountingAnalyzer::default();
        scan(root.path(), &db, chrono_tz::UTC, &analyzer).expect("initial scan");

        fs::remove_dir_all(root.path()).expect("remove scan root");
        assert!(scan(root.path(), &db, chrono_tz::UTC, &analyzer).is_err());
        assert_eq!(active_paths(&db), ["photo.jpg"]);
    }

    #[test]
    fn incomplete_scan_does_not_mark_unseen_rows_missing() {
        let root = TestDir::new();
        root.write("a-visible.jpg", b"visible");
        root.write("z-later/hidden.jpg", b"hidden");
        let db = TimelineDb::open_in_memory().expect("db");
        let analyzer = CountingAnalyzer::default();
        scan(root.path(), &db, chrono_tz::UTC, &analyzer).expect("initial scan");

        fs::remove_file(root.path().join("z-later/hidden.jpg")).expect("remove old photo");
        root.write("m-fails.jpg", b"new photo");
        let failing = FailOnNameAnalyzer {
            inner: &analyzer,
            filename: "m-fails.jpg",
        };

        assert!(scan(root.path(), &db, chrono_tz::UTC, &failing).is_err());
        assert_eq!(active_paths(&db).len(), 3);
    }

    #[test]
    fn exif_data_is_converted_to_timeline_analysis() {
        let exif = ExifData {
            make: Some("Example".into()),
            model: Some("Camera".into()),
            lens: Some("Prime".into()),
            focal_length: None,
            aperture: None,
            shutter_speed: None,
            iso: None,
            date_taken: Some("2024:02:10 09:13:00".into()),
            timezone: Some("+08:00".into()),
            gps: None,
            dimensions: crate::exif::ImageDimensions {
                width: 10,
                height: 20,
            },
            file_size: 100,
            format: "JPEG".into(),
            flash: None,
            software: None,
            orientation: None,
            artist: None,
            color_space: None,
            image_description: None,
            user_comment: None,
            tags: vec![],
            tone: None,
        };

        let analysis = analysis_from_exif("id", "photo.jpg", UNIX_EPOCH, exif, chrono_tz::UTC)
            .expect("analysis");
        assert_eq!(
            analysis.taken_at.as_deref(),
            Some("2024-02-10T09:13:00+08:00")
        );
        assert_eq!(analysis.width, 10);
        assert_eq!(analysis.camera_make.as_deref(), Some("Example"));
    }
}
