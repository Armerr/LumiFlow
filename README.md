# LumiFlow

LumiFlow is a self-hosted photo album for local and NAS photo libraries. It recursively indexes mounted libraries into SQLite by default, generates cached WebP thumbnails on demand, and serves a WebGL album home plus a memory-bounded vertical photo grid from a single Rust binary. Original photo directories stay read-only.

[简体中文文档](README.zh-CN.md)

Default port: `4320`.

## Features

- Folder-based albums: every first-level directory under the photo root becomes one album.
- Timeline albums: recursively index nested photos, keep first-level folders as hard boundaries, then build deterministic daily virtual albums within each folder without moving originals.
- Optional local CPU vision tags and optional cached album descriptions from a low-resolution contact sheet.
- WebGL album home with a folding-fan cover layout.
- Native vertical-only album grid with paginated metadata, lazy thumbnails, and bounded DOM residency.
- Photo detail view with keyboard controls, browser-compatible WebP preview, and original-file download.
- EXIF metadata extraction for camera, lens, exposure, GPS, dimensions, and file details.
- Original photo serving with Range requests and immutable cache headers.
- Automatic SQLite indexing and on-demand thumbnail cache generation.
- Docker-first deployment for NAS and home servers.

## Docker image

The published image is:

```text
armerr/lumiflow:latest
```

The `latest` tag is published for:

- `linux/amd64`
- `linux/arm64`

To build a local image instead:

```bash
docker build -t lumiflow:local .
```

The default image uses glibc and intentionally excludes ONNX code and runtime assets. It remains available for both `linux/amd64` and `linux/arm64`. To opt into local vision, build the same multi-architecture Dockerfile with `vision-onnx`:

```bash
docker build --build-arg LUMIFLOW_CARGO_FEATURES=vision-onnx -t lumiflow:vision-onnx .
```

`ort` rc.13 downloads the matching `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu` CPU archive and statically links ONNX Runtime into the LumiFlow executable. The image uses Debian Trixie because those archives require its newer GNU C++ ABI; the runtime installs `libstdc++6` but needs no separate ONNX Runtime shared library or loader-path configuration. The model and label-vector files are still explicit read-only runtime mounts.

## Docker Compose quick start

Copy the Compose example and edit it for the host:

```bash
cp docker-compose.example.yml docker-compose.yml
```

Edit these lines in `docker-compose.yml`:

```yaml
user: "1000:1000"
ports:
  - "127.0.0.1:4320:4320"
volumes:
  - /path/to/your/photos:/photos:ro
  - ./lumiflow-data:/data
```

Start the service:

```bash
docker compose up -d
```

Open:

```text
http://127.0.0.1:4320
```

For direct LAN access, change the published address in `ports`:

```yaml
ports:
  - "0.0.0.0:4320:4320"
```

For Cloudflare Tunnel, Nginx, Caddy, or another reverse proxy, keep the default loopback bind and proxy to:

```text
http://127.0.0.1:4320
```

## Compose file

The repository includes `docker-compose.example.yml`:

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

## Environment variables

| Variable | Default | Required | Description |
|---|---:|---:|---|
| `LUMIFLOW_PHOTOS_PATH` | none | yes | Photo root seen by the application. In Docker this is usually `/photos`. |
| `LUMIFLOW_DATA_PATH` | none | yes | Writable directory for `manifest.json` and generated thumbnails. In Docker this is usually `/data`. |
| `LUMIFLOW_BIND_ADDRESS` | `0.0.0.0` | no | Address the Rust server binds to. Docker should keep `0.0.0.0`. |
| `LUMIFLOW_PORT` | `4320` | no | Rust server port. |
| `LUMIFLOW_BUILDER_WORKERS` | `1` | no | Maximum concurrent on-demand thumbnail decodes. Raise it only when the host has sufficient memory. |
| `LUMIFLOW_EXCLUDE_REGEX` | built-in NAS/system-file ignore regex | no | Regex for files/directories skipped during scans. |
| `RUST_LOG` | `lumiflow=info,tower_http=warn` | no | Rust log filter. |
| `LUMIFLOW_ALBUM_MODE` | `timeline` | no | `timeline` recursively indexes photos into SQLite while preserving first-level folder boundaries; set `folders` explicitly for legacy first-level directory albums. |
| `LUMIFLOW_TIMELINE_TIMEZONE` | `Asia/Shanghai` | no | IANA timezone used for timeline date bucketing. |
| `LUMIFLOW_CALENDAR_REGION` | `CN_COMMON` | no | Calendar naming region. The first release supports `CN_COMMON`. |
| `LUMIFLOW_PLACE_PROVIDER` | none | no | Optional reverse-geocoding provider. Set to `nominatim` to explicitly allow GPS lookups; when unset, LumiFlow uses only its place cache and path fallback and sends no GPS data over the network. |
| `LUMIFLOW_PLACE_BASE_URL` | none | with Nominatim | Nominatim-compatible service base URL. Prefer a self-hosted endpoint for large libraries. `https://nominatim.openstreetmap.org` is possible subject to its usage policy; LumiFlow appends `/reverse` unless already present. |
| `LUMIFLOW_VISION_TAGGER` | `none` | no | `none` or `onnx-mobileclip`. ONNX requires a feature-enabled build and explicit local assets. |
| `LUMIFLOW_VISION_MODEL_PATH` | none | with ONNX | Local ONNX image-encoder path. LumiFlow never downloads model assets. |
| `LUMIFLOW_VISION_LABELS_PATH` | none | with ONNX | Local labels/text-embedding JSON path described below. |
| `LUMIFLOW_VISION_WORKERS` | `1` | no | Positive ONNX intra-op CPU thread count. |
| `LUMIFLOW_AI_ENABLED` | `false` | no | Enables cached album descriptions after deterministic album creation. |
| `LUMIFLOW_AI_PROVIDER` | none | no | If set, must be `openai-compatible`. The configured service must implement the Responses API image-input schema. |
| `LUMIFLOW_AI_BASE_URL` | none | with AI | Base URL such as `https://api.openai.com/v1`, or a full URL ending in `/responses`. |
| `LUMIFLOW_AI_API_KEY` | none | with AI | Bearer token. Never written to SQLite or logs. |
| `LUMIFLOW_AI_MODEL` | none | with AI | Vision-capable Responses API model ID. |
| `LUMIFLOW_AI_DESCRIPTION_LANGUAGE` | `zh-CN` | no | Requested album-description language. |

## Timeline albums and enrichment

Enable recursive timeline albums without optional enrichment:

```yaml
environment:
  LUMIFLOW_ALBUM_MODE: timeline
  LUMIFLOW_TIMELINE_TIMEZONE: Asia/Shanghai
  LUMIFLOW_CALENDAR_REGION: CN_COMMON
  LUMIFLOW_VISION_TAGGER: none
  LUMIFLOW_AI_ENABLED: "false"
```

Timeline mode stores metadata and daily membership in `LUMIFLOW_DATA_PATH/lumiflow.sqlite`. By-ID WebP thumbnails live under `thumbs/by-id/`. AI contact sheets and fingerprints live under `ai/contact-sheets/`. Rescans reuse unchanged EXIF, thumbnails, local tags, contact sheets, and AI descriptions.

Scanning, album rebuilding, thumbnails, local tags, and contact sheets complete before a rescan response. Remote AI requests then run in a background post-processing pass, so a slow or unavailable provider does not delay server startup or manual/watcher rescans. Refresh the album list to see newly cached descriptions.

Optional local vision requires a binary built with `--features vision-onnx`, then these settings and read-only asset mounts:

```yaml
volumes:
  - /path/to/mobileclip-image-encoder.onnx:/models/mobileclip.onnx:ro
  - /path/to/mobileclip-labels.json:/models/mobileclip-labels.json:ro
environment:
  LUMIFLOW_VISION_TAGGER: onnx-mobileclip
  LUMIFLOW_VISION_MODEL_PATH: /models/mobileclip.onnx
  LUMIFLOW_VISION_LABELS_PATH: /models/mobileclip-labels.json
  LUMIFLOW_VISION_WORKERS: "1"
```

The image encoder contract is one float32 input `[1,3,N,N]` and one float32 embedding output `[1,D]` or `[D]`. The labels file is strict JSON:

```json
{
  "version": 1,
  "model_id": "mobileclip-s0-image-v1",
  "tagset_version": "lumiflow-tags-v1",
  "image_size": 224,
  "mean": [0.0, 0.0, 0.0],
  "std": [1.0, 1.0, 1.0],
  "labels": [{"label": "family", "embedding": [0.1, 0.2]}]
}
```

Every embedding must have exactly the model output dimension; the abbreviated vector above only illustrates the schema.

Optional AI configuration:

```yaml
environment:
  LUMIFLOW_AI_ENABLED: "true"
  LUMIFLOW_AI_PROVIDER: openai-compatible
  LUMIFLOW_AI_BASE_URL: https://api.openai.com/v1
  LUMIFLOW_AI_API_KEY: ${LUMIFLOW_AI_API_KEY}
  LUMIFLOW_AI_MODEL: gpt-5-mini
  LUMIFLOW_AI_DESCRIPTION_LANGUAGE: zh-CN
```

Privacy boundary: local tagging reads cached thumbnails only and stays on-device. When AI is enabled, LumiFlow sends one low-resolution contact-sheet JPEG plus album metadata to the configured provider; it never uploads originals. AI cannot rename albums or change membership. Provider, thumbnail, contact-sheet, or AI failures leave deterministic albums and original serving available; a future rescan retries stale work.

## Photo directory layout

```text
/photos/
├── 2024-tokyo/
│   ├── DSC0001.jpg
│   └── DSC0002.heic
└── 2025-kyoto/
    └── IMG_0001.png
```

In `folders` mode, only first-level directories become albums and root-level photos are ignored. In `timeline` mode, supported photos are indexed recursively at any depth, but photos from different first-level folders are never merged into the same generated album; local-day grouping happens within each folder. Root-level photos use a separate root bucket.

## Run from source

The Rust binary embeds `web/dist`, so the frontend must be built first:

```bash
cd web
npm ci
npm run build
cd ..

cargo build --release --locked
LUMIFLOW_PHOTOS_PATH=/path/to/photos \
LUMIFLOW_DATA_PATH=./lumiflow-data \
./target/release/lumiflow
```

Open:

```text
http://127.0.0.1:4320
```

For frontend development, run Vite in `web/`. `web/vite.config.ts` proxies `/api` to `127.0.0.1:4320`.

## API

| Endpoint | Description |
|---|---|
| `GET /` | Web application. |
| `GET /api/albums` | Album list. |
| `GET /api/albums/:name` | Photos in one album. |
| `GET /api/thumbs/:album/:file` | Cached/generated WebP thumbnail. |
| `GET /api/photos/:album/:file` | Original photo with Range support. |
| `GET /api/photos/:album/:file?download=1` | Original photo as an attachment download. |
| `GET /api/exif/:album/:file` | EXIF and file metadata. |
| `GET /api/thumbs/by-id/:photo_id` | Timeline thumbnail by stable photo ID. |
| `GET /api/photos/by-id/:photo_id` | Timeline original by stable photo ID, with Range and optional `?download=1`. |
| `GET /api/exif/by-id/:photo_id` | Timeline EXIF by stable photo ID. |
| `POST /api/rescan` | Trigger a manual rescan. |

## Supported image formats

- Indexed and served as originals: `jpg`, `jpeg`, `png`, `webp`, `gif`, `heic`, `heif`, `avif`, `tif`, `tiff`.
- Thumbnail and tone analysis support in the default build: JPEG, PNG, WebP, GIF.
- HEIC, HEIF, and AVIF thumbnails require a custom Rust build with the `heic` feature and system libheif dependencies.
- TIFF originals can be served, but TIFF thumbnail decoding is not enabled in the default build.

Thumbnail generation failures do not prevent original photo serving.

## Development notes

Ignored local artifacts include dependency directories, build outputs, local data/cache directories, local Compose overrides, logs, and temporary files.

Files expected to stay committed include:

- `docker-compose.example.yml`
- `Dockerfile`
- `Cargo.lock`
- `web/package-lock.json`

## Troubleshooting

- Empty album list: ensure the first host path under `volumes` points to a photo root with first-level album directories.
- Permission errors: ensure the Compose `user` can read the photo directory and write the data directory.
- Missing HEIC/AVIF/TIFF thumbnails: the default build does not decode these formats for thumbnail generation.
- Reverse proxy only: keep the published port bound to `127.0.0.1`, for example `127.0.0.1:4320:4320`.
- Direct LAN access: publish on `0.0.0.0`, for example `0.0.0.0:4320:4320`.

## License

MIT. See [LICENSE](LICENSE).
