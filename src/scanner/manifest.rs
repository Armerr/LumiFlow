use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub updated: DateTime<Utc>,
    pub albums: Vec<Album>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub name: String,
    pub cover: String,
    pub count: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoEntry {
    /// Index within album (for prev/next navigation).
    pub id: usize,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AlbumDetail {
    pub name: String,
    pub photo_count: usize,
    pub photos: Vec<PhotoEntry>,
}

/// Diff result: what changed between old and new manifests.
#[derive(Debug)]
pub struct ManifestDiff {
    pub new_albums: Vec<String>,
    pub removed_albums: Vec<String>,
    pub new_photos: HashMap<String, Vec<String>>, // album → new photo filenames
    pub removed_photos: HashMap<String, Vec<String>>, // album → removed photo filenames
}

impl ManifestDiff {
    pub fn has_changes(&self) -> bool {
        !self.new_albums.is_empty()
            || !self.removed_albums.is_empty()
            || !self.new_photos.is_empty()
            || !self.removed_photos.is_empty()
    }
}

impl Manifest {
    pub fn load(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn diff(&self, old: &Manifest) -> ManifestDiff {
        use std::collections::HashSet;

        let old_names: HashSet<&str> = old.albums.iter().map(|a| a.name.as_str()).collect();
        let new_names: HashSet<&str> = self.albums.iter().map(|a| a.name.as_str()).collect();

        let new_albums: Vec<String> = new_names
            .difference(&old_names)
            .map(|s| s.to_string())
            .collect();
        let removed_albums: Vec<String> = old_names
            .difference(&new_names)
            .map(|s| s.to_string())
            .collect();

        let new_photos: HashMap<String, Vec<String>> = HashMap::new();
        let removed_photos: HashMap<String, Vec<String>> = HashMap::new();

        // Only diff albums that exist in both.
        let old_album_map: HashMap<&str, &Album> =
            old.albums.iter().map(|a| (a.name.as_str(), a)).collect();
        let new_album_map: HashMap<&str, &Album> =
            self.albums.iter().map(|a| (a.name.as_str(), a)).collect();

        for name in new_names.intersection(&old_names) {
            let old_a = old_album_map[name];
            let new_a = new_album_map[name];

            if old_a.count == new_a.count && old_a.updated_at == new_a.updated_at {
                continue;
            }

            // We don't have per-photo timestamps in the manifest summary,
            // so we flag this as "needs rescan" — the caller will handle it.
            // For now, this diff is coarse; the detailed diff will happen
            // in the full rescan path (Phase 3).
        }

        ManifestDiff {
            new_albums,
            removed_albums,
            new_photos,
            removed_photos,
        }
    }
}
