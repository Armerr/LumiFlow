pub mod albums;
pub mod contact_sheet;
pub mod db;
pub mod holidays;
pub mod models;
pub mod places;
pub mod scan;
pub mod time;
pub mod vision;

use crate::config::Config;
use anyhow::{Context, Result};
use chrono_tz::Tz;
use db::TimelineDb;
use places::CachedPlaceResolver;
use scan::ScanReport;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RescanReport {
    pub scan: ScanReport,
    pub albums_count: usize,
}

/// SQLite-backed orchestration for timeline mode.
pub struct TimelineService {
    config: Config,
    db: TimelineDb,
    timezone: Tz,
    scan_lock: Mutex<()>,
}

impl TimelineService {
    /// Open and migrate the timeline database, then fully index the photo root.
    pub async fn open(config: Config) -> Result<Arc<Self>> {
        let timezone = parse_timezone(&config.timeline_timezone)?;
        let db = TimelineDb::open(config.data_path.join("lumiflow.sqlite"))?;
        let service = Arc::new(Self {
            config,
            db,
            timezone,
            scan_lock: Mutex::new(()),
        });
        service.rescan().await?;
        Ok(service)
    }

    #[cfg(test)]
    pub(crate) fn from_db_for_test(config: Config, db: TimelineDb) -> Result<Self> {
        let timezone = parse_timezone(&config.timeline_timezone)?;
        Ok(Self {
            config,
            db,
            timezone,
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
        let timezone = self.timezone;
        tokio::task::spawn_blocking(move || rescan_blocking(&config, &db, timezone))
            .await
            .context("timeline rescan task failed")?
    }
}

fn rescan_blocking(config: &Config, db: &TimelineDb, timezone: Tz) -> Result<RescanReport> {
    let scan = scan::scan(&config.photos_path, db, timezone, &scan::ExifAnalyzer)?;
    let places = CachedPlaceResolver::new(db.clone());
    let albums = albums::rebuild_daily_albums(db, timezone, &places)?;
    Ok(RescanReport {
        scan,
        albums_count: albums.len(),
    })
}

fn parse_timezone(value: &str) -> Result<Tz> {
    value
        .parse()
        .with_context(|| format!("invalid timeline timezone `{value}`"))
}
