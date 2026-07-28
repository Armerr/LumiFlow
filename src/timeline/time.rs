use crate::timeline::models::TimeSource;
use chrono::{DateTime, FixedOffset, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;
use std::sync::LazyLock;

static IMG_FILENAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^IMG_(\d{8})_(\d{6})\.[^.]+$").expect("IMG timestamp regex must compile")
});
static SCREENSHOT_FILENAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Screenshot_(\d{4}-\d{2}-\d{2})-(\d{2}-\d{2}-\d{2})\.[^.]+$")
        .expect("screenshot timestamp regex must compile")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeInput {
    pub exif_datetime: Option<String>,
    pub exif_offset: Option<String>,
    pub filename: String,
    pub mtime: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTime {
    pub timestamp: DateTime<FixedOffset>,
    pub source: TimeSource,
}

pub fn resolve_taken_at(input: &TimeInput, timezone: Tz) -> Option<ResolvedTime> {
    if let Some(timestamp) = input
        .exif_datetime
        .as_deref()
        .and_then(parse_exif_datetime)
        .and_then(|datetime| resolve_exif(datetime, input.exif_offset.as_deref(), timezone))
    {
        return Some(ResolvedTime {
            timestamp,
            source: TimeSource::Exif,
        });
    }

    if let Some(timestamp) =
        parse_filename_datetime(&input.filename).and_then(|datetime| localize(datetime, timezone))
    {
        return Some(ResolvedTime {
            timestamp,
            source: TimeSource::Filename,
        });
    }

    Some(ResolvedTime {
        timestamp: input.mtime.with_timezone(&timezone).fixed_offset(),
        source: TimeSource::Mtime,
    })
}

pub fn local_day(timestamp: DateTime<FixedOffset>, timezone: Tz) -> NaiveDate {
    timestamp.with_timezone(&timezone).date_naive()
}

fn parse_exif_datetime(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y:%m:%d %H:%M:%S").ok()
}

fn resolve_exif(
    datetime: NaiveDateTime,
    offset: Option<&str>,
    timezone: Tz,
) -> Option<DateTime<FixedOffset>> {
    match offset {
        Some(offset) => {
            parse_offset(offset).and_then(|offset| offset.from_local_datetime(&datetime).single())
        }
        None => localize(datetime, timezone),
    }
}

fn parse_offset(value: &str) -> Option<FixedOffset> {
    if value.len() != 6 || &value[3..4] != ":" {
        return None;
    }

    let sign = match &value[..1] {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let hours: i32 = value[1..3].parse().ok()?;
    let minutes: i32 = value[4..6].parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }

    FixedOffset::east_opt(sign * (hours * 3_600 + minutes * 60))
}

fn parse_filename_datetime(filename: &str) -> Option<NaiveDateTime> {
    if let Some(captures) = IMG_FILENAME.captures(filename) {
        let value = [captures.get(1)?.as_str(), captures.get(2)?.as_str()].concat();
        return NaiveDateTime::parse_from_str(&value, "%Y%m%d%H%M%S").ok();
    }

    let captures = SCREENSHOT_FILENAME.captures(filename)?;
    let value = [captures.get(1)?.as_str(), "-", captures.get(2)?.as_str()].concat();
    NaiveDateTime::parse_from_str(&value, "%Y-%m-%d-%H-%M-%S").ok()
}

fn localize(datetime: NaiveDateTime, timezone: Tz) -> Option<DateTime<FixedOffset>> {
    match timezone.from_local_datetime(&datetime) {
        LocalResult::Single(timestamp) => Some(timestamp.fixed_offset()),
        LocalResult::Ambiguous(first, _) => Some(first.fixed_offset()),
        LocalResult::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::models::TimeSource;
    use chrono::{DateTime, NaiveDate, Utc};

    fn utc(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid test timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn exif_timestamp_and_offset_beat_filename_and_mtime() {
        let input = TimeInput {
            exif_datetime: Some("2024:02:10 09:13:00".into()),
            exif_offset: Some("+09:00".into()),
            filename: "IMG_20240101_120000.jpg".into(),
            mtime: utc("2023-12-01T00:00:00Z"),
        };

        let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();

        assert_eq!(resolved.source, TimeSource::Exif);
        assert_eq!(resolved.timestamp.to_rfc3339(), "2024-02-10T09:13:00+09:00");
    }

    #[test]
    fn exif_without_offset_uses_configured_timezone() {
        let input = TimeInput {
            exif_datetime: Some("2024:02:10 09:13:00".into()),
            exif_offset: None,
            filename: "IMG_20240101_120000.jpg".into(),
            mtime: utc("2023-12-01T00:00:00Z"),
        };

        let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();

        assert_eq!(resolved.source, TimeSource::Exif);
        assert_eq!(resolved.timestamp.to_rfc3339(), "2024-02-10T09:13:00+08:00");
    }

    #[test]
    fn img_filename_timestamp_beats_mtime() {
        let input = TimeInput {
            exif_datetime: None,
            exif_offset: None,
            filename: "IMG_20240102_153012.jpg".into(),
            mtime: utc("2023-12-01T00:00:00Z"),
        };

        let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();

        assert_eq!(resolved.source, TimeSource::Filename);
        assert_eq!(resolved.timestamp.to_rfc3339(), "2024-01-02T15:30:12+08:00");
    }

    #[test]
    fn screenshot_filename_timestamp_beats_mtime() {
        let input = TimeInput {
            exif_datetime: None,
            exif_offset: None,
            filename: "Screenshot_2024-01-03-16-31-13.png".into(),
            mtime: utc("2023-12-01T00:00:00Z"),
        };

        let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();

        assert_eq!(resolved.source, TimeSource::Filename);
        assert_eq!(resolved.timestamp.to_rfc3339(), "2024-01-03T16:31:13+08:00");
    }

    #[test]
    fn mtime_is_the_final_fallback() {
        let input = TimeInput {
            exif_datetime: None,
            exif_offset: None,
            filename: "vacation.jpg".into(),
            mtime: utc("2024-02-10T17:30:00Z"),
        };

        let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();

        assert_eq!(resolved.source, TimeSource::Mtime);
        assert_eq!(resolved.timestamp.to_rfc3339(), "2024-02-11T01:30:00+08:00");
    }

    #[test]
    fn invalid_exif_date_falls_back_to_filename() {
        let input = TimeInput {
            exif_datetime: Some("2024:02:30 09:13:00".into()),
            exif_offset: Some("+08:00".into()),
            filename: "IMG_20240102_153012.jpg".into(),
            mtime: utc("2023-12-01T00:00:00Z"),
        };

        let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();

        assert_eq!(resolved.source, TimeSource::Filename);
        assert_eq!(resolved.timestamp.to_rfc3339(), "2024-01-02T15:30:12+08:00");
    }

    #[test]
    fn filename_patterns_are_anchored_and_reject_invalid_dates() {
        for filename in [
            "prefix_IMG_20240102_153012.jpg",
            "IMG_20240230_153012.jpg",
            "Screenshot_2024-13-03-16-31-13.png",
        ] {
            let input = TimeInput {
                exif_datetime: None,
                exif_offset: None,
                filename: filename.into(),
                mtime: utc("2023-12-01T00:00:00Z"),
            };

            let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();

            assert_eq!(resolved.source, TimeSource::Mtime, "{filename}");
        }
    }

    #[test]
    fn local_day_converts_the_instant_to_the_configured_timezone() {
        let timestamp = DateTime::parse_from_rfc3339("2024-02-10T17:30:00Z").unwrap();

        assert_eq!(
            local_day(timestamp, chrono_tz::Asia::Shanghai),
            NaiveDate::from_ymd_opt(2024, 2, 11).unwrap()
        );
    }
}
