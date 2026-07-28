# Timeline AI Albums Design

## Goal

Add a timeline album mode for messy photo backups. LumiFlow will recursively scan a read-only photo root, persist photo metadata in SQLite, build deterministic daily virtual albums, name those albums from date/place/holiday metadata, and optionally generate an AI description/keyword summary for each completed album.

The feature must not copy, move, rename, hard-link, or symlink original photos.

## Non-goals

- AI must not decide album membership.
- AI must not be required for scanning, grouping, thumbnail generation, or photo serving.
- Original file deduplication by full content hash is out of scope for the first version.
- Manual album editing UI is out of scope for the first version.
- Multi-day trip/event merging is out of scope for the first version.

## Current behavior

LumiFlow currently treats every first-level directory under `LUMIFLOW_PHOTOS_PATH` as an album. Photo APIs use `album + filename`, and thumbnails are cached by album/file path. This breaks down for messy backup folders because photos may be deeply nested, filenames may repeat, and meaningful albums may not correspond to directories.

## User-facing model

LumiFlow will support two album modes:

```text
LUMIFLOW_ALBUM_MODE=folders | timeline
```

- `folders`: current behavior. First-level directories are albums.
- `timeline`: recursive scan, no reliance on folder boundaries, daily virtual albums.

The initial default stays `folders` for compatibility. Users opt into automatic organization with `timeline`.

## Data storage

Add SQLite under the data directory:

```text
LUMIFLOW_DATA_PATH/lumiflow.sqlite
LUMIFLOW_DATA_PATH/thumbs/<photo_id>.webp
LUMIFLOW_DATA_PATH/ai/contact-sheets/<album_id>.jpg
```

SQLite is the source of truth for analyzed photo metadata and virtual album membership in `timeline` mode. API responses in timeline mode are queried from SQLite; the existing JSON manifest remains a folder-mode compatibility artifact until that path is migrated.

## Schema

### photos

One row per original photo.

```text
id                  stable photo id, sha1(relative_path)
relative_path       path relative to LUMIFLOW_PHOTOS_PATH
filename            basename for display
extension           normalized extension
size_bytes          file size from stat
mtime_ns            file mtime with nanosecond precision when available
fingerprint         derived from size_bytes + mtime_ns
status              active | missing | unsupported | error
taken_at            best-known timestamp
time_source         exif | filename | mtime | unknown
timezone            parsed EXIF timezone or configured fallback
gps_lat             nullable
gps_lon             nullable
width               nullable until decoded
height              nullable until decoded
camera_make         nullable
camera_model        nullable
lens                nullable
exif_analyzed_at    nullable
created_at          row creation time
updated_at          row update time
```

### albums

One row per virtual album.

```text
id                  stable album id, e.g. auto-day:2024-02-10
type                folder | auto_day | unknown_date
date_start          nullable for unknown-date albums
date_end            nullable for unknown-date albums
display_name        deterministic name shown to users
place_name          nullable
holiday_name        nullable
photo_count         denormalized count
cover_photo_id      nullable
created_at
updated_at
```

### album_photos

```text
album_id
photo_id
sort_order
```

Unique `(album_id, photo_id)`. Sort order is by `taken_at`, then `relative_path`.

### places

Cache reverse geocoding and path-derived location results.

```text
geo_bucket          rounded GPS bucket, e.g. 31.230,121.474
lat
lon
country
region
city
district
provider
resolved_at
```

### calendar_events

Built-in and migrated calendar data.

```text
date
region              CN_COMMON for first version
name
kind                chinese_traditional | china_public | gregorian | custom
```

### album_ai_descriptions

AI output cache. AI descriptions never override `albums.display_name`.

```text
album_id
input_fingerprint   hash of album metadata + selected photo fingerprints + prompt version
model
description
keywords_json
confidence
generated_at
error               nullable last error
```

## Incremental scanning

Timeline scanning walks the entire photo root recursively with existing exclude rules.

For each supported file:

1. Compute `relative_path`, `size_bytes`, `mtime_ns`, and `fingerprint` using file metadata only.
2. Look up the existing `photos` row by `id = sha1(relative_path)`.
3. If `fingerprint` is unchanged, skip EXIF, filename parsing, thumbnail invalidation, and AI invalidation.
4. If new or changed, analyze metadata and update the row.
5. Mark rows that were not seen in the scan as `missing` instead of deleting immediately.

This avoids repeated photo analysis while keeping the original directory read-only.

## Time extraction

Resolve `taken_at` in priority order:

1. EXIF `DateTimeOriginal` + `OffsetTimeOriginal`.
2. EXIF `DateTimeOriginal` with configured fallback timezone.
3. Filename timestamp patterns, including common camera, mobile, screenshot, and chat-export names.
4. File modification time.
5. Unknown.

Persist `time_source` so the UI can distinguish reliable EXIF dates from mtime fallbacks.

## Album cutting

First version uses deterministic natural-day albums.

Rules:

- Convert `taken_at` to the configured local timezone for date bucketing.
- All photos on the same local date go into `auto-day:<YYYY-MM-DD>`.
- Photos with unknown time go into `unknown-date`.
- Album membership is fully reproducible from database metadata.
- AI output never changes membership.

This matches the current requirement: scan first, cut albums by time, then enrich the already-created albums.

## Album naming

Use deterministic naming from date, place, and holiday.

Template:

```text
YYYY-MM-DD [place] [· holiday]
```

Examples:

```text
2024-02-10 上海 · 春节
2024-04-05 杭州 · 清明节
2024-10-03 京都 · 国庆假期
2024-12-25 东京 · Christmas
2025-01-01 元旦
2024-08-18
```

Place priority:

1. Reverse-geocoded GPS place from the album's median or dominant GPS bucket.
2. Path-derived place tokens.
3. Empty.

Holiday strategy for the first version is fixed `CN_COMMON`: Chinese traditional/public holidays plus common Gregorian holidays, regardless of GPS country. This matches the selected product behavior.

If multiple places appear in one day, use at most two names plus a count suffix:

```text
2024-02-10 上海 · 苏州 +1 · 春节
```

## AI responsibility boundary

AI is post-processing only.

AI does not:

- scan photos;
- decide whether a file is a photo;
- extract EXIF/GPS;
- cut album boundaries;
- decide album membership;
- serve images;
- generate thumbnails.

AI does:

- inspect a low-resolution contact sheet for an existing album;
- combine visual cues with date/place/holiday/photo-count metadata;
- generate a description, keywords, and confidence.

If AI fails, albums remain usable with deterministic names and empty or stale descriptions.

## Contact sheet generation

For each album requiring AI description, generate:

```text
LUMIFLOW_DATA_PATH/ai/contact-sheets/<album_id>.jpg
```

Rules:

- Use generated thumbnails, not originals.
- Select up to 36 representative photos.
- Preserve chronological order.
- For large albums, sample from beginning, middle, and end.
- Use low resolution, approximately 160-240 px per cell.
- Include no sensitive raw path text in the image itself.

The contact sheet is the only image sent to the AI provider.

## AI prompt and output

Input metadata:

```json
{
  "album_name": "2024-02-10 上海 · 春节",
  "date": "2024-02-10",
  "place": "上海",
  "holiday": "春节",
  "photo_count": 184,
  "time_range": "09:13-22:41",
  "camera_summary": ["iPhone 15 Pro", "X2D 100C"],
  "language": "zh-CN"
}
```

Required model output:

```json
{
  "description": "这组照片记录了春节当天的家庭聚会、城市街景和夜间灯光氛围，包含餐桌、街头与节日装饰等场景。",
  "keywords": ["春节", "家庭", "上海", "夜景", "街拍"],
  "confidence": 0.82
}
```

Validate JSON before storing. On invalid output, keep the deterministic album name and record the error for retry.

## AI invalidation

Compute `input_fingerprint` from:

- prompt version;
- album id;
- deterministic display name;
- date/place/holiday fields;
- selected photo ids;
- selected photo fingerprints.

If the fingerprint is unchanged, reuse the existing AI description. If it changes, enqueue regeneration.

## API changes

Timeline mode needs stable photo IDs because filenames can repeat across nested directories.

Add:

```text
GET /api/photos/by-id/:photo_id
GET /api/thumbs/by-id/:photo_id
GET /api/exif/by-id/:photo_id
```

Album responses include richer metadata:

```json
{
  "id": "auto-day:2024-02-10",
  "name": "2024-02-10 上海 · 春节",
  "description": "这组照片记录了春节当天...",
  "date_start": "2024-02-10",
  "date_end": "2024-02-10",
  "place": "上海",
  "holiday": "春节",
  "photo_count": 184,
  "cover_photo_id": "..."
}
```

Photo entries include IDs and relative paths:

```json
{
  "id": "...",
  "name": "IMG_0001.jpg",
  "relative_path": "DCIM/100APPLE/IMG_0001.jpg",
  "taken_at": "2024-02-10T09:13:00+08:00",
  "time_source": "exif",
  "size_bytes": 4812345,
  "format": "JPG"
}
```

Existing folder-mode endpoints stay for compatibility. The frontend should prefer by-id URLs when `id` is present.

## Configuration

Add runtime variables:

```text
LUMIFLOW_ALBUM_MODE=folders
LUMIFLOW_TIMELINE_TIMEZONE=Asia/Shanghai
LUMIFLOW_CALENDAR_REGION=CN_COMMON
LUMIFLOW_AI_ENABLED=false
LUMIFLOW_AI_PROVIDER=openai-compatible
LUMIFLOW_AI_BASE_URL=
LUMIFLOW_AI_API_KEY=
LUMIFLOW_AI_MODEL=
LUMIFLOW_AI_DESCRIPTION_LANGUAGE=zh-CN
```

Compose documentation should keep AI disabled by default. Users who opt into AI add provider settings directly to `docker-compose.yml`.

## Frontend behavior

- Album cards show the deterministic name from date/place/holiday.
- If AI description exists, show a short excerpt under the deterministic album name.
- If AI is enabled and an album is queued, show a lightweight `Description pending` state only where helpful.
- Album grid uses by-id thumbnail/photo URLs in timeline mode.
- Detail page uses by-id photo and EXIF URLs in timeline mode.

## Error handling

- Database unavailable: startup fails with a clear error because timeline mode depends on it.
- EXIF parse failure: store available file metadata and continue.
- Missing or invalid time: assign `time_source=unknown` and group into `Unknown Date`.
- Reverse geocode failure: leave place empty and retry later.
- AI failure: keep deterministic album name, store error, retry later.
- Thumbnail failure: keep original photo serving available.

## Testing strategy

Behavior tests should cover:

- Recursive scanner finds nested photos and ignores excluded directories.
- Unchanged fingerprint skips EXIF reanalysis.
- Changed size/mtime triggers reanalysis.
- EXIF time beats filename time, filename time beats mtime.
- Natural-day cutting groups photos by local date.
- Unknown-time photos go to `Unknown Date`.
- Holiday naming adds CN/common holidays.
- Album membership is unchanged when AI description changes.
- Contact sheet generation samples large albums deterministically.
- AI output is cached by input fingerprint.
- by-id photo/thumb/exif APIs serve the expected original file.

## Rollout plan

1. Introduce SQLite schema and repository layer.
2. Add recursive timeline scanner and incremental metadata analysis.
3. Generate daily virtual albums and by-id APIs.
4. Update frontend to consume by-id photo entries.
5. Add holiday naming and path/GPS place naming.
6. Add contact sheet generation and AI description worker.
7. Add documentation and Compose examples with AI disabled by default.

## Acceptance criteria

- A messy nested photo root can be scanned without changing any original file.
- Re-running scan on unchanged files does not repeat EXIF analysis.
- Timeline mode creates daily virtual albums from recursive files.
- Album names include date and optionally place/holiday.
- AI descriptions are generated only after albums are created.
- AI failures do not prevent album browsing.
- Frontend can browse and open photos with duplicate filenames in different directories.
