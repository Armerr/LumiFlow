use crate::timeline::db::TimelineDb;
use crate::timeline::holidays::holiday_for;
use crate::timeline::models::{DailyAlbumBuild, TimelineAlbum, TimelinePhoto};
use crate::timeline::places::PlaceResolver;
use anyhow::Result;
use chrono::{DateTime, FixedOffset, Utc};
use chrono_tz::Tz;
use sha1::{Digest, Sha1};
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
        (String, chrono::NaiveDate),
        Vec<(&TimelinePhoto, DateTime<FixedOffset>)>,
    > = BTreeMap::new();
    for photo in photos {
        let timestamp = photo
            .taken_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .unwrap_or_else(|| DateTime::<Utc>::from_timestamp_nanos(photo.mtime_ns).fixed_offset());
        let date = timestamp.with_timezone(&timezone).date_naive();
        let folder = first_level_folder(&photo.relative_path).to_owned();
        days.entry((folder, date))
            .or_default()
            .push((photo, timestamp));
    }

    let mut builds = Vec::with_capacity(days.len());
    for ((folder, date), mut members) in days {
        members.sort_by(|(left_photo, left_time), (right_photo, right_time)| {
            compare_members(left_photo, *left_time, right_photo, *right_time)
        });
        let member_photos = members
            .iter()
            .map(|(photo, _)| (*photo).clone())
            .collect::<Vec<_>>();
        let resolved_places = places.resolve_album_places(&member_photos)?;
        let photo_ids = members
            .iter()
            .map(|(photo, _)| photo.id.clone())
            .collect::<Vec<_>>();
        let cover_photo_id = photo_ids.get(photo_ids.len() / 2).cloned();

        let holiday = holiday_for(date);
        let name = format_album_name(date, &resolved_places);
        let photo_count = photo_ids.len();
        builds.push(DailyAlbumBuild {
            album: TimelineAlbum {
                id: dated_album_id(&folder, date),
                name,
                description: None,
                date_start: Some(date),
                date_end: Some(date),
                place: place_summary(&resolved_places),
                holiday: holiday.map(str::to_owned),
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
            .then_with(|| left.album.name.cmp(&right.album.name))
            .then_with(|| left.album.id.cmp(&right.album.id))
    });
    Ok(builds)
}

pub fn format_album_name(date: chrono::NaiveDate, places: &[String]) -> String {
    let mut name = date.format("%y%m%d").to_string();
    if let Some(place) = place_summary(places) {
        name.push_str(" · ");
        name.push_str(&place);
    }
    name
}

fn first_level_folder(relative_path: &str) -> &str {
    relative_path
        .split_once('/')
        .map_or("", |(folder, _)| folder)
}

fn dated_album_id(folder: &str, date: chrono::NaiveDate) -> String {
    if folder.is_empty() {
        format!("auto-day:{date}")
    } else {
        format!("auto-day:{date}:{}", folder_fingerprint(folder))
    }
}

fn folder_fingerprint(folder: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha1::digest(folder.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn compare_members(
    left_photo: &TimelinePhoto,
    left_time: DateTime<FixedOffset>,
    right_photo: &TimelinePhoto,
    right_time: DateTime<FixedOffset>,
) -> Ordering {
    left_time
        .cmp(&right_time)
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
            mtime_ns: 1_704_067_200_000_000_000,
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
    fn keeps_first_level_folders_separate_before_grouping_by_day() {
        let photos = vec![
            photo("trip", "Trips/Kyoto/a.jpg", Some("2024-03-12T09:00:00Z")),
            photo("family", "Family/b.jpg", Some("2024-03-12T10:00:00Z")),
        ];

        let albums =
            build_daily_albums(&photos, chrono_tz::UTC, &FixedPlaces(vec![])).expect("album build");

        assert_eq!(albums.len(), 2);
        assert_ne!(albums[0].album.id, albums[1].album.id);
        assert_eq!(albums[0].photo_ids, ["family"]);
        assert_eq!(albums[1].photo_ids, ["trip"]);
        assert_eq!(albums[0].album.name, "240312");
        assert_eq!(albums[1].album.name, "240312");
    }

    #[test]
    fn keeps_nested_photos_together_within_the_same_first_level_folder() {
        let photos = vec![
            photo("kyoto", "Trips/Kyoto/a.jpg", Some("2024-03-12T09:00:00Z")),
            photo("tokyo", "Trips/Tokyo/b.jpg", Some("2024-03-12T10:00:00Z")),
        ];

        let albums =
            build_daily_albums(&photos, chrono_tz::UTC, &FixedPlaces(vec![])).expect("album build");

        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].album.name, "240312");
        assert_eq!(albums[0].photo_ids, ["kyoto", "tokyo"]);
    }

    #[test]
    fn falls_back_to_file_mtime_for_missing_or_invalid_dates_in_each_folder() {
        let photos = vec![
            photo("trip", "Trips/missing.jpg", None),
            photo("family", "Family/invalid.jpg", Some("not-a-time")),
        ];

        let albums =
            build_daily_albums(&photos, chrono_tz::UTC, &FixedPlaces(vec![])).expect("album build");

        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].album.name, "240101");
        assert_eq!(albums[1].album.name, "240101");
        assert!(albums
            .iter()
            .all(|build| build.album.id.starts_with("auto-day:2024-01-01:")));
    }

    #[test]
    fn groups_root_missing_and_invalid_times_by_file_mtime() {
        let photos = vec![
            photo("missing", "missing.jpg", None),
            photo("invalid", "invalid.jpg", Some("not-a-time")),
        ];

        let albums =
            build_daily_albums(&photos, chrono_tz::UTC, &FixedPlaces(vec![])).expect("album build");

        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].album.id, "auto-day:2024-01-01");
        assert_eq!(albums[0].album.name, "240101");
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
    fn builds_timeline_title_from_time_and_place_not_folder_name() {
        let photos = vec![photo(
            "spring",
            "Camera Uploads/spring.jpg",
            Some("2024-02-10T09:00:00+08:00"),
        )];

        let albums = build_daily_albums(
            &photos,
            chrono_tz::Asia::Shanghai,
            &FixedPlaces(vec!["上海".into()]),
        )
        .expect("album build");

        assert_eq!(albums[0].album.name, "240210 · 上海");
        assert!(albums[0].album.id.starts_with("auto-day:2024-02-10:"));
    }

    #[test]
    fn formats_date_and_places_without_holiday_suffix() {
        let day = NaiveDate::from_ymd_opt(2024, 2, 10).unwrap();
        assert_eq!(
            format_album_name(day, &["上海".into()]),
            "240210 · 上海"
        );
        assert_eq!(format_album_name(day, &[]), "240210");
    }

    #[test]
    fn name_uses_at_most_two_places_and_suffixes_the_remainder() {
        let day = NaiveDate::from_ymd_opt(2024, 3, 12).unwrap();
        assert_eq!(
            format_album_name(
                day,
                &["上海".into(), "苏州".into(), "杭州".into(), "南京".into()],
            ),
            "240312 · 上海 · 苏州 +2"
        );
    }

    #[test]
    fn build_keeps_place_and_holiday_metadata_without_folder_title() {
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

        assert_eq!(albums[0].album.name, "240210 · 上海");
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
            "240210 · 上海"
        );
    }
}
