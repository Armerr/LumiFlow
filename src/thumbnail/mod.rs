use crate::config::Config;
use crate::scanner::walk;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

mod generate;
pub use generate::{decode_image, generate_thumbnail, get_dimensions};

/// Background thumbnail generator with bounded concurrency.
#[derive(Clone)]
pub struct ThumbnailPool {
    semaphore: Arc<Semaphore>,
}

impl ThumbnailPool {
    pub fn new(config: &Config) -> Self {
        let workers = config.builder_workers.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(workers)),
        }
    }

    /// Pre-generate thumbnails for all albums in the background.
    pub fn pregenerate_all(&self, photos_path: PathBuf, data_path: PathBuf, exclude_regex: String) {
        let sem = self.semaphore.clone();

        tokio::spawn(async move {
            tracing::info!("starting background thumbnail pre-generation...");

            let regex = match regex::Regex::new(&exclude_regex) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("invalid exclude regex for pregeneration: {}", e);
                    return;
                }
            };

            let manifest = match walk::scan(&photos_path, &exclude_regex) {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!("pregenerate scan failed: {}", e);
                    return;
                }
            };

            let mut total = 0usize;
            let mut generated = 0usize;
            let mut errors = 0usize;

            for album in &manifest.albums {
                let album_dir = photos_path.join(&album.name);
                let photos = walk::scan_album_photos(&album_dir, &regex);
                total += photos.len();

                for photo in &photos {
                    let _permit = sem.acquire().await;
                    let source = album_dir.join(&photo.name);
                    let thumb_path = thumb_path(&data_path, &album.name, &photo.name);

                    if thumb_is_fresh(&thumb_path, &source) {
                        continue;
                    }

                    match generate_thumbnail(&source, 400, 80.0) {
                        Ok(data) => {
                            if let Some(parent) = thumb_path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if std::fs::write(&thumb_path, &data).is_ok() {
                                generated += 1;
                            }
                        }
                        Err(e) => {
                            errors += 1;
                            if errors <= 5 {
                                tracing::warn!("thumbnail failed for {:?}: {}", source, e);
                            }
                        }
                    }
                }
            }

            if errors > 5 {
                tracing::warn!("... and {} more thumbnail errors", errors - 5);
            }
            tracing::info!(
                "thumbnail pre-generation done: {}/{} generated",
                generated,
                total
            );
        });
    }

    /// Generate a single thumbnail on-demand, returning the WebP data.
    pub fn generate_on_demand(source: &Path, thumb_path: &Path) -> anyhow::Result<Vec<u8>> {
        let data = generate_thumbnail(source, 400, 80.0)?;
        if let Some(parent) = thumb_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(thumb_path, &data)?;
        Ok(data)
    }
}

/// Construct the filesystem path for a cached thumbnail.
pub fn thumb_path(data_path: &Path, album: &str, filename: &str) -> PathBuf {
    data_path
        .join("thumbs")
        .join(album)
        .join(format!("{}.webp", filename))
}

/// A cached thumbnail is fresh if it exists and is not older than its source.
fn thumb_is_fresh(thumb_path: &Path, source: &Path) -> bool {
    if !thumb_path.exists() {
        return false;
    }
    let thumb_mtime = thumb_path.metadata().ok().and_then(|m| m.modified().ok());
    let source_mtime = source.metadata().ok().and_then(|m| m.modified().ok());
    match (thumb_mtime, source_mtime) {
        (Some(t), Some(s)) => t >= s,
        _ => false,
    }
}

/// Construct the filesystem path for a timeline thumbnail keyed by stable photo ID.
pub fn timeline_thumb_path(data_path: &Path, photo_id: &str) -> PathBuf {
    data_path
        .join("thumbs")
        .join("by-id")
        .join(format!("{photo_id}.webp"))
}

/// Return whether a by-ID timeline thumbnail exists for the exact source fingerprint.
pub fn timeline_thumb_is_fresh(data_path: &Path, photo_id: &str, fingerprint: &str) -> bool {
    timeline_thumb_path(data_path, photo_id).is_file()
        && timeline_thumb_fingerprint_path(data_path, photo_id).is_file()
        && std::fs::read_to_string(timeline_thumb_fingerprint_path(data_path, photo_id))
            .is_ok_and(|cached| cached == fingerprint)
}

/// Persist the source fingerprint alongside a generated timeline thumbnail.
pub fn write_timeline_thumb_fingerprint(
    data_path: &Path,
    photo_id: &str,
    fingerprint: &str,
) -> std::io::Result<()> {
    let path = timeline_thumb_fingerprint_path(data_path, photo_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, fingerprint)
}

fn timeline_thumb_fingerprint_path(data_path: &Path, photo_id: &str) -> PathBuf {
    data_path
        .join("thumbs")
        .join("by-id")
        .join(format!("{photo_id}.fingerprint"))
}

#[cfg(test)]
mod timeline_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn timeline_thumbnail_path_is_keyed_only_by_photo_id() {
        let data = Path::new("data");
        assert_eq!(
            timeline_thumb_path(data, "0123456789abcdef"),
            data.join("thumbs/by-id/0123456789abcdef.webp")
        );
    }

    #[test]
    fn timeline_thumbnail_freshness_uses_exact_fingerprint_metadata() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let data = std::env::temp_dir().join(format!(
            "lumiflow-timeline-thumb-{}-{nonce}",
            std::process::id()
        ));
        let thumbnail = timeline_thumb_path(&data, "photo-id");
        std::fs::create_dir_all(thumbnail.parent().expect("thumbnail parent"))
            .expect("create cache");
        std::fs::write(&thumbnail, b"webp").expect("write thumbnail");

        assert!(!timeline_thumb_is_fresh(&data, "photo-id", "fp-1"));
        write_timeline_thumb_fingerprint(&data, "photo-id", "fp-1").expect("write fingerprint");
        assert!(timeline_thumb_is_fresh(&data, "photo-id", "fp-1"));
        assert!(!timeline_thumb_is_fresh(&data, "photo-id", "fp-2"));

        let _ = std::fs::remove_dir_all(data);
    }
}
