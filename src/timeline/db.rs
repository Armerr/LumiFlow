use crate::timeline::models::{
    AlbumAiDescription, AnalysisDecision, DailyAlbumBuild, PhotoAnalysis, PhotoCandidate,
    TimeSource, TimelineAlbum, TimelineAlbumDetail, TimelinePhoto, VisionTags,
};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const MIGRATION_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS photos (
    id TEXT PRIMARY KEY,
    relative_path TEXT NOT NULL UNIQUE,
    filename TEXT NOT NULL,
    extension TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    mtime_ns INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'missing', 'unsupported', 'error')),
    last_scan_id TEXT NOT NULL,
    taken_at TEXT,
    time_source TEXT NOT NULL DEFAULT 'unknown'
        CHECK (time_source IN ('exif', 'filename', 'mtime', 'unknown')),
    timezone TEXT,
    gps_lat REAL,
    gps_lon REAL,
    width INTEGER,
    height INTEGER,
    camera_make TEXT,
    camera_model TEXT,
    lens TEXT,
    exif_json TEXT,
    exif_analyzed_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS albums (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL CHECK (type IN ('folder', 'auto_day', 'unknown_date')),
    date_start TEXT,
    date_end TEXT,
    display_name TEXT NOT NULL,
    place_name TEXT,
    holiday_name TEXT,
    photo_count INTEGER NOT NULL CHECK (photo_count >= 0),
    cover_photo_id TEXT REFERENCES photos(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS album_photos (
    album_id TEXT NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
    PRIMARY KEY (album_id, photo_id),
    UNIQUE (album_id, sort_order)
);

CREATE TABLE IF NOT EXISTS places (
    geo_bucket TEXT PRIMARY KEY,
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    country TEXT,
    region TEXT,
    city TEXT,
    district TEXT,
    provider TEXT NOT NULL,
    resolved_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS calendar_events (
    date TEXT NOT NULL,
    region TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (date, region, name)
);

CREATE TABLE IF NOT EXISTS photo_vision_tags (
    photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    input_fingerprint TEXT NOT NULL,
    labels_json TEXT NOT NULL,
    scores_json TEXT NOT NULL,
    analyzed_at TEXT NOT NULL,
    error TEXT,
    PRIMARY KEY (photo_id, model)
);

CREATE TABLE IF NOT EXISTS album_ai_descriptions (
    album_id TEXT PRIMARY KEY REFERENCES albums(id) ON DELETE CASCADE,
    input_fingerprint TEXT NOT NULL,
    model TEXT NOT NULL,
    description TEXT NOT NULL,
    keywords_json TEXT NOT NULL,
    confidence REAL NOT NULL,
    generated_at TEXT NOT NULL,
    error TEXT
);

CREATE INDEX IF NOT EXISTS idx_photos_active_taken_at
    ON photos(status, taken_at, relative_path);
CREATE INDEX IF NOT EXISTS idx_photos_last_scan_id
    ON photos(last_scan_id, status);
CREATE INDEX IF NOT EXISTS idx_photos_fingerprint
    ON photos(fingerprint);
CREATE INDEX IF NOT EXISTS idx_albums_sort
    ON albums(date_start DESC, id ASC);
CREATE INDEX IF NOT EXISTS idx_album_photos_membership
    ON album_photos(album_id, sort_order, photo_id);
CREATE INDEX IF NOT EXISTS idx_album_photos_photo
    ON album_photos(photo_id, album_id);
CREATE INDEX IF NOT EXISTS idx_vision_fingerprint
    ON photo_vision_tags(photo_id, model, input_fingerprint);
CREATE INDEX IF NOT EXISTS idx_ai_fingerprint
    ON album_ai_descriptions(album_id, input_fingerprint, model);
"#;

#[derive(Clone)]
pub struct TimelineDb {
    storage: DbStorage,
}

#[derive(Clone)]
enum DbStorage {
    File(PathBuf),
    Memory(Arc<Mutex<Connection>>),
}

impl TimelineDb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create timeline database directory {parent:?}")
            })?;
        }

        let db = Self {
            storage: DbStorage::File(path),
        };
        db.with_connection(|connection| migrate(connection))?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().context("failed to open in-memory SQLite")?;
        configure_connection(&connection, false)?;
        migrate(&connection)?;
        Ok(Self {
            storage: DbStorage::Memory(Arc::new(Mutex::new(connection))),
        })
    }

    fn with_connection<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        match &self.storage {
            DbStorage::File(path) => {
                let connection = Connection::open(path)
                    .with_context(|| format!("failed to open timeline database {path:?}"))?;
                configure_connection(&connection, true)?;
                operation(&connection)
            }
            DbStorage::Memory(connection) => {
                let connection = lock_connection(connection)?;
                operation(&connection)
            }
        }
    }

    fn with_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        match &self.storage {
            DbStorage::File(path) => {
                let mut connection = Connection::open(path)
                    .with_context(|| format!("failed to open timeline database {path:?}"))?;
                configure_connection(&connection, true)?;
                let transaction = connection.transaction()?;
                let result = operation(&transaction)?;
                transaction.commit()?;
                Ok(result)
            }
            DbStorage::Memory(connection) => {
                let mut connection = lock_connection(connection)?;
                let transaction = connection.transaction()?;
                let result = operation(&transaction)?;
                transaction.commit()?;
                Ok(result)
            }
        }
    }

    pub fn upsert_candidate(&self, candidate: &PhotoCandidate) -> Result<AnalysisDecision> {
        let size_bytes =
            i64::try_from(candidate.size_bytes).context("photo size exceeds SQLite")?;
        self.with_transaction(|transaction| {
            let existing_fingerprint = transaction
                .query_row(
                    "SELECT fingerprint FROM photos WHERE id = ?1",
                    [&candidate.id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let decision = match existing_fingerprint.as_deref() {
                Some(fingerprint) if fingerprint == candidate.fingerprint => {
                    AnalysisDecision::Reuse
                }
                _ => AnalysisDecision::Analyze,
            };

            transaction.execute(
                "INSERT INTO photos (
                    id, relative_path, filename, extension, size_bytes, mtime_ns,
                    fingerprint, status, last_scan_id, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, CURRENT_TIMESTAMP)
                 ON CONFLICT(id) DO UPDATE SET
                    relative_path = excluded.relative_path,
                    filename = excluded.filename,
                    extension = excluded.extension,
                    size_bytes = excluded.size_bytes,
                    mtime_ns = excluded.mtime_ns,
                    fingerprint = excluded.fingerprint,
                    status = 'active',
                    last_scan_id = excluded.last_scan_id,
                    updated_at = CURRENT_TIMESTAMP",
                params![
                    candidate.id,
                    candidate.relative_path,
                    candidate.filename,
                    candidate.extension,
                    size_bytes,
                    candidate.mtime_ns,
                    candidate.fingerprint,
                    candidate.scan_id,
                ],
            )?;
            Ok(decision)
        })
    }

    pub fn save_analysis(&self, analysis: &PhotoAnalysis) -> Result<()> {
        let exif_json = serde_json::to_string(&analysis.exif_json)?;
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE photos SET
                    taken_at = ?2,
                    time_source = ?3,
                    timezone = ?4,
                    gps_lat = ?5,
                    gps_lon = ?6,
                    width = ?7,
                    height = ?8,
                    camera_make = ?9,
                    camera_model = ?10,
                    lens = ?11,
                    exif_json = ?12,
                    exif_analyzed_at = CURRENT_TIMESTAMP,
                    status = 'active',
                    updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1",
                params![
                    analysis.id,
                    analysis.taken_at,
                    analysis.time_source.as_str(),
                    analysis.timezone,
                    analysis.gps_lat,
                    analysis.gps_lon,
                    i64::from(analysis.width),
                    i64::from(analysis.height),
                    analysis.camera_make,
                    analysis.camera_model,
                    analysis.lens,
                    exif_json,
                ],
            )?;
            anyhow::ensure!(changed == 1, "photo `{}` does not exist", analysis.id);
            Ok(())
        })
    }

    pub fn mark_missing_except(&self, scan_id: &str) -> Result<usize> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE photos
                 SET status = 'missing', updated_at = CURRENT_TIMESTAMP
                 WHERE status = 'active' AND last_scan_id <> ?1",
                [scan_id],
            )?)
        })
    }

    pub fn replace_daily_albums(&self, albums: &[DailyAlbumBuild]) -> Result<()> {
        self.with_transaction(|transaction| {
            transaction.execute(
                "DELETE FROM albums WHERE type IN ('auto_day', 'unknown_date')",
                [],
            )?;

            for build in albums {
                let album_type = if build.album.id == "unknown-date" {
                    "unknown_date"
                } else {
                    "auto_day"
                };
                let photo_count = i64::try_from(build.photo_ids.len())
                    .context("album photo count exceeds SQLite")?;
                anyhow::ensure!(
                    build.album.photo_count == build.photo_ids.len(),
                    "album `{}` declares {} photos but has {} memberships",
                    build.album.id,
                    build.album.photo_count,
                    build.photo_ids.len()
                );

                transaction.execute(
                    "INSERT INTO albums (
                        id, type, date_start, date_end, display_name, place_name,
                        holiday_name, photo_count, cover_photo_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        build.album.id,
                        album_type,
                        build.album.date_start,
                        build.album.date_end,
                        build.album.name,
                        build.album.place,
                        build.album.holiday,
                        photo_count,
                        build.album.cover_photo_id,
                    ],
                )?;

                for (sort_order, photo_id) in build.photo_ids.iter().enumerate() {
                    transaction.execute(
                        "INSERT INTO album_photos (album_id, photo_id, sort_order)
                         VALUES (?1, ?2, ?3)",
                        params![
                            build.album.id,
                            photo_id,
                            i64::try_from(sort_order).context("album order exceeds SQLite")?,
                        ],
                    )?;
                }
            }
            Ok(())
        })
    }

    pub fn list_active_photos(&self) -> Result<Vec<TimelinePhoto>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, relative_path, filename, width, height, size_bytes,
                        extension, taken_at, time_source, fingerprint
                 FROM photos
                 WHERE status = 'active'
                 ORDER BY taken_at IS NULL, taken_at, relative_path, id",
            )?;
            let photos = statement
                .query_map([], map_photo)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(photos)
        })
    }

    pub fn list_albums(&self) -> Result<Vec<TimelineAlbum>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT a.id, a.display_name, d.description, a.date_start, a.date_end,
                        a.place_name, a.holiday_name, a.photo_count, a.cover_photo_id
                 FROM albums a
                 LEFT JOIN album_ai_descriptions d ON d.album_id = a.id
                 ORDER BY a.date_start IS NULL, a.date_start DESC, a.id ASC",
            )?;
            let albums = statement
                .query_map([], map_album)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(albums)
        })
    }

    pub fn get_album(&self, id: &str) -> Result<Option<TimelineAlbumDetail>> {
        self.with_connection(|connection| {
            let album = connection
                .query_row(
                    "SELECT a.id, a.display_name, d.description, a.date_start, a.date_end,
                            a.place_name, a.holiday_name, a.photo_count, a.cover_photo_id
                     FROM albums a
                     LEFT JOIN album_ai_descriptions d ON d.album_id = a.id
                     WHERE a.id = ?1",
                    [id],
                    map_album,
                )
                .optional()?;
            let Some(album) = album else {
                return Ok(None);
            };

            let mut statement = connection.prepare(
                "SELECT p.id, p.relative_path, p.filename, p.width, p.height, p.size_bytes,
                        p.extension, p.taken_at, p.time_source, p.fingerprint
                 FROM album_photos ap
                 JOIN photos p ON p.id = ap.photo_id
                 WHERE ap.album_id = ?1 AND p.status = 'active'
                 ORDER BY ap.sort_order, p.id",
            )?;
            let photos = statement
                .query_map([id], map_photo)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Some(TimelineAlbumDetail { album, photos }))
        })
    }

    pub fn get_photo(&self, id: &str) -> Result<Option<TimelinePhoto>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT id, relative_path, filename, width, height, size_bytes,
                            extension, taken_at, time_source, fingerprint
                     FROM photos WHERE id = ?1 AND status = 'active'",
                    [id],
                    map_photo,
                )
                .optional()?)
        })
    }

    pub fn save_vision_tags(&self, tags: &VisionTags) -> Result<()> {
        let labels = serde_json::to_string(&tags.labels)?;
        let scores = serde_json::to_string(&tags.scores)?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO photo_vision_tags (
                    photo_id, model, input_fingerprint, labels_json, scores_json, analyzed_at, error
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(photo_id, model) DO UPDATE SET
                    input_fingerprint = excluded.input_fingerprint,
                    labels_json = excluded.labels_json,
                    scores_json = excluded.scores_json,
                    analyzed_at = excluded.analyzed_at,
                    error = excluded.error",
                params![
                    tags.photo_id,
                    tags.model,
                    tags.input_fingerprint,
                    labels,
                    scores,
                    tags.analyzed_at,
                    tags.error,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_vision_tags(&self, photo_id: &str, model: &str) -> Result<Option<VisionTags>> {
        self.with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT input_fingerprint, labels_json, scores_json, analyzed_at, error
                     FROM photo_vision_tags WHERE photo_id = ?1 AND model = ?2",
                    params![photo_id, model],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    },
                )
                .optional()?;
            raw.map(|(input_fingerprint, labels, scores, analyzed_at, error)| {
                Ok(VisionTags {
                    photo_id: photo_id.into(),
                    model: model.into(),
                    input_fingerprint,
                    labels: serde_json::from_str(&labels)?,
                    scores: serde_json::from_str(&scores)?,
                    analyzed_at,
                    error,
                })
            })
            .transpose()
        })
    }

    pub fn save_ai_description(&self, description: &AlbumAiDescription) -> Result<()> {
        let keywords = serde_json::to_string(&description.keywords)?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO album_ai_descriptions (
                    album_id, input_fingerprint, model, description, keywords_json,
                    confidence, generated_at, error
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(album_id) DO UPDATE SET
                    input_fingerprint = excluded.input_fingerprint,
                    model = excluded.model,
                    description = excluded.description,
                    keywords_json = excluded.keywords_json,
                    confidence = excluded.confidence,
                    generated_at = excluded.generated_at,
                    error = excluded.error",
                params![
                    description.album_id,
                    description.input_fingerprint,
                    description.model,
                    description.description,
                    keywords,
                    description.confidence,
                    description.generated_at,
                    description.error,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_ai_description(&self, album_id: &str) -> Result<Option<AlbumAiDescription>> {
        self.with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT input_fingerprint, model, description, keywords_json,
                            confidence, generated_at, error
                     FROM album_ai_descriptions WHERE album_id = ?1",
                    [album_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, Option<String>>(6)?,
                        ))
                    },
                )
                .optional()?;
            raw.map(
                |(
                    input_fingerprint,
                    model,
                    description,
                    keywords,
                    confidence,
                    generated_at,
                    error,
                )| {
                    Ok(AlbumAiDescription {
                        album_id: album_id.into(),
                        input_fingerprint,
                        model,
                        description,
                        keywords: serde_json::from_str(&keywords)?,
                        confidence,
                        generated_at,
                        error,
                    })
                },
            )
            .transpose()
        })
    }

    #[cfg(test)]
    fn has_table(&self, table: &str) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
        })
    }

    #[cfg(test)]
    fn last_scan_id(&self, photo_id: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT last_scan_id FROM photos WHERE id = ?1",
                    [photo_id],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }
}

fn configure_connection(connection: &Connection, file_backed: bool) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    if file_backed {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
    }
    Ok(())
}

fn migrate(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(MIGRATION_SQL)
        .context("failed to migrate timeline database")
}

fn lock_connection(connection: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>> {
    connection
        .lock()
        .map_err(|_| anyhow::anyhow!("in-memory timeline database lock was poisoned"))
}

fn map_photo(row: &Row<'_>) -> rusqlite::Result<TimelinePhoto> {
    let width: Option<i64> = row.get(3)?;
    let height: Option<i64> = row.get(4)?;
    let size_bytes: i64 = row.get(5)?;
    let time_source: String = row.get(8)?;
    Ok(TimelinePhoto {
        id: row.get(0)?,
        relative_path: row.get(1)?,
        name: row.get(2)?,
        width: unsigned_u32(width.unwrap_or_default(), 3)?,
        height: unsigned_u32(height.unwrap_or_default(), 4)?,
        size_bytes: unsigned_u64(size_bytes, 5)?,
        format: row.get::<_, String>(6)?.to_uppercase(),
        taken_at: row.get(7)?,
        time_source: TimeSource::from_db(&time_source)?,
        fingerprint: row.get(9)?,
    })
}

fn map_album(row: &Row<'_>) -> rusqlite::Result<TimelineAlbum> {
    let photo_count: i64 = row.get(7)?;
    Ok(TimelineAlbum {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        date_start: row.get(3)?,
        date_end: row.get(4)?,
        place: row.get(5)?,
        holiday: row.get(6)?,
        photo_count: usize::try_from(photo_count).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        cover_photo_id: row.get(8)?,
    })
}

fn unsigned_u32(value: i64, column: usize) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn unsigned_u64(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::models::{
        AnalysisDecision, DailyAlbumBuild, PhotoAnalysis, PhotoCandidate, TimeSource, TimelineAlbum,
    };
    use chrono::NaiveDate;
    use serde_json::json;

    fn candidate(id: &str, fingerprint: &str, scan_id: &str) -> PhotoCandidate {
        PhotoCandidate {
            id: id.into(),
            relative_path: format!("nested/{id}.jpg"),
            filename: format!("{id}.jpg"),
            extension: "jpg".into(),
            size_bytes: 100,
            mtime_ns: 1_234,
            fingerprint: fingerprint.into(),
            scan_id: scan_id.into(),
        }
    }

    fn analyzed_candidate(
        db: &TimelineDb,
        id: &str,
        fingerprint: &str,
        scan_id: &str,
        taken_at: &str,
    ) {
        let photo = candidate(id, fingerprint, scan_id);
        assert_eq!(
            db.upsert_candidate(&photo).expect("candidate upsert"),
            AnalysisDecision::Analyze
        );
        db.save_analysis(&PhotoAnalysis {
            id: id.into(),
            taken_at: Some(taken_at.into()),
            time_source: TimeSource::Exif,
            timezone: Some("+08:00".into()),
            gps_lat: None,
            gps_lon: None,
            width: 4_032,
            height: 3_024,
            camera_make: Some("Example".into()),
            camera_model: Some("Camera".into()),
            lens: None,
            exif_json: json!({"iso": 100}),
        })
        .expect("save analysis");
    }

    fn album(id: &str, date: NaiveDate, photo_ids: &[&str]) -> DailyAlbumBuild {
        DailyAlbumBuild {
            album: TimelineAlbum {
                id: id.into(),
                name: format!("{date}"),
                description: None,
                date_start: Some(date),
                date_end: Some(date),
                place: None,
                holiday: None,
                photo_count: photo_ids.len(),
                cover_photo_id: photo_ids.first().map(|id| (*id).into()),
            },
            photo_ids: photo_ids.iter().map(|id| (*id).into()).collect(),
        }
    }

    #[test]
    fn migrations_create_all_timeline_tables() {
        let db = TimelineDb::open_in_memory().expect("db");

        for table in [
            "photos",
            "albums",
            "album_photos",
            "places",
            "calendar_events",
            "photo_vision_tags",
            "album_ai_descriptions",
        ] {
            assert!(
                db.has_table(table).expect("schema query"),
                "missing {table}"
            );
        }
    }

    #[test]
    fn candidate_upsert_analyzes_then_reuses_unchanged_fingerprint() {
        let db = TimelineDb::open_in_memory().expect("db");
        let first = candidate("photo-1", "fp-1", "scan-1");

        assert_eq!(
            db.upsert_candidate(&first).expect("first upsert"),
            AnalysisDecision::Analyze
        );

        let mut second = first.clone();
        second.scan_id = "scan-2".into();
        assert_eq!(
            db.upsert_candidate(&second).expect("second upsert"),
            AnalysisDecision::Reuse
        );
        assert_eq!(
            db.last_scan_id("photo-1").expect("scan lookup").as_deref(),
            Some("scan-2")
        );
    }

    #[test]
    fn changed_fingerprint_requires_analysis() {
        let db = TimelineDb::open_in_memory().expect("db");
        let first = candidate("photo-1", "fp-1", "scan-1");
        db.upsert_candidate(&first).expect("first upsert");

        let changed = candidate("photo-1", "fp-2", "scan-2");
        assert_eq!(
            db.upsert_candidate(&changed).expect("changed upsert"),
            AnalysisDecision::Analyze
        );
    }

    #[test]
    fn mark_missing_except_marks_only_photos_unseen_in_scan() {
        let db = TimelineDb::open_in_memory().expect("db");
        db.upsert_candidate(&candidate("seen", "fp-seen", "scan-2"))
            .expect("seen upsert");
        db.upsert_candidate(&candidate("unseen", "fp-unseen", "scan-1"))
            .expect("unseen upsert");

        assert_eq!(db.mark_missing_except("scan-2").expect("mark missing"), 1);
        assert!(db.get_photo("seen").expect("seen lookup").is_some());
        assert!(db.get_photo("unseen").expect("unseen lookup").is_none());
    }

    #[test]
    fn replacing_daily_albums_writes_memberships_and_queries_in_stable_order() {
        let db = TimelineDb::open_in_memory().expect("db");
        analyzed_candidate(
            &db,
            "early",
            "fp-early",
            "scan-1",
            "2024-02-10T09:00:00+08:00",
        );
        analyzed_candidate(
            &db,
            "late",
            "fp-late",
            "scan-1",
            "2024-02-10T18:00:00+08:00",
        );
        analyzed_candidate(
            &db,
            "new-day",
            "fp-new",
            "scan-1",
            "2024-02-11T10:00:00+08:00",
        );

        let older = album(
            "auto-day:2024-02-10",
            NaiveDate::from_ymd_opt(2024, 2, 10).unwrap(),
            &["late", "early"],
        );
        let newer = album(
            "auto-day:2024-02-11",
            NaiveDate::from_ymd_opt(2024, 2, 11).unwrap(),
            &["new-day"],
        );
        db.replace_daily_albums(&[older, newer])
            .expect("replace albums");

        let albums = db.list_albums().expect("list albums");
        assert_eq!(
            albums
                .iter()
                .map(|album| album.id.as_str())
                .collect::<Vec<_>>(),
            ["auto-day:2024-02-11", "auto-day:2024-02-10"]
        );

        let detail = db
            .get_album("auto-day:2024-02-10")
            .expect("get album")
            .expect("album exists");
        assert_eq!(
            detail
                .photos
                .iter()
                .map(|photo| photo.id.as_str())
                .collect::<Vec<_>>(),
            ["late", "early"]
        );

        let replacement = album(
            "auto-day:2024-02-11",
            NaiveDate::from_ymd_opt(2024, 2, 11).unwrap(),
            &["early", "new-day"],
        );
        db.replace_daily_albums(&[replacement])
            .expect("replace albums again");

        assert!(db
            .get_album("auto-day:2024-02-10")
            .expect("old album lookup")
            .is_none());
        let detail = db
            .get_album("auto-day:2024-02-11")
            .expect("new album lookup")
            .expect("new album exists");
        assert_eq!(
            detail
                .photos
                .iter()
                .map(|photo| photo.id.as_str())
                .collect::<Vec<_>>(),
            ["early", "new-day"]
        );
    }
    #[test]
    fn file_database_enables_foreign_keys_wal_and_busy_timeout() {
        let path = std::env::temp_dir().join(format!(
            "lumiflow-timeline-db-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let db = TimelineDb::open(&path).expect("file db");
        let pragmas = db
            .with_connection(|connection| {
                Ok((
                    connection.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?,
                    connection
                        .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?,
                    connection.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))?,
                ))
            })
            .expect("pragma query");
        assert_eq!(pragmas, (1, "wal".into(), 5_000));

        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    }

    #[test]
    fn list_active_photos_is_chronological_with_unknown_dates_last() {
        let db = TimelineDb::open_in_memory().expect("db");
        analyzed_candidate(
            &db,
            "later",
            "fp-later",
            "scan-1",
            "2024-02-11T10:00:00+08:00",
        );
        analyzed_candidate(
            &db,
            "earlier",
            "fp-earlier",
            "scan-1",
            "2024-02-10T09:00:00+08:00",
        );
        db.upsert_candidate(&candidate("unknown", "fp-unknown", "scan-1"))
            .expect("unknown candidate");

        let photos = db.list_active_photos().expect("active photos");
        assert_eq!(
            photos
                .iter()
                .map(|photo| photo.id.as_str())
                .collect::<Vec<_>>(),
            ["earlier", "later", "unknown"]
        );
    }

    #[test]
    fn vision_and_ai_cache_values_round_trip() {
        use crate::timeline::models::{AlbumAiDescription, VisionTags};

        let db = TimelineDb::open_in_memory().expect("db");
        analyzed_candidate(
            &db,
            "photo",
            "fp-photo",
            "scan-1",
            "2024-02-10T09:00:00+08:00",
        );
        db.replace_daily_albums(&[album(
            "auto-day:2024-02-10",
            NaiveDate::from_ymd_opt(2024, 2, 10).unwrap(),
            &["photo"],
        )])
        .expect("album build");

        let tags = VisionTags {
            photo_id: "photo".into(),
            model: "model-v1".into(),
            input_fingerprint: "vision-fp".into(),
            labels: vec!["family".into(), "meal".into()],
            scores: vec![0.9, 0.8],
            analyzed_at: "2024-02-10T12:00:00Z".into(),
            error: None,
        };
        db.save_vision_tags(&tags).expect("save tags");
        assert_eq!(
            db.get_vision_tags("photo", "model-v1").expect("get tags"),
            Some(tags)
        );

        let description = AlbumAiDescription {
            album_id: "auto-day:2024-02-10".into(),
            input_fingerprint: "ai-fp".into(),
            model: "vision-model".into(),
            description: "A family meal.".into(),
            keywords: vec!["family".into(), "meal".into()],
            confidence: 0.92,
            generated_at: "2024-02-10T12:30:00Z".into(),
            error: None,
        };
        db.save_ai_description(&description)
            .expect("save description");
        assert_eq!(
            db.get_ai_description("auto-day:2024-02-10")
                .expect("get description"),
            Some(description)
        );
        assert_eq!(
            db.list_albums().expect("albums")[0].description.as_deref(),
            Some("A family meal.")
        );
    }
}
