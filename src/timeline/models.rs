use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeSource {
    Exif,
    Filename,
    Mtime,
    Unknown,
}

impl TimeSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Exif => "exif",
            Self::Filename => "filename",
            Self::Mtime => "mtime",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "exif" => Ok(Self::Exif),
            "filename" => Ok(Self::Filename),
            "mtime" => Ok(Self::Mtime),
            "unknown" => Ok(Self::Unknown),
            _ => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("invalid time source `{value}`").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoCandidate {
    pub id: String,
    pub relative_path: String,
    pub filename: String,
    pub extension: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub fingerprint: String,
    pub scan_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoAnalysis {
    pub id: String,
    pub taken_at: Option<String>,
    pub time_source: TimeSource,
    pub timezone: Option<String>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
    pub width: u32,
    pub height: u32,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub exif_json: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelinePhoto {
    pub id: String,
    pub relative_path: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub format: String,
    pub taken_at: Option<String>,
    pub time_source: TimeSource,
    pub fingerprint: String,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineAlbum {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub date_start: Option<NaiveDate>,
    pub date_end: Option<NaiveDate>,
    pub place: Option<String>,
    pub holiday: Option<String>,
    pub photo_count: usize,
    pub cover_photo_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineAlbumDetail {
    #[serde(flatten)]
    pub album: TimelineAlbum,
    pub photos: Vec<TimelinePhoto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyAlbumBuild {
    pub album: TimelineAlbum,
    pub photo_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisDecision {
    Analyze,
    Reuse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionTags {
    pub photo_id: String,
    pub model: String,
    pub input_fingerprint: String,
    pub labels: Vec<String>,
    pub scores: Vec<f32>,
    pub analyzed_at: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumAiDescription {
    pub album_id: String,
    pub input_fingerprint: String,
    pub model: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub confidence: f64,
    pub generated_at: String,
    pub error: Option<String>,
}
