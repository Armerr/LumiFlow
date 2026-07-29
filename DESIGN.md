# LumiFlow — Architecture & Design v2

## 1. Overview

LumiFlow is a self-hosted photo album web app deployed as a Docker container on a NAS.
It reads a mounted photo directory read-only and supports two backend album modes: first-level folder albums and SQLite-backed timeline albums generated deterministically from recursive photo metadata. Both modes render the same three WebGL-powered views:

| Page | Route | Description |
|------|-------|-------------|
| Home — 扇面 | `/` | Chinese folding-fan layout of album covers. Infinite circular scroll, WebGL. |
| Album — 相册 | `/album/<name>` | Infinite scrollable + draggable photo grid with WebGL overlay. |
| Photo — 详情 | `/album/<name>/photo/<id>` | Full-size photo + sidebar with EXIF metadata. Prev/next navigation. |

**Non-negotiable constraints:**
- Original photo directory is **read-only** — never modified.
- All generated artifacts (thumbnails, manifest, cache) live under `LUMIFLOW_DATA_PATH`.
- Albums sorted by **directory creation time** (newest first).
- **No authentication** — pure public display.
- **Auto-detect** new/deleted photos via file watcher + periodic fallback scan.

## 2. Tech Stack

### Backend: Rust

| Layer | Crate | Rationale |
|-------|-------|-----------|
| HTTP framework | `axum` | Ergonomic, tower-based, first-class async. |
| Async runtime | `tokio` | De facto standard, required by axum. |
| Static serving | `tower-http` | `ServeDir` + `ServeFile` for embedded frontend + photos. |
| Image decode (JPEG/PNG/WebP/GIF) | `image` | Pure Rust, no system deps. |
| HEIC decode | `libheif-rs` | Safe Rust bindings to libheif. System lib required at runtime. |
| WebP encode | `webp` | Pure Rust WebP encoder with quality control. |
| EXIF extraction | `kamadak-exif` | Pure Rust, supports JPEG + HEIC/HEIF. |
| JSON | `serde` + `serde_json` | Standard. |
| File watching | `notify` | Cross-platform, recursive, debounced. |
| Directory walk | `walkdir` | Recursive, filtered, fast. |
| Frontend embed | `rust-embed` | Embeds Vite output at compile time. |
| Config | `std::env` + `serde` | Env-var-driven runtime config. |
| Logging | `tracing` + `tracing-subscriber` | Structured, filterable. |
| Thumbnail worker pool | `tokio::task` | Bounded concurrent thumbnail generation. |
| Timeline metadata | `rusqlite` + `chrono-tz` | Persistent recursive index, stable by-ID media, and deterministic local-day albums. |
| Local vision | optional `ort` | CPU-only ONNX image embeddings against explicit local label vectors; absent from default builds. |
| Album AI | `reqwest` | Optional Responses API client for contact-sheet descriptions after album membership is fixed. |

**Why Rust over Go?**
- `kamadak-exif`: pure-Rust EXIF parser with native HEIC support. Go has no equivalent — requires CGO or manual binary parsing.
- FFI ergonomics: `libheif-rs` provides safe, idiomatic Rust bindings. Go's CGO requires `#cgo` directives, manual memory management, breaks cross-compilation.
- `image` crate: richer format support (WebP encode/decode, animated GIF detection) vs Go's `imaging`.
- `notify`: more robust recursive watching than Go's `fsnotify` (kernel-level on Linux via inotify).
- No garbage collection → predictable memory under sustained WebGL texture serving.

### Frontend

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Build tool | Vite | Fast HMR, multi-page, code splitting, asset hashing. |
| Language | TypeScript | Type safety for WebGL math + API contracts. |
| WebGL | Three.js | Mature, ShaderMaterial, both reference projects use it. |
| Animation | GSAP | lerp/easing primitives. |
| Styling | SCSS + CSS custom properties | Theming via `:root` variables. |
| Routing | Vanilla History API | Three pages; SPA framework is overkill. |
| EXIF display | `exifr` (client-side fallback) | The backend provides structured EXIF JSON; `exifr` as fallback for direct photo inspection. |

### Infrastructure

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Container | Multi-stage Docker → `debian:trixie-slim` | glibc runtime available for both published architectures; runtime installs CA certificates, timezone data, and the GNU C++ runtime required by optional ONNX inference. |
| Build stages | `rust:1.88-trixie` + `node:22-alpine` | The frontend remains architecture-neutral; the Rust builder emits GNU Linux binaries for the selected platform and provides the C++ ABI required by `ort` rc.13 archives. |
| Volumes | `photos:/photos:ro`, `data:/data` | Photos read-only, generated data writable. |
| ONNX build | Disabled by default; `LUMIFLOW_CARGO_FEATURES=vision-onnx` opts in | `ort` rc.13 supplies CPU archives for `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, statically links `libonnxruntime.a`, and dynamically uses Trixie's `libstdc++6`; model/tagset assets remain explicit read-only mounts. |
| Entry | Cloudflare Tunnel (optional) | HTTPS via Cloudflare, no port forwarding. |


## 2.1 Album modes and enrichment boundary

- `folders` is the compatibility default: first-level directories and `manifest.json` remain the source of truth.
- `timeline` recursively scans supported photos, persists stat/EXIF/GPS metadata in `lumiflow.sqlite`, preserves each first-level folder as a hard album boundary, then buckets by configured local day within that folder. Root-level photos use their own bucket.
- Original paths are never copied, moved, renamed, linked, or written.
- Generated album identity includes both the first-level folder and local day; display name, ordering, membership, and cover are deterministic. Local vision and remote AI are post-processing only.
- Local vision reads generated thumbnails, caches model/tagset/fingerprint results in SQLite, and never downloads assets or sends data off-device.
- AI reads at most 36 representative thumbnails through one `224px`-cell JPEG contact sheet plus deterministic metadata. It returns description, keywords, and confidence only. It cannot return or override a title.
- Failures are isolated: thumbnail/vision/contact-sheet/AI failures are counted and logged while albums and original serving remain available. A later rescan retries work whose fingerprint lacks a valid cache entry.

Timeline generated data:

```text
LUMIFLOW_DATA_PATH/lumiflow.sqlite
LUMIFLOW_DATA_PATH/thumbs/by-id/<photo_id>.webp
LUMIFLOW_DATA_PATH/thumbs/by-id/<photo_id>.fingerprint
LUMIFLOW_DATA_PATH/ai/contact-sheets/<sha1(album_id)>.jpg
LUMIFLOW_DATA_PATH/ai/contact-sheets/<sha1(album_id)>.fingerprint
```

## 3. Project Structure

```
LumiFlow/
├── Cargo.toml
├── Cargo.lock
├── src/
│   ├── main.rs                  # Entry: config, tracing, server bootstrap
│   ├── config.rs                # Env-var config parsing
│   ├── server.rs                # Axum router, middleware, graceful shutdown
│   ├── api/
│   │   ├── mod.rs
│   │   ├── albums.rs            # GET /api/albums, GET /api/albums/:name
│   │   ├── photos.rs            # GET /api/photos/:album/:file
│   │   ├── thumbnails.rs        # GET /api/thumbs/:album/:file (on-demand gen)
│   │   ├── exif.rs              # GET /api/exif/:album/:file
│   │   └── rescan.rs            # POST /api/rescan
│   ├── scanner/
│   │   ├── mod.rs
│   │   ├── walk.rs              # Directory traversal, album/photo discovery
│   │   ├── manifest.rs          # Manifest types + read/write + diff detection
│   │   └── watcher.rs           # notify watcher + periodic fallback scan
│   ├── thumbnail/
│   │   ├── mod.rs
│   │   ├── generate.rs          # Decode (image crate or libheif) → resize → WebP encode
│   │   └── pool.rs              # Bounded worker pool for batch generation
│   ├── exif.rs                  # EXIF extraction via kamadak-exif
│   └── embedded.rs              # rust-embed macro for web/dist/
├── web/
│   ├── src/
│   │   ├── fan/                 # Home — fan layout
│   │   │   ├── main.ts
│   │   │   ├── FanScene.ts
│   │   │   ├── AlbumCard.ts
│   │   │   ├── shaders/
│   │   │   │   ├── fan-vertex.glsl
│   │   │   │   └── fan-fragment.glsl
│   │   │   └── style.scss
│   │   ├── grid/                # Album — infinite grid
│   │   │   ├── main.ts
│   │   │   ├── GridScene.ts
│   │   │   ├── PhotoPlane.ts
│   │   │   ├── shaders/
│   │   │   │   ├── grid-vertex.glsl
│   │   │   │   └── grid-fragment.glsl
│   │   │   └── style.scss
│   │   ├── detail/              # Photo — detail + EXIF sidebar
│   │   │   ├── main.ts
│   │   │   ├── DetailView.ts
│   │   │   ├── ExifSidebar.ts
│   │   │   └── style.scss
│   │   ├── shared/
│   │   │   ├── api.ts           # fetch wrappers, typed responses
│   │   │   ├── texture.ts       # Texture loader with placeholder/error
│   │   │   ├── math.ts          # lerp, map, clamp, wrap
│   │   │   ├── router.ts        # History API SPA router
│   │   │   ├── store.ts         # Shared state: albums list, cache
│   │   │   └── types.ts         # Shared TypeScript types (mirrors API)
│   │   └── global.scss
│   ├── fan.html                 # Vite entry: fan page
│   ├── album.html               # Vite entry: album page
│   ├── detail.html              # Vite entry: detail page
│   ├── vite.config.ts
│   ├── tsconfig.json
│   └── package.json
├── Dockerfile
├── docker-compose.example.yml
├── README.md
└── README.zh-CN.md
```

## 4. Backend Design

### 4.1 API Endpoints

```
GET  /api/albums
  → { albums: [{ name, cover, count, created_at, updated_at }] }
  Sorted by album directory creation time, newest first.

GET  /api/albums/:name
  → { name, photos: [{ id, name, width, height, size_bytes, format }] }
  id = index within album (for prev/next navigation).

GET  /api/photos/:album/:file
  → Serve original photo (Range support, ETag, Cache-Control: immutable).

GET  /api/thumbs/:album/:file
  → Serve 400px-wide WebP thumbnail.
  Generate on first request, cache to LUMIFLOW_DATA_PATH/thumbs/.

GET  /api/exif/:album/:file
  → { make, model, lens, focal_length, aperture, shutter_speed, iso,
       date_taken, gps, dimensions, file_size, color_space, flash, ... }
  All fields nullable; missing tags → null.

POST /api/rescan
  → Run the immediate local scan/rebuild/enrichment pass and return counts; optional remote AI continues in a non-blocking background post-processing task.

```

Known client-side routes → SPA fallback (serve embedded `web/dist/index.html` — the fan page). Unknown server routes return 404.

### 4.2 Manifest & Scanning

**Startup flow:**
1. Load `LUMIFLOW_DATA_PATH/manifest.json` if exists.
2. Walk `LUMIFLOW_PHOTOS_PATH` in background (non-blocking).
3. Diff against cached manifest:
   - New albums → add, generate thumbnails for cover.
   - New photos in existing albums → add, generate thumbnails.
   - Deleted albums/photos → remove from manifest, clean up thumbnails.
4. Write updated manifest.
5. Frontend polls `GET /api/albums` with `If-None-Match` → 304 when unchanged.

**Auto-detection (file watcher):**
```
watcher thread:
  ┌─ notify (inotify/FSEvents) watches PHOTOS_PATH recursively
  │   └─ Debounced: batch events within 5s window
  │       └─ On change: re-scan affected subtree only
  │           └─ Diff → update manifest → generate new thumbnails
  │
  └─ Periodic fallback: full re-scan every 30 min
      └─ Ensures consistency if watcher misses events (NFS, CIFS mounts)
```

**Exclusion:** Regex from `LUMIFLOW_EXCLUDE_REGEX`, default: `(^|/)(@eaDir|#recycle|\.DS_Store|Thumbs\.db)(/|$)`.

**Album sorting:** By `created_at` (directory birth time / `btime` on Linux, `birthtime` on macOS). Fallback to `modified_at` if birth time unavailable.

### 4.3 Thumbnail Pipeline

```
Input: /photos/<album>/<file> (any supported format)
  │
  ├─ .heic / .heif → libheif-rs decode → RGBA buffer
  ├─ .jpg / .jpeg → image::jpeg decode → RGBA buffer
  ├─ .png         → image::png decode → RGBA buffer
  ├─ .webp        → image::webp decode → RGBA buffer
  └─ .gif         → image::gif decode (first frame) → RGBA buffer
  │
  ▼
Resize: Lanczos3 → 400px wide, proportional height
  │
  ▼
Encode: webp::Encoder::from_image(), quality 80
  │
  ▼
Output: /data/thumbs/<album>/<file>.webp
```

**Worker pool:** `LUMIFLOW_BUILDER_WORKERS` (default 2). Bounded semaphore; on-demand generation takes priority over batch pre-generation.

**Cache strategy:**
- Thumbnail filename = `{original_filename}.webp` → stable across restarts.
- Check thumb exists + newer than source → skip generation.
- On photo deletion: delete corresponding thumb.
- On photo modification: regenerate thumb.

### 4.4 EXIF Extraction

```
Source: /photos/<album>/<file>
  │
  ▼
kamadak-exif::Reader::read_from_container()
  │
  ├─ JPEG: reads EXIF from APP1 marker
  └─ HEIC: reads EXIF from iloc/Exif item
  │
  ▼
Extract known tags → structured JSON:
  {
    "make": "Apple",                    // IFD0::Make
    "model": "iPhone 15 Pro",           // IFD0::Model
    "lens": "iPhone 15 Pro back triple camera 6.765mm f/1.78",
    "focal_length": "6.765mm",          // ExifIFD::FocalLength
    "aperture": "f/1.78",               // ExifIFD::FNumber
    "shutter_speed": "1/250",           // ExifIFD::ExposureTime
    "iso": 200,                         // ExifIFD::ISOSpeedRatings
    "date_taken": "2024-03-15T14:30:00Z", // ExifIFD::DateTimeOriginal
    "gps": { "lat": 35.6762, "lon": 139.6503 },
    "dimensions": { "width": 4032, "height": 3024 },
    "file_size": 2_456_789,
    "format": "HEIC",
    "flash": "Off, Did not fire",
    "software": null,
    "orientation": 1
  }
```

Fallback: if EXIF parse fails → return `{ format, dimensions, file_size }` only.

### 4.5 Photo Serving

- `Accept-Ranges: bytes` + Range header handling (progressive loading).
- `Cache-Control: public, max-age=31536000, immutable` (photos are immutable by contract).
- `ETag: <mtime_nano_hex>` for efficient 304 responses.
- `Content-Type` from file extension; `Content-Disposition: inline`.

## 5. Frontend Design

### 5.1 Home Page — Chinese Folding Fan

**Concept:** Albums arranged as ribs (扇骨) of a folding fan, pivoting from bottom-center.
Scroll/drag to rotate through albums. Click an album to enter.

```
                 ____----~~~~----____
             ----    album4  album3    ----
          ---    album5    ●     album2    ---
        --   album6              album1      --
       /    album7    ●  ●  ●     album12     \
      |   album8   PIVOT POINT    album11      |
       \   album9               album10      /
        --  album10            albumN     --
          ---                          ---
             ----____        ____----
                     ~~~~~~~~
```

**Geometry:**
- Albums on a cylindrical arc: radius $R = 1.2 \times V_h$, arc span $\theta_{\text{span}} = 160°$.
- Each album = textured Three.js quad, rotated to face pivot point.
- Position for album $i$ (total $N$, current offset $\theta_o$):
  $$\theta_i = \theta_o + \left(i - \frac{N-1}{2}\right) \cdot \frac{\theta_{\text{span}}}{N-1}$$
  $$(x_i, z_i) = (R \cdot \alpha \cdot \sin\theta_i,\; R \cdot (1 - \alpha \cdot (1 - \cos\theta_i)))$$

**Fan open/close:** Parameter $\alpha \in [0, 1]$. Page enter: $\alpha$ animates 0→1 (GSAP power3.out, 1.5s). Page exit: $\alpha$ → 0, focused album scales up.

**Interaction:**

| Input | Action |
|-------|--------|
| Scroll wheel (X) / touch drag ← → | Rotate fan |
| Click album | → `/album/<name>` |
| Hover album | Scale ×1.05 + golden glow |
| Edge albums | Fade opacity, desaturate |
| `Escape` | Reset rotation to center |

**Shader effects:**
- *Vertex*: subtle z-wave for organic "breathing" (from circular gallery reference).
- *Fragment*: aspect-ratio-correct UV fit; vignette toward edges; desaturation for non-focused.

**Visual style:**
- Background: `#0d0d0d` with radial gradient + noise texture → silk-like.
- Fan ribs: thin `#c4a35a` lines from pivot, between album cards.
- Active album: full saturation, `#d4af37` border glow.
- Typography: `system-ui` sans-serif (Chinese + Latin).
- Pivot handle: small circle with glow.

### 5.2 Album Page — Infinite WebGL Grid

Based on `infinite-scrollable-and-draggable-webgl-grid`.

**Structure:** CSS Grid (5 cols, responsive → 3/4/5 based on viewport) + Three.js overlay.

**Each cell:** Three.js `Plane` (Object3D + PlaneGeometry + ShaderMaterial) positioned via `getBoundingClientRect()`.

**Infinite wrap:** When a plane exits one boundary, wraps to opposite side (gsap.utils.wrap).

**Interaction:**

| Input | Action |
|-------|--------|
| Drag / touch pan | Pan grid (both axes) |
| Scroll wheel / pinch | Zoom |
| Click photo | → `/album/<name>/photo/<id>` |
| `←` / browser back | → fan page |
| `Escape` | → fan page |

**Shader:** aspect-ratio-correct UV; `u_diff` for drag wobble; LinearFilter (no mipmaps).

**Performance:** Lazy texture loading (visible + buffer zone); WebP thumbnails; dispose off-screen textures.

### 5.3 Photo Detail Page

```
┌─────────────────────────────────────────────────┐
│ ← Back to album    ← Prev    Photo 3/42    Next →│
├──────────────────────────┬──────────────────────┤
│                          │ 📷 Camera            │
│                          │ Sony A7M4            │
│                          │                      │
│     Full-resolution      │ 🔭 Lens              │
│     photo display        │ FE 24-70mm F2.8 GM   │
│     (contained to        │                      │
│      available space)    │ 📏 Focal Length      │
│                          │ 35mm                 │
│                          │                      │
│                          │ 🔆 Aperture          │
│                          │ f/2.8                │
│                          │                      │
│                          │ ⏱ Shutter Speed      │
│                          │ 1/500s               │
│                          │                      │
│                          │ 🎯 ISO               │
│                          │ 400                  │
│                          │                      │
│                          │ 📅 Date Taken        │
│                          │ 2024-03-15 14:30     │
│                          │                      │
│                          │ 📍 GPS               │
│                          │ 35.6762, 139.6503    │
│                          │ (map link)           │
│                          │                      │
│                          │ 📐 Dimensions        │
│                          │ 7008 × 4672          │
│                          │                      │
│                          │ 💾 File Size         │
│                          │ 12.3 MB              │
│                          │                      │
│                          │ 🖼 Format            │
│                          │ HEIC                 │
└──────────────────────────┴──────────────────────┘
```

**Layout:** CSS Grid — photo area (2/3) + sidebar (1/3). On narrow screens: stacked, sidebar below.

**Photo display:** serve original via `<img>` or fetch + render at native resolution with pan/zoom.
V1: simple `<img>` with `object-fit: contain` + click-to-zoom.

**Sidebar:** EXIF data from `GET /api/exif/:album/:file`. Icons + human-readable values.
GPS → clickable map link (Apple Maps / Google Maps).

**Navigation:** Prev/next arrows; keyboard `←` `→`; swipe on mobile.

**EXIF data loading:** Fetched on mount, cached per photo in memory.

### 5.4 Routing

```
History API:
  /                            → FanScene
  /album/<name>                → GridScene
  /album/<name>/photo/<id>     → DetailView

State:
  - Albums list (cached, poll every 30s for updates)
  - Current route + params
  - Texture LRU cache (max ~50)
```

### 5.5 Loading & Error States

- **Skeleton screens:** CSS shimmer placeholders for fan cards + grid cells.
- **Progressive:** Nearest albums load first; grid loads visible-first.
- **Error:** "加载失败" toast + retry button.
- **Empty:** "此相册暂无照片" for empty albums.

## 6. Supported Formats

| Format | Extension | Thumbnail | EXIF | Notes |
|--------|-----------|-----------|------|-------|
| JPEG | `.jpg` `.jpeg` | ✅ `image` | ✅ `kamadak-exif` | |
| PNG | `.png` | ✅ `image` | ⚠️ limited | PNG EXIF is rare |
| WebP | `.webp` | ✅ `image` | ✅ | |
| HEIC | `.heic` `.heif` | ✅ `libheif-rs` | ✅ `kamadak-exif` | Requires libheif at runtime |
| GIF | `.gif` | ✅ `image` (1st frame) | ❌ | Animated → static thumbnail |
| AVIF | `.avif` | ✅ `libheif-rs` | ⚠️ | Via libheif |
| TIFF | `.tif` `.tiff` | ✅ `image` | ✅ `kamadak-exif` | |

## 7. Docker Deployment

### 7.1 Dockerfile

```dockerfile
# Stage 1: Frontend build
FROM node:22-alpine AS frontend
WORKDIR /build
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# Stage 2: Rust build (glibc)
FROM rust:1.88-bookworm AS backend
ARG LUMIFLOW_CARGO_FEATURES=""
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY --from=frontend /build/dist ./web/dist/
RUN cargo build --release --locked ${LUMIFLOW_CARGO_FEATURES:+--features "$LUMIFLOW_CARGO_FEATURES"}
RUN strip target/release/lumiflow

# Stage 3: Runtime
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*
COPY --from=backend /build/target/release/lumiflow /usr/local/bin/lumiflow
EXPOSE 4320
ENV LUMIFLOW_PORT=4320
ENV LUMIFLOW_BIND_ADDRESS=0.0.0.0
ENTRYPOINT ["lumiflow"]
```

The Dockerfile builds for both `linux/amd64` (`x86_64-unknown-linux-gnu`) and `linux/arm64` (`aarch64-unknown-linux-gnu`). The default feature set is empty and therefore downloads or packages no ONNX Runtime. With `LUMIFLOW_CARGO_FEATURES=vision-onnx`, `ort` rc.13 downloads the matching CPU archive and statically links `libonnxruntime.a` into the executable; there is no ONNX shared library to copy or configure in the runtime image.

### 7.2 docker-compose

```yaml
services:
  lumiflow:
    image: armerr/lumiflow:latest
    container_name: lumiflow
    restart: unless-stopped
    user: "1000:1000"
    ports:
      - "127.0.0.1:4320:4320"
    volumes:
      - /volume1/photos/gallery:/photos:ro
      - ./lumiflow-data:/data
    environment:
      LUMIFLOW_PHOTOS_PATH: /photos
      LUMIFLOW_DATA_PATH: /data
      LUMIFLOW_BIND_ADDRESS: 0.0.0.0
      LUMIFLOW_PORT: 4320
      LUMIFLOW_BUILDER_WORKERS: "2"
      RUST_LOG: lumiflow=info,tower_http=warn
```

## 8. Implementation Phases

### Phase 1: Backend Foundation
- Cargo project, dependencies, config loading.
- Directory scanner → manifest.json (walkdir + diff).
- Axum server: routing, embedded frontend serving, SPA fallback.
- `/api/albums`, `/api/albums/:name`.
- Photo serving (`/api/photos`) with Range + ETag.
- Dockerfile multi-stage build.
- **Verify:** `curl /api/albums` returns correct data with test directory.

### Phase 2: Thumbnails + EXIF
- Thumbnail generation: `image` crate for JPEG/PNG/WebP/GIF.
- HEIC decode via `libheif-rs`.
- WebP encode via `webp` crate.
- On-demand generation (`/api/thumbs`) + background pre-generation.
- EXIF extraction via `kamadak-exif` (`/api/exif`).
- Worker pool with bounded concurrency.
- **Verify:** `curl /api/thumbs/<album>/<file>` returns valid WebP; `curl /api/exif/...` returns JSON.

### Phase 3: File Watcher
- `notify` watcher: recursive, debounced.
- Periodic full re-scan every 30 min.
- Diff detection: new/removed albums/photos.
- Auto thumbnail generation for new photos.
- Auto cleanup for deleted photos.
- **Verify:** Add photo to test dir → appears in API within ~10s.

### Phase 4: Frontend Skeleton
- Vite multi-page setup (fan, album, detail).
- Router (History API).
- API client with typed responses.
- Skeleton loading + error states.
- Global styles + CSS custom properties.

### Phase 5: Fan Page
- Three.js scene: renderer, camera, lights.
- Album card mesh with aspect-ratio-correct texturing.
- Fan arc geometry + positioning math.
- Scroll + drag interaction.
- Fan open/close animation.
- Custom GLSL shaders.
- Visual polish (ribs, background, effects).

### Phase 6: Grid Page
- CSS Grid + Three.js overlay.
- PhotoPlane: getBoundingClientRect → position sync.
- Infinite wrap-around panning.
- Drag + wheel interaction.
- Lazy texture loading + disposal.
- Custom shaders.

### Phase 7: Detail Page
- Full-size photo display with pan/zoom.
- EXIF sidebar with icons + human-readable values.
- Prev/next navigation + keyboard support.
- GPS → map link.
- Mobile responsive (stacked layout).

### Phase 8: Polish & Deploy
- Page transition animations.
- Performance audit (texture memory, paint frames).
- Mobile/touch optimization.
- README + deployment docs.
- docker-compose.example.yml.
