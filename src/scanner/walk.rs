use crate::scanner::manifest::{Album, Manifest, PhotoEntry};
use anyhow::Context;
use chrono::{DateTime, Utc};
use regex::Regex;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "heic", "heif", "avif", "tif", "tiff",
];

/// Scan the photos directory and build a manifest.
pub fn scan(photos_path: &Path, exclude_regex: &str) -> anyhow::Result<Manifest> {
    let exclude = Regex::new(exclude_regex).context("invalid exclude regex")?;
    let mut albums: Vec<Album> = Vec::new();

    if !photos_path.exists() {
        tracing::warn!("photos path does not exist: {:?}", photos_path);
        return Ok(Manifest {
            updated: Utc::now(),
            albums: Vec::new(),
        });
    }

    let entries: Vec<_> = fs::read_dir(photos_path)
        .context("failed to read photos directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();

    for entry in &entries {
        let dir_name = entry.file_name().to_string_lossy().to_string();

        // Skip excluded directories (NAS metadata etc.)
        let dir_path_str = format!("/{}", dir_name);
        if exclude.is_match(&dir_path_str) || exclude.is_match(&dir_name) {
            tracing::debug!("skipping excluded directory: {}", dir_name);
            continue;
        }

        let album_path = entry.path();
        let photos = scan_album_photos(&album_path, &exclude);

        if photos.is_empty() {
            tracing::debug!("skipping empty album: {}", dir_name);
            continue;
        }

        let created_at = dir_creation_time(&album_path);
        let updated_at = album_latest_mtime(&album_path, &photos);

        albums.push(Album {
            cover: photos.first().map(|p| p.name.clone()).unwrap_or_default(),
            count: photos.len(),
            name: dir_name,
            created_at,
            updated_at,
        });
    }

    // Sort by creation time, newest first.
    albums.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(Manifest {
        updated: Utc::now(),
        albums,
    })
}

/// Scan photos in a single album directory.
/// Returns photos sorted by filename.
pub fn scan_album_photos(album_path: &Path, exclude: &Regex) -> Vec<PhotoEntry> {
    let mut photos: Vec<PhotoEntry> = WalkDir::new(album_path)
        .min_depth(1)
        .max_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            // Filter out hidden files and excluded patterns.
            let name = e.file_name().to_string_lossy();
            !exclude.is_match(&name)
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_string_lossy().to_string();
            let meta = path.metadata().ok()?;
            let format = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_uppercase();

            Some(PhotoEntry {
                id: 0, // assigned after sorting
                name,
                width: 0,  // populated later during thumbnail generation
                height: 0, // populated later during thumbnail generation
                size_bytes: meta.len(),
                format,
            })
        })
        .collect();

    // Assign sequential IDs.
    for (i, photo) in photos.iter_mut().enumerate() {
        photo.id = i;
    }

    photos
}

/// Get album-level detail (photos list) for the API.
pub fn get_album_detail(
    photos_path: &Path,
    album_name: &str,
    exclude: &Regex,
) -> Option<Vec<PhotoEntry>> {
    let album_path = photos_path.join(album_name);
    if !album_path.is_dir() {
        return None;
    }
    Some(scan_album_photos(&album_path, exclude))
}

// --- Helpers ---

/// Get directory creation time (birth time) if available, else modification time.
fn dir_creation_time(path: &Path) -> DateTime<Utc> {
    path.metadata()
        .ok()
        .and_then(|m| m.created().ok())
        .or_else(|| path.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|t| {
            let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
            DateTime::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos()).unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Get the latest modification time among all photos in the album.
fn album_latest_mtime(album_path: &Path, photos: &[PhotoEntry]) -> DateTime<Utc> {
    photos
        .iter()
        .filter_map(|p| {
            album_path
                .join(&p.name)
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
        })
        .map(|t| {
            let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
            DateTime::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos()).unwrap_or_default()
        })
        .max()
        .unwrap_or_else(Utc::now)
}
