use exif::{Context, Tag};
use serde::Serialize;
use std::io::BufReader;
use std::path::Path;

/// Structured EXIF data extracted from a photo.
#[derive(Debug, Clone, Serialize)]
pub struct ExifData {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<String>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub iso: Option<u32>,
    pub date_taken: Option<String>,
    pub timezone: Option<String>,
    pub gps: Option<GpsCoords>,
    pub dimensions: ImageDimensions,
    pub file_size: u64,
    pub format: String,
    pub flash: Option<String>,
    pub software: Option<String>,
    pub orientation: Option<u16>,
    pub artist: Option<String>,
    pub color_space: Option<String>,
    pub image_description: Option<String>,
    pub user_comment: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpsCoords {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}


/// Extract EXIF metadata from an image file.
pub fn extract_exif(path: &Path) -> anyhow::Result<ExifData> {
    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len();

    let format = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();

    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(&file);
    let exif_reader = exif::Reader::new();

    let mut data = ExifData {
        make: None,
        model: None,
        lens: None,
        focal_length: None,
        aperture: None,
        shutter_speed: None,
        iso: None,
        date_taken: None,
        timezone: None,
        gps: None,
        dimensions: ImageDimensions {
            width: 0,
            height: 0,
        },
        file_size,
        format: format.clone(),
        flash: None,
        software: None,
        orientation: None,
        artist: None,
        color_space: None,
        image_description: None,
        user_comment: None,
        tags: Vec::new(),
    };
    let mut gps_lat_ref: Option<String> = None;
    let mut gps_lon_ref: Option<String> = None;
    let mut gps_lat: Option<f64> = None;
    let mut gps_lon: Option<f64> = None;

    match exif_reader.read_from_container(&mut reader) {
        Ok(exif) => {
            for field in exif.fields() {
                match field.tag {
                    exif::Tag::Make => {
                        data.make = display_string(field);
                    }
                    exif::Tag::Model => {
                        data.model = display_string(field);
                    }
                    exif::Tag::ImageDescription => {
                        data.image_description = display_string(field);
                    }
                    exif::Tag::LensModel => {
                        data.lens = display_string(field);
                    }
                    exif::Tag::FocalLength => {
                        if let exif::Value::Rational(ref v) = field.value {
                            if !v.is_empty() {
                                data.focal_length = Some(format!("{:.0}mm", v[0].to_f64()));
                            }
                        }
                    }
                    exif::Tag::FNumber => {
                        if let exif::Value::Rational(ref v) = field.value {
                            if !v.is_empty() {
                                data.aperture = Some(format!("f/{:.1}", v[0].to_f64()));
                            }
                        }
                    }
                    exif::Tag::ExposureTime => {
                        if let exif::Value::Rational(ref v) = field.value {
                            if !v.is_empty() {
                                let val = v[0].to_f64();
                                if val < 1.0 {
                                    data.shutter_speed =
                                        Some(format!("1/{}s", (1.0 / val).round() as u32));
                                } else {
                                    data.shutter_speed = Some(format!("{:.1}s", val));
                                }
                            }
                        }
                    }
                    exif::Tag::ISOSpeed => {
                        if let exif::Value::Short(ref v) = field.value {
                            if !v.is_empty() {
                                data.iso = Some(v[0] as u32);
                            }
                        }
                    }
                    exif::Tag::DateTimeOriginal => {
                        data.date_taken = display_string(field);
                    }
                    exif::Tag::OffsetTimeOriginal => {
                        data.timezone = display_string(field).and_then(|s| normalize_timezone(&s));
                    }
                    exif::Tag::OffsetTime => {
                        if data.timezone.is_none() {
                            data.timezone =
                                display_string(field).and_then(|s| normalize_timezone(&s));
                        }
                    }
                    exif::Tag::Flash => {
                        if let exif::Value::Short(ref v) = field.value {
                            if !v.is_empty() {
                                data.flash = Some(match v[0] {
                                    0 => "No Flash".into(),
                                    1 => "Fired".into(),
                                    _ => format!("Flash mode {}", v[0]),
                                });
                            }
                        }
                    }
                    exif::Tag::Software => {
                        data.software = display_string(field);
                    }
                    exif::Tag::Artist => {
                        data.artist = display_string(field);
                    }
                    exif::Tag::ColorSpace => {
                        data.color_space = display_string(field);
                    }
                    exif::Tag::UserComment => {
                        data.user_comment =
                            decode_user_comment(&field.value).or_else(|| display_string(field));
                    }
                    exif::Tag::Orientation => {
                        if let exif::Value::Short(ref v) = field.value {
                            if !v.is_empty() {
                                data.orientation = Some(v[0]);
                            }
                        }
                    }
                    exif::Tag::ImageWidth => {
                        if let exif::Value::Long(ref v) = field.value {
                            if !v.is_empty() {
                                data.dimensions.width = v[0];
                            }
                        } else if let exif::Value::Short(ref v) = field.value {
                            if !v.is_empty() {
                                data.dimensions.width = v[0] as u32;
                            }
                        }
                    }
                    exif::Tag::ImageLength => {
                        if let exif::Value::Long(ref v) = field.value {
                            if !v.is_empty() {
                                data.dimensions.height = v[0];
                            }
                        } else if let exif::Value::Short(ref v) = field.value {
                            if !v.is_empty() {
                                data.dimensions.height = v[0] as u32;
                            }
                        }
                    }
                    // GPS
                    exif::Tag::GPSLatitude => {
                        if let exif::Value::Rational(v) = &field.value {
                            if v.len() >= 3 {
                                gps_lat = Some(
                                    v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0,
                                );
                            }
                        }
                    }
                    exif::Tag::GPSLongitude => {
                        if let exif::Value::Rational(v) = &field.value {
                            if v.len() >= 3 {
                                gps_lon = Some(
                                    v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0,
                                );
                            }
                        }
                    }

                    exif::Tag::GPSLatitudeRef => {
                        gps_lat_ref = display_string(field);
                    }
                    exif::Tag::GPSLongitudeRef => {
                        gps_lon_ref = display_string(field);
                    }
                    tag if tag == Tag(Context::Tiff, 0x9c9c) => {
                        data.user_comment =
                            parse_xp_string(&field.value).or_else(|| display_string(field));
                    }
                    tag if tag == Tag(Context::Tiff, 0x9c9d) => {
                        data.artist =
                            parse_xp_string(&field.value).or_else(|| display_string(field));
                    }
                    tag if tag == Tag(Context::Tiff, 0x9c9e) => {
                        data.tags = parse_keywords(&field.value);
                    }
                    tag if tag == Tag(Context::Tiff, 0x9c9f) => {
                        data.image_description =
                            parse_xp_string(&field.value).or_else(|| display_string(field));
                    }
                    _ => {}
                }
            }
        }
        Err(_) => {
            // EXIF parse failed; continue with basic info only
        }
    }

    data.gps = build_gps_coords(gps_lat, gps_lon);

    apply_gps_refs(&mut data, gps_lat_ref.as_deref(), gps_lon_ref.as_deref());

    // If dimensions weren't found in EXIF, try to get them from image metadata
    if data.dimensions.width == 0 || data.dimensions.height == 0 {
        if let Ok((w, h)) = crate::thumbnail::get_dimensions(path) {
            data.dimensions.width = w;
            data.dimensions.height = h;
        }
    }



    if data.tags.is_empty() {
        data.tags = data
            .image_description
            .as_deref()
            .map(parse_inline_tags)
            .unwrap_or_default();
    }

    Ok(data)
}

fn build_gps_coords(lat: Option<f64>, lon: Option<f64>) -> Option<GpsCoords> {
    Some(GpsCoords {
        lat: lat?,
        lon: lon?,
    })
}

fn apply_gps_refs(data: &mut ExifData, lat_ref: Option<&str>, lon_ref: Option<&str>) {
    if let Some(gps) = data.gps.as_mut() {
        if lat_ref == Some("S") {
            gps.lat = -gps.lat.abs();
        }
        if lon_ref == Some("W") {
            gps.lon = -gps.lon.abs();
        }
    }
}

fn display_string(field: &exif::Field) -> Option<String> {
    clean_exif_string(&field.display_value().to_string())
}

fn clean_exif_string(raw: &str) -> Option<String> {
    let value = raw
        .trim_matches('\0')
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string();

    if value.is_empty() || value == "-" {
        None
    } else {
        Some(value)
    }
}

fn normalize_timezone(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("z") || trimmed == "+00:00" || trimmed == "-00:00" {
        return Some("UTC".into());
    }

    let sign = trimmed.chars().next()?;
    if sign != '+' && sign != '-' {
        return clean_exif_string(trimmed);
    }

    let rest = &trimmed[sign.len_utf8()..];
    let (hours, minutes) = rest.split_once(':').unwrap_or((rest, "00"));
    let hours = hours.trim_start_matches('0');
    let hours = if hours.is_empty() { "0" } else { hours };
    let minutes = minutes.trim_end_matches('0');

    if minutes.is_empty() {
        Some(format!("UTC{}{}", sign, hours))
    } else {
        Some(format!("UTC{}{}:{}", sign, hours, minutes))
    }
}

fn decode_user_comment(value: &exif::Value) -> Option<String> {
    let bytes = match value {
        exif::Value::Undefined(bytes, _) => bytes.as_slice(),
        exif::Value::Byte(bytes) => bytes.as_slice(),
        _ => return None,
    };

    let text = if bytes.starts_with(b"ASCII\0\0\0") {
        String::from_utf8_lossy(&bytes[8..]).into_owned()
    } else if bytes.starts_with(b"UNICODE\0") {
        decode_utf16(&bytes[8..])
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };

    clean_exif_string(&text)
}

fn parse_xp_string(value: &exif::Value) -> Option<String> {
    let bytes = match value {
        exif::Value::Byte(bytes) => bytes.as_slice(),
        exif::Value::Undefined(bytes, _) => bytes.as_slice(),
        _ => return None,
    };

    clean_exif_string(&decode_utf16(bytes))
}

fn decode_utf16(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

fn parse_keywords(value: &exif::Value) -> Vec<String> {
    parse_xp_string(value)
        .as_deref()
        .map(parse_inline_tags)
        .unwrap_or_default()
}

fn parse_inline_tags(value: &str) -> Vec<String> {
    value
        .split([';', ',', '，', '、', '#'])
        .filter_map(clean_exif_string)
        .take(24)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_coordinates_require_both_axes_before_serializing() {
        assert!(build_gps_coords(Some(35.6586), None).is_none());
        assert!(build_gps_coords(None, Some(139.7454)).is_none());

        let gps = build_gps_coords(Some(0.0), Some(139.7454)).expect("complete gps coords");
        assert_eq!(gps.lat, 0.0);
        assert_eq!(gps.lon, 139.7454);
    }

    #[test]
    fn gps_refs_apply_after_coordinates_regardless_of_tag_order() {
        let mut data = empty_exif_data();
        data.gps = Some(GpsCoords {
            lat: 35.6586,
            lon: 139.7454,
        });

        apply_gps_refs(&mut data, Some("S"), Some("W"));

        let gps = data.gps.expect("gps coords");
        assert_eq!(gps.lat, -35.6586);
        assert_eq!(gps.lon, -139.7454);
    }

    fn empty_exif_data() -> ExifData {
        ExifData {
            make: None,
            model: None,
            lens: None,
            focal_length: None,
            aperture: None,
            shutter_speed: None,
            iso: None,
            date_taken: None,
            timezone: None,
            gps: None,
            dimensions: ImageDimensions {
                width: 0,
                height: 0,
            },
            file_size: 0,
            format: "JPG".into(),
            flash: None,
            software: None,
            orientation: None,
            artist: None,
            color_space: None,
            image_description: None,
            user_comment: None,
            tags: Vec::new(),
        }
    }
}
