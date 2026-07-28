# Timeline AI Albums Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a SQLite-backed timeline mode that recursively indexes photos once, cuts deterministic daily virtual albums, names them from date/place/holiday metadata, optionally tags changed photos with a local CPU ONNX model, and optionally generates cached album descriptions from a contact sheet through an OpenAI-compatible API.

**Architecture:** Keep the current folder-mode manifest path intact. Add a separate timeline service backed by SQLite and selected through `LUMIFLOW_ALBUM_MODE`. The service owns recursive scanning, metadata persistence, daily album rebuilding, stable photo IDs, by-ID media APIs, local vision-tag cache, contact-sheet generation, and album-level AI description jobs. The frontend consumes a mode-neutral richer API and switches to by-ID media URLs when a string photo ID is present.

**Tech Stack:** Rust 2021, Axum, Tokio, rusqlite (bundled SQLite), chrono/chrono-tz, sha1, reqwest/rustls, image/webp, optional ort ONNX Runtime feature, TypeScript/Vite/Vitest, Docker.

---

## File map

**Create:**

- `src/timeline/mod.rs` — timeline service façade and startup/rescan orchestration.
- `src/timeline/models.rs` — database/API models shared by timeline modules.
- `src/timeline/db.rs` — SQLite connection, migrations, repository queries, transaction boundaries.
- `src/timeline/time.rs` — EXIF/filename/mtime timestamp resolution and timezone bucketing.
- `src/timeline/scan.rs` — recursive stat scan, incremental analysis, missing-file marking.
- `src/timeline/albums.rs` — daily grouping, place/holiday naming, album rebuild.
- `src/timeline/holidays.rs` — CN/common Gregorian and Chinese lunar holiday lookup.
- `src/timeline/places.rs` — cached GPS reverse geocoding and path-derived place fallback.
- `src/timeline/vision.rs` — tagger trait, disabled provider, feature-gated ONNX MobileCLIP provider, cached worker.
- `src/timeline/contact_sheet.rs` — deterministic representative sampling and JPEG contact sheets.
- `src/timeline/ai.rs` — OpenAI-compatible album description client, fingerprinting, cache/retry.
- `web/src/shared/api.test.ts` — mode-neutral by-ID URL behavior.
- `web/src/pages/grid/gridPage.test.ts` — description rendering and stable navigation index behavior.

**Modify:**

- `Cargo.toml`, `Cargo.lock` — database, hashing, timezone, HTTP, lunar calendar, optional ONNX dependencies/features.
- `src/main.rs` — register timeline module.
- `src/config.rs` — album mode, timezone, place, vision, and AI settings.
- `src/exif.rs` — expose raw photo metadata needed by timeline scanning without repeating extraction.
- `src/api.rs` — dispatch albums/rescan by mode; add by-ID thumb/EXIF handlers.
- `src/server.rs` — initialize timeline service, register by-ID routes, dispatch original photo by ID.
- `src/thumbnail/mod.rs` — stable by-ID thumbnail path and generation helper.
- `src/scanner/manifest.rs` — serialize richer optional fields so folder mode shares frontend types.
- `src/scanner/watcher.rs` — dispatch timeline rescan when timeline mode is active.
- `web/src/shared/types.ts` — richer album/photo types with string IDs and description metadata.
- `web/src/shared/api.ts` — by-ID URL selection.
- `web/src/shared/router.ts` — route detail photos by stable list index while album identity uses album ID.
- `web/src/pages/fan.ts`, `web/src/pages/fan/FanScene.ts` — navigate by album ID and show description excerpt.
- `web/src/pages/grid.ts`, `web/src/pages/grid/GridScene.ts` — use album ID and photo-aware thumbnail URLs.
- `web/src/pages/detail.ts`, `web/src/pages/detail.test.ts` — use photo-aware EXIF/original/download URLs.
- `docker-compose.example.yml`, `README.md`, `README.zh-CN.md`, `DESIGN.md` — timeline/AI/vision configuration and privacy behavior.
- `Dockerfile` — preserve default image portability; build ONNX support only behind an explicit build arg/feature.

---

### Task 1: Add configuration and dependency boundaries

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/config.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write failing config tests**

Add tests that establish typed modes and defaults:

```rust
#[test]
fn config_defaults_to_folder_mode_with_optional_enrichment_disabled() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let saved = save_env(&[
        "LUMIFLOW_PHOTOS_PATH", "LUMIFLOW_DATA_PATH", "LUMIFLOW_ALBUM_MODE",
        "LUMIFLOW_VISION_TAGGER", "LUMIFLOW_AI_ENABLED",
    ]);
    env::set_var("LUMIFLOW_PHOTOS_PATH", "/tmp/photos");
    env::set_var("LUMIFLOW_DATA_PATH", "/tmp/data");
    env::remove_var("LUMIFLOW_ALBUM_MODE");
    env::remove_var("LUMIFLOW_VISION_TAGGER");
    env::remove_var("LUMIFLOW_AI_ENABLED");

    let config = Config::from_env().expect("config");
    assert_eq!(config.album_mode, AlbumMode::Folders);
    assert_eq!(config.timeline_timezone, "Asia/Shanghai");
    assert_eq!(config.vision_tagger, VisionTagger::None);
    assert!(!config.ai.enabled);

    restore_env_map(saved);
}

#[test]
fn config_parses_timeline_and_ai_settings() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let saved = save_env(&[
        "LUMIFLOW_PHOTOS_PATH", "LUMIFLOW_DATA_PATH", "LUMIFLOW_ALBUM_MODE",
        "LUMIFLOW_VISION_TAGGER", "LUMIFLOW_AI_ENABLED", "LUMIFLOW_AI_BASE_URL",
        "LUMIFLOW_AI_API_KEY", "LUMIFLOW_AI_MODEL",
    ]);
    env::set_var("LUMIFLOW_PHOTOS_PATH", "/tmp/photos");
    env::set_var("LUMIFLOW_DATA_PATH", "/tmp/data");
    env::set_var("LUMIFLOW_ALBUM_MODE", "timeline");
    env::set_var("LUMIFLOW_VISION_TAGGER", "onnx-mobileclip");
    env::set_var("LUMIFLOW_AI_ENABLED", "true");
    env::set_var("LUMIFLOW_AI_BASE_URL", "https://example.invalid/v1");
    env::set_var("LUMIFLOW_AI_API_KEY", "test-key");
    env::set_var("LUMIFLOW_AI_MODEL", "vision-model");

    let config = Config::from_env().expect("config");
    assert_eq!(config.album_mode, AlbumMode::Timeline);
    assert_eq!(config.vision_tagger, VisionTagger::OnnxMobileClip);
    assert!(config.ai.enabled);
    assert_eq!(config.ai.base_url.as_deref(), Some("https://example.invalid/v1"));
    assert_eq!(config.ai.model.as_deref(), Some("vision-model"));

    restore_env_map(saved);
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test config::tests::config_defaults_to_folder_mode_with_optional_enrichment_disabled
```

Expected: compile failure because `AlbumMode`, `VisionTagger`, and the new config fields do not exist.

- [ ] **Step 3: Add typed configuration**

Define:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AlbumMode { Folders, Timeline }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum VisionTagger { None, OnnxMobileClip, OpenVinoMobileClip }

#[derive(Clone, Debug, Serialize)]
pub struct AiConfig {
    pub enabled: bool,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub language: String,
}
```

Extend `Config` with `album_mode`, `timeline_timezone`, `calendar_region`, `place_provider`, `vision_tagger`, `vision_model_path`, `vision_labels_path`, `vision_workers`, and `ai`. Parse strict enum values and return a clear error for unsupported providers. AI may be disabled with empty credentials; enabled AI requires base URL, API key, and model.

- [ ] **Step 4: Add dependencies and features**

Use:

```toml
rusqlite = { version = "0.40", features = ["bundled", "chrono"] }
sha1 = "0.11"
chrono-tz = "0.10"
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls"] }
base64 = "0.22"
lunar-lite = "1.3"
ort = { version = "2.0.0-rc.13", optional = true, default-features = false, features = ["std", "ndarray", "download-binaries", "tls-rustls", "api-27"] }
ndarray = { version = "0.17", optional = true }

[features]
default = []
heic = ["libheif-rs"]
vision-onnx = ["ort", "ndarray"]
```

Register `mod timeline;` in `main.rs`.

- [ ] **Step 5: Run tests and commit**

```bash
cargo test config::tests
cargo fmt --check
git add Cargo.toml Cargo.lock src/config.rs src/main.rs
git commit -m "feat: add timeline enrichment configuration"
```

Expected: config tests pass and formatting is clean.

---

### Task 2: Build the SQLite repository and migrations

**Files:**
- Create: `src/timeline/mod.rs`
- Create: `src/timeline/models.rs`
- Create: `src/timeline/db.rs`

- [ ] **Step 1: Write failing repository tests**

Use a real in-memory SQLite database:

```rust
#[test]
fn migrations_create_timeline_schema() {
    let db = TimelineDb::open_in_memory().expect("db");
    for table in ["photos", "albums", "album_photos", "places", "calendar_events",
                  "photo_vision_tags", "album_ai_descriptions"] {
        assert!(db.has_table(table).expect("schema query"), "missing {table}");
    }
}

#[test]
fn unchanged_photo_upsert_reports_no_analysis_needed() {
    let db = TimelineDb::open_in_memory().expect("db");
    let candidate = PhotoCandidate {
        id: "photo-1".into(),
        relative_path: "nested/a.jpg".into(),
        filename: "a.jpg".into(),
        extension: "jpg".into(),
        size_bytes: 100,
        mtime_ns: 1234,
        fingerprint: "fp-1".into(),
        scan_id: "scan-1".into(),
    };
    assert_eq!(db.upsert_candidate(&candidate).unwrap(), AnalysisDecision::Analyze);
    assert_eq!(db.upsert_candidate(&candidate).unwrap(), AnalysisDecision::Reuse);
}
```

- [ ] **Step 2: Verify RED**

```bash
cargo test timeline::db::tests
```

Expected: compile failure because `TimelineDb` and models do not exist.

- [ ] **Step 3: Implement migrations and repository models**

Create `TimelineDb { path: PathBuf }`. Open one rusqlite connection per repository operation, configure `journal_mode=WAL`, `foreign_keys=ON`, and `busy_timeout=5s`. Run idempotent migration SQL at startup.

Define core structs:

```rust
pub struct PhotoCandidate {
    pub id: String,
    pub relative_path: String,
    pub filename: String,
    pub extension: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub fingerprint: String,
    pub scan_id: String,
}
pub enum AnalysisDecision { Analyze, Reuse }
pub struct PhotoAnalysis {
    pub id: String,
    pub taken_at: Option<String>,
    pub time_source: TimeSource,
    pub timezone: Option<String>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
    pub width: u32,
    pub height: u32,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub exif_json: serde_json::Value,
}
pub struct TimelinePhoto {
    pub id: String,
    pub relative_path: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub format: String,
    pub taken_at: Option<String>,
    pub time_source: TimeSource,
    pub fingerprint: String,
}
pub struct TimelineAlbum {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub date_start: Option<chrono::NaiveDate>,
    pub date_end: Option<chrono::NaiveDate>,
    pub place: Option<String>,
    pub holiday: Option<String>,
    pub photo_count: usize,
    pub cover_photo_id: Option<String>,
}
pub struct DailyAlbumBuild { pub album: TimelineAlbum, pub photo_ids: Vec<String> }
```

Repository methods required by later tasks:

```rust
pub fn upsert_candidate(&self, candidate: &PhotoCandidate) -> Result<AnalysisDecision>;
pub fn save_analysis(&self, analysis: &PhotoAnalysis) -> Result<()>;
pub fn mark_missing_except(&self, scan_id: &str) -> Result<usize>;
pub fn replace_daily_albums(&self, albums: &[DailyAlbumBuild]) -> Result<()>;
pub fn list_albums(&self) -> Result<Vec<TimelineAlbum>>;
pub fn get_album(&self, id: &str) -> Result<Option<TimelineAlbumDetail>>;
pub fn get_photo(&self, id: &str) -> Result<Option<TimelinePhoto>>;
```

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test timeline::db::tests
cargo fmt --check
git add src/timeline

git commit -m "feat: add timeline sqlite repository"
```

---

### Task 3: Resolve photo time without repeated analysis

**Files:**
- Create: `src/timeline/time.rs`
- Modify: `src/exif.rs`

- [ ] **Step 1: Write failing precedence and timezone tests**

```rust
#[test]
fn exif_timestamp_beats_filename_and_mtime() {
    let input = TimeInput {
        exif_datetime: Some("2024:02:10 09:13:00".into()),
        exif_offset: Some("+08:00".into()),
        filename: "IMG_20240101_120000.jpg".into(),
        mtime: utc("2023-12-01T00:00:00Z"),
    };
    let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();
    assert_eq!(resolved.source, TimeSource::Exif);
    assert_eq!(resolved.timestamp.to_rfc3339(), "2024-02-10T09:13:00+08:00");
}

#[test]
fn filename_timestamp_beats_mtime() {
    let input = TimeInput {
        exif_datetime: None,
        exif_offset: None,
        filename: "IMG_20240102_153012.jpg".into(),
        mtime: utc("2023-12-01T00:00:00Z"),
    };
    let resolved = resolve_taken_at(&input, chrono_tz::Asia::Shanghai).unwrap();
    assert_eq!(resolved.source, TimeSource::Filename);
    assert_eq!(resolved.timestamp.to_rfc3339(), "2024-01-02T15:30:12+08:00");
}

#[test]
fn local_day_uses_configured_timezone() {
    let timestamp = DateTime::parse_from_rfc3339("2024-02-10T17:30:00Z").unwrap();
    assert_eq!(local_day(timestamp, chrono_tz::Asia::Shanghai).to_string(), "2024-02-11");
}
```

- [ ] **Step 2: Verify RED**

```bash
cargo test timeline::time::tests
```

Expected: module/functions missing.

- [ ] **Step 3: Implement parser and expose EXIF primitives**

Implement common filename patterns with anchored regexes. Do not parse arbitrary digit runs. Parse EXIF local datetime with optional offset, then fallback timezone, then filename, then mtime. Return:

```rust
pub struct ResolvedTime { pub timestamp: DateTime<FixedOffset>, pub source: TimeSource }
```

Refactor `exif.rs` to expose an internal `ExtractedPhotoMetadata`/existing `ExifData` value once per changed file so timeline scanning stores it and by-ID EXIF reads it from SQLite rather than reopening the original.

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test timeline::time::tests
cargo test exif::tests
cargo fmt --check
git add src/timeline/time.rs src/exif.rs
git commit -m "feat: resolve photo capture times deterministically"
```

---

### Task 4: Recursively scan and persist changed photos

**Files:**
- Create: `src/timeline/scan.rs`
- Modify: `src/timeline/mod.rs`
- Modify: `src/thumbnail/mod.rs`

- [ ] **Step 1: Write failing recursive/incremental tests**

Create temporary directories under `std::env::temp_dir()` with unique test names and cleanup guards. Copy tiny fixture bytes generated by `image` into nested directories.

```rust
#[test]
fn recursive_scan_finds_nested_photos_and_skips_excluded_paths() {
    let root = TestTree::new()
        .image("one/a.jpg")
        .image("two/deep/b.png")
        .image("@eaDir/ignored.jpg");
    let result = scan_candidates(root.path(), DEFAULT_EXCLUDE).unwrap();
    assert_eq!(relative_paths(&result), ["one/a.jpg", "two/deep/b.png"]);
}

#[test]
fn second_scan_reuses_unchanged_analysis() {
    let analyzer = CountingAnalyzer::default();
    service.scan_with(&analyzer).unwrap();
    service.scan_with(&analyzer).unwrap();
    assert_eq!(analyzer.calls(), 1);
}
```

- [ ] **Step 2: Verify RED**

```bash
cargo test timeline::scan::tests
```

- [ ] **Step 3: Implement scanning**

Use `WalkDir::new(root).follow_links(false)`, `filter_entry`, canonical root containment, and supported-extension filtering. Stable photo ID is lower-hex SHA-1 of normalized UTF-8 relative path. Fingerprint is SHA-1 of `relative_path + NUL + size + NUL + mtime_ns`.

Changed-photo analysis stores EXIF/time/GPS/dimensions. Missing files are marked after a completed scan. A failed/incomplete walk must not mark rows missing.

Add:

```rust
pub fn timeline_thumb_path(data_path: &Path, photo_id: &str) -> PathBuf {
    data_path.join("thumbs").join("by-id").join(format!("{photo_id}.webp"))
}
```

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test timeline::scan::tests
cargo fmt --check
git add src/timeline/scan.rs src/timeline/mod.rs src/thumbnail/mod.rs
git commit -m "feat: recursively index timeline photos"
```

---

### Task 5: Build daily albums and deterministic names

**Files:**
- Create: `src/timeline/albums.rs`
- Create: `src/timeline/holidays.rs`
- Create: `src/timeline/places.rs`
- Modify: `src/timeline/db.rs`

- [ ] **Step 1: Write failing grouping/naming tests**

```rust
#[test]
fn cuts_albums_at_local_midnight() {
    let photos = vec![photo("a", "2024-02-10T23:59:00+08:00"), photo("b", "2024-02-11T00:01:00+08:00")];
    let albums = build_daily_albums(&photos, chrono_tz::Asia::Shanghai, &NoPlaces, &CnCommonCalendar);
    assert_eq!(album_ids(&albums), ["auto-day:2024-02-10", "auto-day:2024-02-11"]);
}

#[test]
fn names_album_from_date_place_and_holiday() {
    assert_eq!(format_album_name(day(2024, 2, 10), Some("上海"), Some("春节")), "2024-02-10 上海 · 春节");
}

#[test]
fn cn_common_calendar_contains_lunar_and_gregorian_holidays() {
    assert_eq!(holiday_for(day(2024, 2, 10)), Some("春节"));
    assert_eq!(holiday_for(day(2024, 12, 25)), Some("Christmas"));
}
```

- [ ] **Step 2: Verify RED**

```bash
cargo test timeline::albums::tests timeline::holidays::tests
```

- [ ] **Step 3: Implement daily album rebuild**

Group active photos by configured local date; unknown time goes to `unknown-date`. Sort by taken time then relative path. Album ID is deterministic. Cover is the median chronological photo to avoid always choosing setup shots.

Implement CN common calendar with fixed Gregorian holidays and `lunar-lite` conversion for 春节、元宵、端午、七夕、中秋、重阳. Keep public-holiday ranges data-driven in a static year table where legal holiday dates differ by year.

Implement `PlaceResolver`:

```rust
pub trait PlaceResolver { fn resolve_album_place(&self, photos: &[TimelinePhoto]) -> Result<Option<String>>; }
```

First try cached rounded GPS bucket. If absent and provider URL is configured, call Nominatim-compatible reverse geocoding with a project user agent and store the result. If GPS is absent/fails, tokenize relative-path components against a conservative place dictionary. Network failure leaves place empty.

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test timeline::albums::tests timeline::holidays::tests timeline::places::tests
cargo fmt --check
git add src/timeline/albums.rs src/timeline/holidays.rs src/timeline/places.rs src/timeline/db.rs
git commit -m "feat: build named daily timeline albums"
```

---

### Task 6: Initialize timeline mode and dispatch rescans

**Files:**
- Modify: `src/timeline/mod.rs`
- Modify: `src/server.rs`
- Modify: `src/api.rs`
- Modify: `src/scanner/watcher.rs`

- [ ] **Step 1: Write failing mode-dispatch tests**

```rust
#[tokio::test]
async fn timeline_mode_lists_sqlite_albums() {
    let state = timeline_test_state_with_album("auto-day:2024-02-10");
    let response = build_router(state).oneshot(request("/api/albums")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_body_contains(response, "auto-day:2024-02-10").await;
}
```

Also test `POST /api/rescan` calls timeline rescan and returns counts.

- [ ] **Step 2: Verify RED**

```bash
cargo test server::tests::timeline_mode_lists_sqlite_albums
```

- [ ] **Step 3: Implement mode-neutral app state**

Add `timeline: Option<Arc<TimelineService>>` to `AppState`. In `serve`, folder mode follows existing manifest/watcher logic; timeline mode opens/migrates SQLite, performs the first scan/rebuild before binding, starts a debounced watcher that calls `TimelineService::rescan`, and skips folder thumbnail pre-generation.

Change album handlers to dispatch by mode and return a shared JSON shape. Rescan must run blocking scan/SQLite work through `tokio::task::spawn_blocking` and serialize concurrent scans with a mutex.

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test server::tests api::tests timeline::tests
cargo fmt --check
git add src/timeline/mod.rs src/server.rs src/api.rs src/scanner/watcher.rs
git commit -m "feat: serve sqlite timeline albums"
```

---

### Task 7: Add stable by-ID media APIs

**Files:**
- Modify: `src/api.rs`
- Modify: `src/server.rs`
- Modify: `src/thumbnail/mod.rs`

- [ ] **Step 1: Write failing router tests**

```rust
#[tokio::test]
async fn by_id_original_serves_nested_file_and_rejects_unknown_id() {
    let state = timeline_state_with_photo("p1", "nested/a.jpg");
    assert_eq!(status(build_router(state.clone()), "/api/photos/by-id/p1").await, StatusCode::OK);
    assert_eq!(status(build_router(state), "/api/photos/by-id/missing").await, StatusCode::NOT_FOUND);
}
```

Cover Range and `?download=1`, by-ID thumbnail generation/cache, and by-ID EXIF JSON from stored metadata.

- [ ] **Step 2: Verify RED**

```bash
cargo test server::tests::by_id api::tests::by_id
```

- [ ] **Step 3: Register and implement routes**

```text
GET /api/photos/by-id/{photo_id}
GET /api/thumbs/by-id/{photo_id}
GET /api/exif/by-id/{photo_id}
```

Look up relative paths only through SQLite. Canonicalize and enforce containment beneath the photo root before opening a file. Reuse existing Range/ETag/download response code. Timeline thumbnail cache uses photo ID; freshness uses stored fingerprint and regenerates on mismatch.

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test server::tests api::tests
cargo fmt --check
git add src/api.rs src/server.rs src/thumbnail/mod.rs
git commit -m "feat: serve timeline photos by stable id"
```

---

### Task 8: Make the frontend mode-neutral

**Files:**
- Create: `web/src/shared/api.test.ts`
- Create: `web/src/pages/grid/gridPage.test.ts`
- Modify: `web/src/shared/types.ts`
- Modify: `web/src/shared/api.ts`
- Modify: `web/src/pages/fan.ts`
- Modify: `web/src/pages/fan/FanScene.ts`
- Modify: `web/src/pages/grid.ts`
- Modify: `web/src/pages/grid/GridScene.ts`
- Modify: `web/src/pages/detail.ts`
- Modify: `web/src/pages/detail.test.ts`

- [ ] **Step 1: Write failing frontend tests**

```ts
test('uses by-id URLs for timeline photos and legacy URLs for folder photos', () => {
  expect(api.thumbUrl({ id: 'abc', name: 'same.jpg' }, 'album')).toBe('/api/thumbs/by-id/abc')
  expect(api.photoUrl({ id: 4, name: 'same.jpg' }, 'album')).toBe('/api/photos/album/same.jpg')
})

test('renders deterministic album name and AI description', () => {
  expect(renderGridHeader({ id: 'auto-day:2024-02-10', name: '2024-02-10 上海 · 春节', description: '家庭聚会。', photos: [] }))
    .toContain('家庭聚会。')
})
```

- [ ] **Step 2: Verify RED**

```bash
cd web && npm test -- src/shared/api.test.ts src/pages/grid/gridPage.test.ts
```

- [ ] **Step 3: Implement richer types and URLs**

`Album` gets `id`, `description`, date/place/holiday, `cover_photo_id`, while preserving legacy fields. `Photo.id` becomes `number | string` and gains optional `relative_path`, `taken_at`, `time_source`.

Change API helpers to accept `Photo`:

```ts
thumbUrl(albumId: string, photo: Photo): string
photoUrl(albumId: string, photo: Photo): string
exif(albumId: string, photo: Photo): Promise<ExifData>
```

String IDs use by-ID routes. Number IDs use legacy album/file routes. Keep detail navigation based on list index, not photo ID. Navigate albums with `album.id ?? album.name`.

- [ ] **Step 4: Verify GREEN, build, and commit**

```bash
cd web && npm test && npm run build
cd ..
git add web/src
git commit -m "feat: browse folder and timeline albums in web ui"
```

---

### Task 9: Add cached local CPU vision tagging

**Files:**
- Create: `src/timeline/vision.rs`
- Modify: `src/timeline/db.rs`
- Modify: `src/timeline/mod.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Write failing provider-independent cache tests**

```rust
#[test]
fn unchanged_vision_input_reuses_cached_tags() {
    let db = TimelineDb::open_in_memory().unwrap();
    let tagger = CountingTagger::returns(vec![tag("family", 0.91)]);
    tag_photo(&db, &tagger, photo("p1", "fp1"), thumb("tfp1")).unwrap();
    tag_photo(&db, &tagger, photo("p1", "fp1"), thumb("tfp1")).unwrap();
    assert_eq!(tagger.calls(), 1);
}

#[test]
fn changed_model_or_thumbnail_invalidates_tags() {
    let db = TimelineDb::open_in_memory().unwrap();
    let first = CountingTagger::new("model-v1", vec![tag("family", 0.91)]);
    tag_photo(&db, &first, photo("p1", "fp1"), thumb("tfp1")).unwrap();
    tag_photo(&db, &first, photo("p1", "fp1"), thumb("tfp2")).unwrap();
    assert_eq!(first.calls(), 2);

    let second = CountingTagger::new("model-v2", vec![tag("family", 0.91)]);
    tag_photo(&db, &second, photo("p1", "fp1"), thumb("tfp2")).unwrap();
    assert_eq!(second.calls(), 1);
}
```

- [ ] **Step 2: Verify RED**

```bash
cargo test timeline::vision::tests
```

- [ ] **Step 3: Implement tagger trait and cache worker**

```rust
pub trait VisionTagger: Send + Sync {
    fn model_id(&self) -> &str;
    fn tag(&self, rgb_224: &[f32]) -> anyhow::Result<Vec<VisionTag>>;
}
```

`NoneTagger` returns no work. `OnnxMobileClipTagger` is behind `vision-onnx`, loads `LUMIFLOW_VISION_MODEL_PATH` and a JSON labels/text-embedding asset, preprocesses a 224×224 thumbnail, normalizes embeddings, computes cosine scores, and returns at most five labels above a configured threshold. It must not download a model implicitly. Missing model assets with the provider enabled is a startup error; disabled vision has no runtime/model dependency.

Run a bounded number of blocking workers after thumbnails exist. Cache by photo fingerprint + thumbnail fingerprint + model ID + tagset version.

`openvino-mobileclip` returns a clear unsupported-provider startup error in this release; it remains a documented future optimization and is never silently mapped to ONNX.

- [ ] **Step 4: Verify GREEN with and without feature**

```bash
cargo test timeline::vision::tests
cargo check --features vision-onnx
cargo fmt --check
git add Cargo.toml Cargo.lock src/timeline/vision.rs src/timeline/db.rs src/timeline/mod.rs
git commit -m "feat: cache local cpu vision tags"
```

---

### Task 10: Generate deterministic contact sheets

**Files:**
- Create: `src/timeline/contact_sheet.rs`
- Modify: `src/timeline/mod.rs`

- [ ] **Step 1: Write failing sampling/render tests**

```rust
#[test]
fn samples_large_album_across_full_time_range() {
    let selected = representative_indices(100, 36);
    assert_eq!(selected.len(), 36);
    assert_eq!(selected[0], 0);
    assert_eq!(*selected.last().unwrap(), 99);
    assert!(selected.windows(2).all(|w| w[0] < w[1]));
}
```

Decode the generated JPEG and assert its dimensions/cell count for a four-photo fixture.

- [ ] **Step 2: Verify RED**

```bash
cargo test timeline::contact_sheet::tests
```

- [ ] **Step 3: Implement contact sheets**

Select at most 36 chronological photos with an endpoint-preserving even sampler. Decode existing WebP thumbnails, letterbox cells without cropping, render a 6-column JPEG grid with no filenames/path text, and write atomically under `data/ai/contact-sheets/<safe album hash>.jpg`.

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test timeline::contact_sheet::tests
cargo fmt --check
git add src/timeline/contact_sheet.rs src/timeline/mod.rs
git commit -m "feat: generate album contact sheets"
```

---

### Task 11: Generate and cache album AI descriptions

**Files:**
- Create: `src/timeline/ai.rs`
- Modify: `src/timeline/db.rs`
- Modify: `src/timeline/mod.rs`

- [ ] **Step 1: Read current OpenAI Responses/vision documentation**

Use the OpenAI developer-docs tools before coding the request schema. If the configured endpoint is explicitly OpenAI, use the documented Responses API image input. For generic OpenAI-compatible providers, support the widely implemented Chat Completions multimodal schema behind a provider setting. Do not guess request fields.

- [ ] **Step 2: Write failing fingerprint/parser tests**

```rust
#[test]
fn description_fingerprint_changes_with_vision_tags() {
    let a = description_fingerprint(&input_with_tags(["family"]));
    let b = description_fingerprint(&input_with_tags(["street"]));
    assert_ne!(a, b);
}

#[test]
fn validates_structured_description_output() {
    let parsed = parse_description(r#"{"description":"家庭聚会。","keywords":["家庭"],"confidence":0.8}"#).unwrap();
    assert_eq!(parsed.keywords, vec!["家庭"]);
}
```

Add an HTTP-level test using a one-shot local TCP server that captures the request and returns deterministic JSON; do not mock the client method itself.

- [ ] **Step 3: Verify RED**

```bash
cargo test timeline::ai::tests
```

- [ ] **Step 4: Implement non-blocking AI worker**

Build prompt metadata from deterministic album name, date/place/holiday, photo count, time range, camera summary, and aggregated local tags. Send the contact sheet as base64 image input. Enforce timeout, status checking, response size limit, strict JSON validation, description/keyword length limits, confidence clamping, and no title override.

Cache by prompt version + album metadata + selected photo fingerprints + local tag signatures. AI errors are stored and logged; album browsing remains available. Do not retry in a tight loop—retry on a future rescan or explicit rescan.

- [ ] **Step 5: Verify GREEN and commit**

```bash
cargo test timeline::ai::tests
cargo fmt --check
git add src/timeline/ai.rs src/timeline/db.rs src/timeline/mod.rs
git commit -m "feat: generate cached album ai descriptions"
```

---

### Task 12: Document and configure the feature

**Files:**
- Modify: `docker-compose.example.yml`
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Modify: `DESIGN.md`
- Modify: `Dockerfile`

- [ ] **Step 1: Update Compose with safe defaults**

Add directly under `environment`:

```yaml
LUMIFLOW_ALBUM_MODE: timeline
LUMIFLOW_TIMELINE_TIMEZONE: Asia/Shanghai
LUMIFLOW_CALENDAR_REGION: CN_COMMON
LUMIFLOW_VISION_TAGGER: none
LUMIFLOW_AI_ENABLED: "false"
```

Do not add `.env` placeholders or credentials. Document opt-in snippets for vision model mounts and AI credentials, with a warning that contact sheets leave the host only when AI is enabled.

- [ ] **Step 2: Update Docker build behavior**

Default multi-arch image stays free of ONNX native runtime and builds with default features. Document/build an opt-in `vision-onnx` image variant or build arg; never make the default image architecture-dependent.

- [ ] **Step 3: Validate docs/config**

```bash
docker compose -f docker-compose.example.yml config --format json
cargo test
cd web && npm test && npm run build
```

- [ ] **Step 4: Commit**

```bash
git add docker-compose.example.yml README.md README.zh-CN.md DESIGN.md Dockerfile
git commit -m "docs: explain timeline albums and optional enrichment"
```

---

### Task 13: End-to-end verification

**Files:**
- No new files unless a genuine regression requires one.

- [ ] **Step 1: Create a nested smoke library**

Use the ignored `.lumiflow-demo` tree with photos in multiple nested directories, duplicate filenames in different directories, two local dates, and one file with no EXIF date.

- [ ] **Step 2: Run the full verification suite**

```bash
cargo fmt --check
cargo test
cargo check --features vision-onnx
cd web && npm test && npm run build
cd ..
docker compose -f docker-compose.example.yml config --format json
```

Expected: all commands exit zero.

- [ ] **Step 3: Smoke timeline mode**

Start LumiFlow with:

```text
LUMIFLOW_ALBUM_MODE=timeline
LUMIFLOW_VISION_TAGGER=none
LUMIFLOW_AI_ENABLED=false
```

Verify through real HTTP/browser behavior:

- `/api/albums` returns daily album IDs and deterministic names.
- Nested duplicate filenames have distinct string IDs.
- `/api/photos/by-id/<id>` serves each correct original.
- `/api/thumbs/by-id/<id>` generates and serves WebP.
- `/api/exif/by-id/<id>` returns stored metadata.
- A second rescan leaves `exif_analyzed_at` unchanged for unchanged files.
- The fan, grid, and detail pages render and navigate successfully in the browser.

- [ ] **Step 4: Smoke optional enrichment failure isolation**

Enable AI with an intentionally unreachable local endpoint and verify albums still load while the description error is recorded. Enable ONNX vision without model assets and verify startup fails with an actionable model-path error rather than silently skipping.

- [ ] **Step 5: Review final changes and finish branch**

Use `superpowers:verification-before-completion`, then `superpowers:finishing-a-development-branch`. Do not claim Docker Hub was updated unless a new image was actually built, pushed, and inspected.
