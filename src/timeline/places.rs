use crate::timeline::db::TimelineDb;
use crate::timeline::models::TimelinePhoto;
use anyhow::Result;
use std::collections::HashMap;

const PATH_PLACES: &[(&str, &str)] = &[
    ("beijing", "北京"),
    ("北京", "北京"),
    ("shanghai", "上海"),
    ("上海", "上海"),
    ("suzhou", "苏州"),
    ("苏州", "苏州"),
    ("hangzhou", "杭州"),
    ("杭州", "杭州"),
    ("nanjing", "南京"),
    ("南京", "南京"),
    ("tokyo", "东京"),
    ("东京", "东京"),
    ("kyoto", "京都"),
    ("京都", "京都"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub geo_bucket: String,
    pub lat: f64,
    pub lon: f64,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub district: Option<String>,
    pub provider: String,
    pub resolved_at: String,
}

impl Place {
    pub fn display_name(&self) -> Option<String> {
        [&self.city, &self.district, &self.region, &self.country]
            .into_iter()
            .find_map(|value| {
                value
                    .as_ref()
                    .filter(|value| !value.trim().is_empty())
                    .cloned()
            })
    }
}

pub trait PlaceResolver {
    fn resolve_album_places(&self, photos: &[TimelinePhoto]) -> Result<Vec<String>>;

    fn resolve_album_place(&self, photos: &[TimelinePhoto]) -> Result<Option<String>> {
        Ok(self.resolve_album_places(photos)?.into_iter().next())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoPlaces;

impl PlaceResolver for NoPlaces {
    fn resolve_album_places(&self, _photos: &[TimelinePhoto]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
pub struct CachedPlaceResolver {
    db: TimelineDb,
}

impl CachedPlaceResolver {
    pub fn new(db: TimelineDb) -> Self {
        Self { db }
    }
}

impl PlaceResolver for CachedPlaceResolver {
    fn resolve_album_places(&self, photos: &[TimelinePhoto]) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for bucket in dominant_gps_buckets(photos) {
            if let Some(name) = self
                .db
                .get_place(&bucket)?
                .and_then(|place| place.display_name())
            {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names = path_place_fallback(photos);
        }
        Ok(names)
    }
}

pub fn gps_bucket(lat: f64, lon: f64) -> String {
    format!("{:.3},{:.3}", normalize_zero(lat), normalize_zero(lon))
}

pub fn dominant_gps_buckets(photos: &[TimelinePhoto]) -> Vec<String> {
    let mut counts = HashMap::<String, usize>::new();
    for photo in photos {
        if let (Some(lat), Some(lon)) = (photo.gps_lat, photo.gps_lon) {
            if lat.is_finite()
                && lon.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon)
            {
                *counts.entry(gps_bucket(lat, lon)).or_default() += 1;
            }
        }
    }
    let mut ranked = counts.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|(left_bucket, left_count), (right_bucket, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_bucket.cmp(right_bucket))
    });
    ranked.into_iter().map(|(bucket, _)| bucket).collect()
}

pub fn path_place_fallback(photos: &[TimelinePhoto]) -> Vec<String> {
    let mut counts = HashMap::<&'static str, (usize, usize)>::new();
    for photo in photos {
        let mut seen = Vec::new();
        for component in photo.relative_path.split('/') {
            let normalized = component.trim().to_lowercase();
            if let Some((_, name)) = PATH_PLACES.iter().find(|(token, _)| *token == normalized) {
                if !seen.contains(name) {
                    seen.push(*name);
                    let entry = counts.entry(*name).or_insert((0, PATH_PLACES.len()));
                    entry.0 += 1;
                    entry.1 = entry.1.min(
                        PATH_PLACES
                            .iter()
                            .position(|(_, candidate)| candidate == name)
                            .unwrap_or(PATH_PLACES.len()),
                    );
                }
            }
        }
    }
    let mut ranked = counts.into_iter().collect::<Vec<_>>();
    ranked.sort_by(
        |(_, (left_count, left_order)), (_, (right_count, right_order))| {
            right_count
                .cmp(left_count)
                .then_with(|| left_order.cmp(right_order))
        },
    );
    ranked
        .into_iter()
        .map(|(name, _)| name.to_owned())
        .collect()
}

fn normalize_zero(value: f64) -> f64 {
    if value.abs() < 0.0005 {
        0.0
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dominant_gps_buckets, gps_bucket, path_place_fallback, CachedPlaceResolver, Place,
        PlaceResolver,
    };
    use crate::timeline::db::TimelineDb;
    use crate::timeline::models::{TimeSource, TimelinePhoto};

    fn photo(id: &str, path: &str, gps: Option<(f64, f64)>) -> TimelinePhoto {
        TimelinePhoto {
            id: id.into(),
            relative_path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            width: 1,
            height: 1,
            size_bytes: 1,
            format: "JPEG".into(),
            taken_at: None,
            time_source: TimeSource::Unknown,
            fingerprint: format!("fp-{id}"),
            gps_lat: gps.map(|coords| coords.0),
            gps_lon: gps.map(|coords| coords.1),
        }
    }

    #[test]
    fn rounds_gps_into_stable_thousandth_degree_bucket() {
        assert_eq!(gps_bucket(31.230416, 121.473701), "31.230,121.474");
        assert_eq!(gps_bucket(-0.0004, -73.98551), "0.000,-73.986");
    }

    #[test]
    fn chooses_dominant_buckets_by_count_then_bucket_key() {
        let photos = vec![
            photo("a", "a.jpg", Some((31.2304, 121.4737))),
            photo("b", "b.jpg", Some((35.6812, 139.7671))),
            photo("c", "c.jpg", Some((31.2303, 121.4738))),
            photo("d", "d.jpg", Some((35.6811, 139.7672))),
            photo("e", "e.jpg", Some((31.2302, 121.4739))),
            photo("none", "none.jpg", None),
        ];

        assert_eq!(
            dominant_gps_buckets(&photos),
            ["31.230,121.474", "35.681,139.767"]
        );
    }

    #[test]
    fn path_fallback_matches_only_complete_conservative_tokens() {
        let photos = vec![
            photo("a", "Trips/2024/Shanghai/IMG_1.jpg", None),
            photo("b", "Trips/Shanghai-family/IMG_2.jpg", None),
            photo("c", "Trips/shanghai/IMG_3.jpg", None),
            photo("d", "Trips/京都/IMG_4.jpg", None),
            photo("e", "Trips/Yorkshire/IMG_5.jpg", None),
        ];

        assert_eq!(path_place_fallback(&photos), ["上海", "京都"]);
    }

    #[test]
    fn path_fallback_orders_by_frequency_then_name_and_limits_output() {
        let photos = vec![
            photo("a", "苏州/a.jpg", None),
            photo("b", "上海/b.jpg", None),
            photo("c", "杭州/c.jpg", None),
            photo("d", "苏州/d.jpg", None),
            photo("e", "上海/e.jpg", None),
            photo("f", "南京/f.jpg", None),
        ];

        assert_eq!(
            path_place_fallback(&photos),
            ["上海", "苏州", "杭州", "南京"]
        );
    }

    #[test]
    fn cached_resolver_uses_dominant_gps_places_then_path_fallback() {
        let db = TimelineDb::open_in_memory().expect("db");
        for (bucket, city, lat, lon) in [
            ("31.230,121.474", "上海", 31.2304, 121.4737),
            ("35.681,139.767", "东京", 35.6812, 139.7671),
        ] {
            db.save_place(&Place {
                geo_bucket: bucket.into(),
                lat,
                lon,
                country: None,
                region: None,
                city: Some(city.into()),
                district: None,
                provider: "test".into(),
                resolved_at: "2024-01-01T00:00:00Z".into(),
            })
            .expect("cache place");
        }
        let resolver = CachedPlaceResolver::new(db);
        let gps_photos = vec![
            photo("shanghai-1", "Kyoto/a.jpg", Some((31.2304, 121.4737))),
            photo("tokyo", "Kyoto/b.jpg", Some((35.6812, 139.7671))),
            photo("shanghai-2", "Kyoto/c.jpg", Some((31.2303, 121.4738))),
        ];

        assert_eq!(
            resolver
                .resolve_album_places(&gps_photos)
                .expect("GPS resolution"),
            ["上海", "东京"]
        );
        assert_eq!(
            resolver
                .resolve_album_places(&[photo("path", "Trips/Kyoto/a.jpg", None)])
                .expect("path resolution"),
            ["京都"]
        );
    }

    #[test]
    fn display_name_prefers_city_then_district_region_country() {
        let base = Place {
            geo_bucket: "31.230,121.474".into(),
            lat: 31.2304,
            lon: 121.4737,
            country: Some("中国".into()),
            region: Some("上海市".into()),
            city: Some("上海".into()),
            district: Some("黄浦区".into()),
            provider: "test".into(),
            resolved_at: "2024-01-01T00:00:00Z".into(),
        };
        assert_eq!(base.display_name().as_deref(), Some("上海"));

        let mut district_only = base.clone();
        district_only.city = None;
        assert_eq!(district_only.display_name().as_deref(), Some("黄浦区"));
    }
}
