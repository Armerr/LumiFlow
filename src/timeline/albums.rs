use crate::timeline::db::TimelineDb;
use crate::timeline::holidays::holiday_for;
use crate::timeline::models::{DailyAlbumBuild, TimelineAlbum, TimelinePhoto};
use crate::timeline::places::PlaceResolver;
use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use chrono_tz::Tz;
use std::cmp::Ordering;
use std::collections::BTreeMap;

pub fn rebuild_daily_albums(
    db: &TimelineDb,
    timezone: Tz,
    places: &(impl PlaceResolver + ?Sized),
) -> Result<Vec<DailyAlbumBuild>> {
    let photos = db.list_active_photos()?;
    let builds = build_daily_albums(&photos, timezone, places)?;
    db.replace_daily_albums(&builds)?;
    Ok(builds)
}

pub fn build_daily_albums(
    photos: &[TimelinePhoto],
    timezone: Tz,
    places: &(impl PlaceResolver + ?Sized),
) -> Result<Vec<DailyAlbumBuild>> {
    let mut days: BTreeMap<
        Option<chrono::NaiveDate>,
        Vec<(&TimelinePhoto, Option<DateTime<FixedOffset>>)>,
    > = BTreeMap::new();
    for photo in photos {
        let timestamp = photo
            .taken_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok());
        let date = timestamp.map(|value| value.with_timezone(&timezone).date_naive());
        days.entry(date).or_default().push((photo, timestamp));
    }

    let mut builds = Vec::with_capacity(days.len());
    for (date, mut members) in days {
        members.sort_by(|(left_photo, left_time), (right_photo, right_time)| {
            compare_members(left_photo, *left_time, right_photo, *right_time)
        });
        let member_photos = members
            .iter()
            .map(|(photo, _)| (*photo).clone())
            .collect::<Vec<_>>();
        let resolved_places = if date.is_some() {
            places.resolve_album_places(&member_photos)?
        } else {
            Vec::new()
        };
        let photo_ids = members
            .iter()
            .map(|(photo, _)| photo.id.clone())
            .collect::<Vec<_>>();
        let cover_photo_id = photo_ids.get(photo_ids.len() / 2).cloned();

        let (id, name, holiday, place) = match date {
            Some(date) => {
                let holiday = holiday_for(date);
                (
                    format!("auto-day:{date}"),
                    format_album_name(date, &resolved_places, holiday),
                    holiday.map(str::to_owned),
                    place_summary(&resolved_places),
                )
            }
            None => ("unknown-date".into(), "Unknown Date".into(), None, None),
        };
        let photo_count = photo_ids.len();
        builds.push(DailyAlbumBuild {
            album: TimelineAlbum {
                id,
                name,
                description: None,
                date_start: date,
                date_end: date,
                place,
                holiday,
                photo_count,
                cover_photo_id,
            },
            photo_ids,
        });
    }
    builds.sort_by(|left, right| {
        left.album
            .date_start
            .is_none()
            .cmp(&right.album.date_start.is_none())
            .then_with(|| left.album.date_start.cmp(&right.album.date_start))
            .then_with(|| left.album.id.cmp(&right.album.id))
    });
    Ok(builds)
}

pub fn format_album_name(
    date: chrono::NaiveDate,
    places: &[String],
    holiday: Option<&str>,
) -> String {
    let mut name = date.to_string();
    if let Some(place) = place_summary(places) {
        name.push(' ');
        name.push_str(&place);
    }
    if let Some(holiday) = holiday {
        name.push_str(" · ");
        name.push_str(holiday);
    }
    name
}

fn compare_members(
    left_photo: &TimelinePhoto,
    left_time: Option<DateTime<FixedOffset>>,
    right_photo: &TimelinePhoto,
    right_time: Option<DateTime<FixedOffset>>,
) -> Ordering {
    left_time
        .is_none()
        .cmp(&right_time.is_none())
        .then_with(|| left_time.cmp(&right_time))
        .then_with(|| left_photo.relative_path.cmp(&right_photo.relative_path))
        .then_with(|| left_photo.id.cmp(&right_photo.id))
}

fn place_summary(places: &[String]) -> Option<String> {
    if places.is_empty() {
        return None;
    }
    let mut summary = places
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    if places.len() > 2 {
        summary.push_str(&format!(" +{}", places.len() - 2));
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::{build_daily_albums, format_album_name, rebuild_daily_albums};
    use crate::timeline::db::TimelineDb;
    use crate::timeline::models::{PhotoAnalysis, PhotoCandidate, TimeSource, TimelinePhoto};
    use crate::timeline::places::PlaceResolver;
    use anyhow::Result;
    use chrono::NaiveDate;
    use serde_json::json;

    struct FixedPlaces(Vec<String>);

    impl PlaceResolver for FixedPlaces {
        fn resolve_album_places(&self, _photos: &[TimelinePhoto]) -> Result<Vec<String>> {
            Ok(self.0.clone())
        }
    }

    fn photo(id: &str, path: &str, taken_at: Option<&str>) -> TimelinePhoto {
        TimelinePhoto {
            id: id.into(),
            relative_path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            width: 100,
            height: 100,
            size_bytes: 10,
            format: "JPEG".into(),
            taken_at: taken_at.map(str::to_owned),
            time_source: if taken_at.is_some() {
                TimeSource::Exif
            } else {
                TimeSource::Unknown
            },
            fingerprint: format!("fp-{id}"),
            gps_lat: None,
            gps_lon: None,
        }
    }

    #[test]
    fn cuts_albums_at_configured_local_midnight() {
        let photos = vec![
            photo("before", "b.jpg", Some("2024-02-10T15:59:00Z")),
            photo("after", "a.jpg", Some("2024-02-10T16:01:00Z")),
        ];

        let albums = build_daily_albums(&photos, chrono_tz::Asia::Shanghai, &FixedPlaces(vec![]))
            .expect("album build");

        assert_eq!(
            albums
                .iter()
                .map(|build| build.album.id.as_str())
                .collect::<Vec<_>>(),
            ["auto-day:2024-02-10", "auto-day:2024-02-11"]
        );
    }

    #[test]
    fn puts_missing_and_invalid_times_in_exactly_named_unknown_album() {
        let photos = vec![
            photo("missing", "z/missing.jpg", None),
            photo("invalid", "a/invalid.jpg", Some("not-a-time")),
        ];

        let albums =
            build_daily_albums(&photos, chrono_tz::UTC, &FixedPlaces(vec![])).expect("album build");

        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].album.id, "unknown-date");
        assert_eq!(albums[0].album.name, "Unknown Date");
        assert_eq!(albums[0].photo_ids, ["invalid", "missing"]);
        assert_eq!(albums[0].album.cover_photo_id.as_deref(), Some("missing"));
    }

    #[test]
    fn membership_sorting_and_median_cover_are_deterministic() {
        let photos = vec![
            photo("third", "z.jpg", Some("2024-03-12T12:00:00Z")),
            photo("tie-b", "b.jpg", Some("2024-03-12T10:00:00Z")),
            photo("first", "first.jpg", Some("2024-03-12T09:00:00Z")),
            photo("tie-a", "a.jpg", Some("2024-03-12T10:00:00Z")),
        ];

        let albums =
            build_daily_albums(&photos, chrono_tz::UTC, &FixedPlaces(vec![])).expect("album build");

        assert_eq!(albums[0].photo_ids, ["first", "tie-a", "tie-b", "third"]);
        assert_eq!(albums[0].album.cover_photo_id.as_deref(), Some("tie-b"));
    }

    #[test]
    fn formats_date_places_and_holiday() {
        let day = NaiveDate::from_ymd_opt(2024, 2, 10).unwrap();
        assert_eq!(
            format_album_name(day, &["上海".into()], Some("春节")),
            "2024-02-10 上海 · 春节"
        );
        assert_eq!(
            format_album_name(day, &[], Some("春节")),
            "2024-02-10 · 春节"
        );
        assert_eq!(
            format_album_name(day, &["上海".into()], None),
            "2024-02-10 上海"
        );
    }

    #[test]
    fn name_uses_at_most_two_places_and_suffixes_the_remainder() {
        let day = NaiveDate::from_ymd_opt(2024, 3, 12).unwrap();
        assert_eq!(
            format_album_name(
                day,
                &["上海".into(), "苏州".into(), "杭州".into(), "南京".into()],
                None,
            ),
            "2024-03-12 上海 · 苏州 +2"
        );
    }

    #[test]
    fn build_includes_deterministic_place_and_holiday_name() {
        let photos = vec![photo(
            "spring",
            "Shanghai/spring.jpg",
            Some("2024-02-10T09:00:00+08:00"),
        )];

        let albums = build_daily_albums(
            &photos,
            chrono_tz::Asia::Shanghai,
            &FixedPlaces(vec!["上海".into()]),
        )
        .expect("album build");

        assert_eq!(albums[0].album.name, "2024-02-10 上海 · 春节");
        assert_eq!(albums[0].album.place.as_deref(), Some("上海"));
        assert_eq!(albums[0].album.holiday.as_deref(), Some("春节"));
    }
    #[test]
    fn rebuild_loads_active_photos_and_replaces_database_albums() {
        let db = TimelineDb::open_in_memory().expect("db");
        db.upsert_candidate(&PhotoCandidate {
            id: "photo".into(),
            relative_path: "Shanghai/photo.jpg".into(),
            filename: "photo.jpg".into(),
            extension: "jpg".into(),
            size_bytes: 1,
            mtime_ns: 1,
            fingerprint: "fp".into(),
            scan_id: "scan".into(),
        })
        .expect("candidate");
        db.save_analysis(&PhotoAnalysis {
            id: "photo".into(),
            taken_at: Some("2024-02-10T09:00:00+08:00".into()),
            time_source: TimeSource::Exif,
            timezone: Some("+08:00".into()),
            gps_lat: None,
            gps_lon: None,
            width: 1,
            height: 1,
            camera_make: None,
            camera_model: None,
            lens: None,
            exif_json: json!({}),
        })
        .expect("analysis");

        let builds = rebuild_daily_albums(
            &db,
            chrono_tz::Asia::Shanghai,
            &FixedPlaces(vec!["上海".into()]),
        )
        .expect("rebuild");

        assert_eq!(builds.len(), 1);
        assert_eq!(
            db.list_albums().expect("albums")[0].name,
            "2024-02-10 上海 · 春节"
        );
    }
}
