use crate::config::Config;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

mod generate;
pub use generate::{decode_image, generate_thumbnail, get_dimensions};

/// On-demand thumbnail generator with bounded blocking decode concurrency.
#[derive(Clone)]
pub struct ThumbnailPool {
    semaphore: Arc<Semaphore>,
}

impl ThumbnailPool {
    pub fn new(config: &Config) -> Self {
        Self::from_worker_count(config.builder_workers)
    }

    fn from_worker_count(workers: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(workers.max(1))),
        }
    }

    pub async fn generate(&self, source: PathBuf, thumb_path: PathBuf) -> anyhow::Result<Vec<u8>> {
        self.run_blocking(move || Self::generate_on_demand(&source, &thumb_path))
            .await
    }

    async fn run_blocking<F, T>(&self, operation: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("thumbnail worker pool is closed"))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation()
        })
        .await
        .map_err(|error| anyhow::anyhow!("thumbnail worker task failed: {error}"))?
    }

    /// Generate a single thumbnail synchronously for startup enrichment and watchers.
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

    #[tokio::test]
    async fn thumbnail_pool_bounds_blocking_decodes_to_configured_workers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let pool = ThumbnailPool::from_worker_count(1);
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let first_pool = pool.clone();
        let first_active = active.clone();
        let first_maximum = maximum.clone();
        let second_pool = pool;
        let second_active = active.clone();
        let second_maximum = maximum.clone();

        let first = first_pool.run_blocking(move || {
            let current = first_active.fetch_add(1, Ordering::SeqCst) + 1;
            first_maximum.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            first_active.fetch_sub(1, Ordering::SeqCst);
            Ok::<_, anyhow::Error>(())
        });
        let second = second_pool.run_blocking(move || {
            let current = second_active.fetch_add(1, Ordering::SeqCst) + 1;
            second_maximum.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            second_active.fetch_sub(1, Ordering::SeqCst);
            Ok::<_, anyhow::Error>(())
        });

        let (first, second) = tokio::join!(first, second);
        first.expect("first generation");
        second.expect("second generation");
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelling_waiter_does_not_release_decode_permit_early() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc;
        use tokio::sync::oneshot;

        let pool = ThumbnailPool::from_worker_count(1);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_pool = pool.clone();
        let first = tokio::spawn(async move {
            first_pool
                .run_blocking(move || {
                    started_tx.send(()).expect("announce first decode");
                    release_rx.recv().expect("release first decode");
                    Ok::<_, anyhow::Error>(())
                })
                .await
        });
        started_rx.await.expect("first decode starts");
        first.abort();

        let active = Arc::new(AtomicUsize::new(0));
        let second_active = active.clone();
        let second_pool = pool;
        let second = tokio::spawn(async move {
            second_pool
                .run_blocking(move || {
                    second_active.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, anyhow::Error>(())
                })
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(active.load(Ordering::SeqCst), 0);

        release_tx.send(()).expect("finish first decode");
        second
            .await
            .expect("second waiter task")
            .expect("second decode");
        assert_eq!(active.load(Ordering::SeqCst), 1);
    }
}
