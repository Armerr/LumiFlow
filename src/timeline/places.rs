use crate::timeline::db::TimelineDb;
use crate::timeline::models::TimelinePhoto;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::blocking::Client;
use reqwest::Url;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

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

const NOMINATIM_PROVIDER: &str = "nominatim";
const PROJECT_USER_AGENT: &str = concat!("LumiFlow/", env!("CARGO_PKG_VERSION"));

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

#[derive(Clone)]
pub struct NominatimPlaceResolver {
    db: TimelineDb,
    client: Client,
    reverse_url: Url,
}

impl NominatimPlaceResolver {
    pub fn new(db: TimelineDb, base_url: &str, timeout: Duration) -> Result<Self> {
        let reverse_url = reverse_url(base_url)?;
        let client = Client::builder()
            .timeout(timeout)
            .user_agent(PROJECT_USER_AGENT)
            .build()
            .context("failed to build Nominatim HTTP client")?;
        Ok(Self {
            db,
            client,
            reverse_url,
        })
    }

    pub fn with_default_timeout(db: TimelineDb, base_url: &str) -> Result<Self> {
        Self::new(db, base_url, Duration::from_secs(10))
    }

    fn fetch_place(&self, bucket: &str) -> Result<Place> {
        let (lat, lon) = bucket.split_once(',').context("invalid GPS bucket")?;
        let parsed_lat = lat.parse::<f64>().context("invalid GPS bucket latitude")?;
        let parsed_lon = lon.parse::<f64>().context("invalid GPS bucket longitude")?;
        let response = self
            .client
            .get(self.reverse_url.clone())
            .query(&[
                ("format", "jsonv2"),
                ("lat", lat),
                ("lon", lon),
                ("addressdetails", "1"),
            ])
            .send()
            .context("Nominatim request failed")?
            .error_for_status()
            .context("Nominatim returned an error response")?;
        let body = response
            .json::<NominatimResponse>()
            .context("invalid Nominatim response JSON")?;
        let address = body.address;
        Ok(Place {
            geo_bucket: bucket.to_owned(),
            lat: parsed_lat,
            lon: parsed_lon,
            country: clean(address.country),
            region: clean(address.state),
            city: first_non_empty([
                address.city,
                address.town,
                address.village,
                address.county.clone(),
            ]),
            district: first_non_empty([address.suburb, address.county]),
            provider: NOMINATIM_PROVIDER.into(),
            resolved_at: Utc::now().to_rfc3339(),
        })
    }
}

impl PlaceResolver for NominatimPlaceResolver {
    fn resolve_album_places(&self, photos: &[TimelinePhoto]) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for bucket in dominant_gps_buckets(photos) {
            let place = match self.db.get_place(&bucket)? {
                Some(place) => Some(place),
                None => match self.fetch_place(&bucket) {
                    Ok(place) => {
                        self.db.save_place(&place)?;
                        Some(place)
                    }
                    Err(error) => {
                        tracing::warn!(geo_bucket = %bucket, error = %error, "reverse geocoding failed");
                        None
                    }
                },
            };
            if let Some(name) = place.and_then(|place| place.display_name()) {
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

#[derive(Deserialize)]
struct NominatimResponse {
    #[serde(default)]
    address: NominatimAddress,
}

#[derive(Default, Deserialize)]
struct NominatimAddress {
    country: Option<String>,
    state: Option<String>,
    city: Option<String>,
    town: Option<String>,
    village: Option<String>,
    suburb: Option<String>,
    county: Option<String>,
}

fn reverse_url(base_url: &str) -> Result<Url> {
    let trimmed = base_url.trim().trim_end_matches('/');
    anyhow::ensure!(!trimmed.is_empty(), "Nominatim base URL is required");
    let endpoint = if trimmed.ends_with("/reverse") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/reverse")
    };
    let url = Url::parse(&endpoint)
        .with_context(|| format!("invalid Nominatim base URL `{base_url}`"))?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "invalid Nominatim base URL `{base_url}`: expected http or https"
    );
    Ok(url)
}

fn clean(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn first_non_empty<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values.into_iter().find_map(clean)
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
        dominant_gps_buckets, gps_bucket, path_place_fallback, CachedPlaceResolver,
        NominatimPlaceResolver, Place, PlaceResolver,
    };
    use crate::timeline::db::TimelineDb;
    use crate::timeline::models::{TimeSource, TimelinePhoto};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn photo(id: &str, path: &str, gps: Option<(f64, f64)>) -> TimelinePhoto {
        TimelinePhoto {
            id: id.into(),
            relative_path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            width: 1,
            height: 1,
            size_bytes: 1,
            mtime_ns: 1_704_067_200_000_000_000,
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
    fn nominatim_fetches_uncached_bucket_parses_address_and_reuses_cache() {
        let response = serde_json::json!({
            "address": {
                "country": "日本",
                "state": "東京都",
                "city": "千代田区",
                "town": "ignored town",
                "village": "ignored village",
                "suburb": "丸の内",
                "county": "ignored county"
            }
        })
        .to_string();
        let server = OneShotServer::start(ResponseAction::Reply {
            status: 200,
            content_type: "application/json",
            body: response.into_bytes(),
        });
        let db = TimelineDb::open_in_memory().expect("db");
        let resolver =
            NominatimPlaceResolver::new(db.clone(), &server.base_url(), Duration::from_secs(10))
                .expect("resolver");
        let photos = vec![
            photo("tokyo-1", "Trips/a.jpg", Some((35.6812, 139.7671))),
            photo("tokyo-2", "Trips/b.jpg", Some((35.6813, 139.7672))),
        ];

        assert_eq!(
            resolver
                .resolve_album_places(&photos)
                .expect("network place"),
            ["千代田区"]
        );
        let request = server.finish();
        assert_eq!(
            request.path,
            "/reverse?format=jsonv2&lat=35.681&lon=139.767&addressdetails=1"
        );
        assert!(
            request
                .user_agent
                .as_deref()
                .is_some_and(|value| value.contains("LumiFlow")),
            "project User-Agent must identify LumiFlow"
        );
        assert_eq!(
            db.get_place("35.681,139.767")
                .expect("cache lookup")
                .expect("cached place"),
            Place {
                geo_bucket: "35.681,139.767".into(),
                lat: 35.681,
                lon: 139.767,
                country: Some("日本".into()),
                region: Some("東京都".into()),
                city: Some("千代田区".into()),
                district: Some("丸の内".into()),
                provider: "nominatim".into(),
                resolved_at: db
                    .get_place("35.681,139.767")
                    .expect("cache lookup")
                    .expect("cached place")
                    .resolved_at,
            }
        );

        assert_eq!(
            resolver
                .resolve_album_places(&photos)
                .expect("cached place"),
            ["千代田区"]
        );
    }

    #[test]
    fn nominatim_address_falls_back_across_city_town_village_and_district_fields() {
        let cases = [
            (
                serde_json::json!({"address":{"town":"Oxford","county":"Oxfordshire"}}),
                "Oxford",
                "Oxfordshire",
            ),
            (
                serde_json::json!({"address":{"village":"Bibury","county":"Gloucestershire"}}),
                "Bibury",
                "Gloucestershire",
            ),
            (
                serde_json::json!({"address":{"county":"Cook County"}}),
                "Cook County",
                "Cook County",
            ),
        ];

        for (index, (response, expected_city, expected_district)) in cases.into_iter().enumerate() {
            let server = OneShotServer::start(ResponseAction::Reply {
                status: 200,
                content_type: "application/json",
                body: response.to_string().into_bytes(),
            });
            let db = TimelineDb::open_in_memory().expect("db");
            let resolver = NominatimPlaceResolver::new(
                db.clone(),
                &server.base_url(),
                Duration::from_secs(10),
            )
            .expect("resolver");
            let lat = 40.0 + index as f64;
            resolver
                .resolve_album_places(&[photo("gps", "Trips/a.jpg", Some((lat, -70.0)))])
                .expect("place");
            server.finish();
            let cached = db
                .get_place(&gps_bucket(lat, -70.0))
                .expect("cache")
                .expect("cached place");
            assert_eq!(cached.city.as_deref(), Some(expected_city));
            assert_eq!(cached.district.as_deref(), Some(expected_district));
        }
    }

    #[test]
    fn nominatim_failures_fall_back_without_poisoning_cache() {
        let cases = [
            ResponseAction::Reply {
                status: 429,
                content_type: "text/plain",
                body: b"rate limited".to_vec(),
            },
            ResponseAction::Reply {
                status: 200,
                content_type: "application/json",
                body: b"not-json".to_vec(),
            },
            ResponseAction::Delay(Duration::from_millis(150)),
        ];

        for action in cases {
            let server = OneShotServer::start(action);
            let db = TimelineDb::open_in_memory().expect("db");
            let resolver = NominatimPlaceResolver::new(
                db.clone(),
                &server.base_url(),
                Duration::from_millis(25),
            )
            .expect("resolver");
            let photos = [photo("gps", "Trips/Kyoto/a.jpg", Some((35.6812, 139.7671)))];

            assert_eq!(
                resolver
                    .resolve_album_places(&photos)
                    .expect("provider failure is isolated"),
                ["京都"]
            );
            server.finish();
            assert_eq!(db.get_place("35.681,139.767").expect("cache lookup"), None);
        }
    }

    #[test]
    fn nominatim_continues_to_next_dominant_bucket_after_failure() {
        let server = SequentialServer::start([
            ResponseAction::Reply {
                status: 503,
                content_type: "text/plain",
                body: b"unavailable".to_vec(),
            },
            ResponseAction::Reply {
                status: 200,
                content_type: "application/json",
                body: br#"{"address":{"city":"Tokyo"}}"#.to_vec(),
            },
        ]);
        let db = TimelineDb::open_in_memory().expect("db");
        let resolver =
            NominatimPlaceResolver::new(db.clone(), &server.base_url(), Duration::from_secs(10))
                .expect("resolver");
        let photos = [
            photo("a", "Trips/a.jpg", Some((31.2304, 121.4737))),
            photo("b", "Trips/b.jpg", Some((35.6812, 139.7671))),
        ];

        assert_eq!(
            resolver
                .resolve_album_places(&photos)
                .expect("failure isolation"),
            ["Tokyo"]
        );
        assert_eq!(
            server.finish(),
            [
                "/reverse?format=jsonv2&lat=31.230&lon=121.474&addressdetails=1",
                "/reverse?format=jsonv2&lat=35.681&lon=139.767&addressdetails=1"
            ]
        );
        assert_eq!(db.get_place("31.230,121.474").expect("cache miss"), None);
        assert!(db
            .get_place("35.681,139.767")
            .expect("cache lookup")
            .is_some());
    }

    #[test]
    fn nominatim_appends_reverse_once_and_rejects_invalid_urls() {
        let db = TimelineDb::open_in_memory().expect("db");
        assert!(
            NominatimPlaceResolver::new(db.clone(), "not a URL", Duration::from_secs(10)).is_err()
        );

        let server = OneShotServer::start(ResponseAction::Reply {
            status: 200,
            content_type: "application/json",
            body: br#"{"address":{"city":"Paris"}}"#.to_vec(),
        });
        let resolver = NominatimPlaceResolver::new(
            db,
            &format!("{}/reverse", server.base_url()),
            Duration::from_secs(10),
        )
        .expect("resolver");
        resolver
            .resolve_album_places(&[photo("gps", "a.jpg", Some((48.857, 2.352)))])
            .expect("place");
        assert!(server.finish().path.starts_with("/reverse?"));
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

    #[derive(Debug)]
    struct CapturedRequest {
        path: String,
        user_agent: Option<String>,
    }

    enum ResponseAction {
        Reply {
            status: u16,
            content_type: &'static str,
            body: Vec<u8>,
        },
        Delay(Duration),
    }

    struct OneShotServer {
        base_url: String,
        request_rx: mpsc::Receiver<CapturedRequest>,
        join: thread::JoinHandle<()>,
    }

    impl OneShotServer {
        fn start(action: ResponseAction) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
            let address = listener.local_addr().expect("server address");
            let (request_tx, request_rx) = mpsc::channel();
            let join = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept request");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("read timeout");
                let headers = read_headers(&mut stream);
                let request_line = headers.lines().next().expect("request line");
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .expect("request path")
                    .to_owned();
                let user_agent = header_value(&headers, "user-agent");
                request_tx
                    .send(CapturedRequest { path, user_agent })
                    .expect("capture request");

                match action {
                    ResponseAction::Reply {
                        status,
                        content_type,
                        body,
                    } => {
                        write!(
                            stream,
                            "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .expect("write response headers");
                        stream.write_all(&body).expect("write response body");
                    }
                    ResponseAction::Delay(duration) => thread::sleep(duration),
                }
            });
            Self {
                base_url: format!("http://{address}"),
                request_rx,
                join,
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn finish(self) -> CapturedRequest {
            let request = self
                .request_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("captured request");
            self.join.join().expect("server thread");
            request
        }
    }

    struct SequentialServer {
        base_url: String,
        requests_rx: mpsc::Receiver<Vec<String>>,
        join: thread::JoinHandle<()>,
    }

    impl SequentialServer {
        fn start<const N: usize>(actions: [ResponseAction; N]) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
            let address = listener.local_addr().expect("server address");
            let (requests_tx, requests_rx) = mpsc::channel();
            let join = thread::spawn(move || {
                let mut paths = Vec::with_capacity(N);
                for action in actions {
                    let (mut stream, _) = listener.accept().expect("accept request");
                    let headers = read_headers(&mut stream);
                    paths.push(
                        headers
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .expect("request path")
                            .to_owned(),
                    );
                    match action {
                        ResponseAction::Reply {
                            status,
                            content_type,
                            body,
                        } => {
                            write!(
                                stream,
                                "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .expect("write response headers");
                            stream.write_all(&body).expect("write response body");
                        }
                        ResponseAction::Delay(duration) => thread::sleep(duration),
                    }
                }
                requests_tx.send(paths).expect("capture requests");
            });
            Self {
                base_url: format!("http://{address}"),
                requests_rx,
                join,
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn finish(self) -> Vec<String> {
            let requests = self
                .requests_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("captured requests");
            self.join.join().expect("server thread");
            requests
        }
    }

    fn read_headers(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let count = stream.read(&mut chunk).expect("read request");
            assert!(count > 0, "request ended before headers");
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                return String::from_utf8(bytes).expect("request headers");
            }
        }
    }

    fn header_value(headers: &str, expected: &str) -> Option<String> {
        headers.lines().find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case(expected)
                    .then(|| value.trim().to_owned())
            })
        })
    }
}
