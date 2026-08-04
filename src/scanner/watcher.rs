use crate::scanner::manifest::Manifest;
use crate::scanner::walk;
use crate::thumbnail::ThumbnailPool;
use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Start the filesystem watcher and periodic rescan loop.
/// Runs as background tasks; never returns unless cancelled.
pub fn start(
    photos_path: PathBuf,
    data_path: PathBuf,
    exclude_regex: String,
    manifest: Arc<RwLock<Manifest>>,
) {
    // --- Periodic full-rescan fallback (every 30 minutes) ---
    {
        let photos = photos_path.clone();
        let data = data_path.clone();
        let exclude = exclude_regex.clone();
        let mf = manifest.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30 * 60));
            loop {
                interval.tick().await;
                tracing::debug!("periodic rescan tick");
                rescan_and_sync(&photos, &data, &exclude, &mf).await;
            }
        });
    }

    // --- Real-time filesystem watcher with debounce ---
    {
        let photos = photos_path.clone();
        let data = data_path.clone();
        let exclude = exclude_regex.clone();
        let mf = manifest.clone();

        tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

            let mut watcher =
                match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        let _ = tx.send(event);
                    }
                }) {
                    Ok(w) => w,
                    Err(e) => {
                        tracing::warn!("filesystem watcher unavailable: {}; periodic scan only", e);
                        return;
                    }
                };

            if let Err(e) = watcher.watch(&photos, RecursiveMode::Recursive) {
                tracing::warn!("cannot watch {:?}: {}; periodic scan only", photos, e);
                return;
            }

            tracing::info!("filesystem watcher active on {:?}", photos);

            loop {
                // Wait for first event
                let mut has_relevant = match rx.recv().await {
                    Some(e) => is_relevant_change(&e),
                    None => break,
                };

                if !has_relevant {
                    // Keep draining until we find a relevant event or timeout
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
                    loop {
                        match tokio::time::timeout_at(deadline, rx.recv()).await {
                            Ok(Some(e)) => {
                                if is_relevant_change(&e) {
                                    has_relevant = true;
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                }

                if !has_relevant {
                    continue;
                }

                // Drain remaining events within debounce window
                let debounce = Duration::from_secs(5);
                let deadline = tokio::time::Instant::now() + debounce;
                loop {
                    match tokio::time::timeout_at(deadline, rx.recv()).await {
                        Ok(Some(_)) => { /* drain */ }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }

                rescan_and_sync(&photos, &data, &exclude, &mf).await;
            }
        });
    }
}

fn should_run_periodic_timeline_rescan(state: crate::timeline::ScanState) -> bool {
    matches!(state, crate::timeline::ScanState::Ready)
}


/// Start timeline-mode periodic and recursive notify rescans.
pub fn start_timeline(photos_path: PathBuf, service: Arc<crate::timeline::TimelineService>) {
    let periodic_service = service.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if !should_run_periodic_timeline_rescan(periodic_service.status().state) {
                tracing::debug!("skip periodic timeline rescan while initial/active scan is not ready");
                continue;
            }
            if let Err(error) = periodic_service.rescan().await {
                tracing::error!("periodic timeline rescan failed: {error:#}");
            }
        }
    });

    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            match notify::recommended_watcher(move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    let _ = tx.send(event);
                }
            }) {
                Ok(watcher) => watcher,
                Err(error) => {
                    tracing::warn!("timeline filesystem watcher unavailable: {error}");
                    return;
                }
            };
        if let Err(error) = watcher.watch(&photos_path, RecursiveMode::Recursive) {
            tracing::warn!("cannot watch timeline root {photos_path:?}: {error}");
            return;
        }
        tracing::info!("timeline filesystem watcher active on {photos_path:?}");

        while let Some(event) = rx.recv().await {
            if !is_relevant_change(&event) {
                continue;
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut paths = event.paths;
            while let Ok(Some(next)) = tokio::time::timeout_at(deadline, rx.recv()).await {
                if is_relevant_change(&next) { paths.extend(next.paths); }
            }
            paths.sort();
            paths.dedup();
            if let Err(error) = service.rescan_paths(paths).await {
                tracing::error!("timeline watcher incremental rescan failed: {error:#}");
            }
        }
    });
}

fn is_relevant_change(event: &Event) -> bool {
    match &event.kind {
        EventKind::Create(CreateKind::File | CreateKind::Folder) => true,
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_)) => true,
        EventKind::Remove(RemoveKind::File | RemoveKind::Folder) => true,
        EventKind::Modify(ModifyKind::Metadata(_)) => event.paths.iter().any(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|ext| {
                    matches!(
                        ext.to_lowercase().as_str(),
                        "jpg"
                            | "jpeg"
                            | "png"
                            | "webp"
                            | "gif"
                            | "heic"
                            | "heif"
                            | "avif"
                            | "tif"
                            | "tiff"
                    )
                })
                .unwrap_or(false)
        }),
        _ => false,
    }
}

/// Full rescan → diff → update manifest → generate/cleanup thumbnails.
async fn rescan_and_sync(
    photos_path: &std::path::Path,
    data_path: &std::path::Path,
    exclude_regex: &str,
    manifest_lock: &RwLock<Manifest>,
) {
    let new_manifest = match walk::scan(photos_path, exclude_regex) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("watcher rescan failed: {}", e);
            return;
        }
    };

    let old = {
        let m = manifest_lock.read().await;
        m.clone()
    };

    let diff = new_manifest.diff(&old);

    if !diff.has_changes() {
        // Check if any album's updated_at changed (new photos within existing album)
        let has_photo_changes = old.albums.iter().any(|old_a| {
            new_manifest
                .albums
                .iter()
                .any(|new_a| new_a.name == old_a.name && new_a.updated_at != old_a.updated_at)
        });
        if !has_photo_changes {
            return;
        }
        tracing::debug!("photo-level changes detected within existing albums");
    } else {
        tracing::info!(
            "filesystem change: +{} albums, -{} albums",
            diff.new_albums.len(),
            diff.removed_albums.len()
        );
    }

    // Generate thumbnails for new albums
    let regex = match regex::Regex::new(exclude_regex) {
        Ok(r) => r,
        Err(_) => return,
    };

    let mut generated = 0usize;
    for album_name in &diff.new_albums {
        let album_dir = photos_path.join(album_name);
        let photos = walk::scan_album_photos(&album_dir, &regex);
        for photo in &photos {
            let source = album_dir.join(&photo.name);
            let thumb_path = crate::thumbnail::thumb_path(data_path, album_name, &photo.name);
            if !thumb_path.exists() {
                match ThumbnailPool::generate_on_demand(&source, &thumb_path) {
                    Ok(_) => generated += 1,
                    Err(e) => {
                        tracing::warn!("thumbnail failed for {:?}: {}", source, e);
                    }
                }
            }
        }
    }

    // Clean up thumbnails for removed albums
    let mut cleaned = 0usize;
    for album_name in &diff.removed_albums {
        let thumb_dir = data_path.join("thumbs").join(album_name);
        if thumb_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&thumb_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        if std::fs::remove_file(entry.path()).is_ok() {
                            cleaned += 1;
                        }
                    }
                }
            }
            let _ = std::fs::remove_dir(&thumb_dir);
        }
    }

    // Regenerate thumbnails for any photos where source is newer than thumb
    // (handles modified photos within existing albums)
    for album in &new_manifest.albums {
        if diff.new_albums.contains(&album.name) {
            continue; // already handled above
        }

        // Only scan if updated_at changed
        let old_album = old.albums.iter().find(|a| a.name == album.name);
        let needs_refresh = old_album.map_or(true, |oa| oa.updated_at != album.updated_at);

        if needs_refresh {
            let album_dir = photos_path.join(&album.name);
            let photos = walk::scan_album_photos(&album_dir, &regex);
            for photo in &photos {
                let source = album_dir.join(&photo.name);
                let thumb_path = crate::thumbnail::thumb_path(data_path, &album.name, &photo.name);
                let source_mtime = source.metadata().ok().and_then(|m| m.modified().ok());
                let thumb_mtime = thumb_path.metadata().ok().and_then(|m| m.modified().ok());
                let stale = match (&source_mtime, &thumb_mtime) {
                    (Some(s), Some(t)) => s > t,
                    (Some(_), None) => true,
                    _ => false,
                };
                if stale {
                    match ThumbnailPool::generate_on_demand(&source, &thumb_path) {
                        Ok(_) => generated += 1,
                        Err(_) => {}
                    }
                }
            }
        }
    }

    if generated > 0 || cleaned > 0 {
        tracing::info!("auto-rescan: +{} thumbs, -{} thumbs", generated, cleaned);
    }

    // Save updated manifest
    let manifest_path = data_path.join("manifest.json");
    if let Err(e) = new_manifest.save(&manifest_path) {
        tracing::error!("failed to save manifest: {}", e);
    }

    // Update in-memory manifest
    *manifest_lock.write().await = new_manifest;
}

#[cfg(test)]
mod tests {
    use super::should_run_periodic_timeline_rescan;
    use crate::timeline::ScanState;

    #[test]
    fn periodic_timeline_rescan_only_runs_from_ready_state() {
        assert!(!should_run_periodic_timeline_rescan(ScanState::Starting));
        assert!(!should_run_periodic_timeline_rescan(ScanState::Scanning));
        assert!(!should_run_periodic_timeline_rescan(ScanState::Error));
        assert!(should_run_periodic_timeline_rescan(ScanState::Ready));
    }
}
