# LumiFlow

LumiFlow is a self-hosted photo album for local and NAS photo libraries. It scans a read-only photo root, treats each first-level directory as an album, generates cached WebP thumbnails, and serves a WebGL gallery UI from a single Rust binary.

[简体中文文档](README.zh-CN.md)

Default port: `4320`.

## Features

- Folder-based albums: every first-level directory under the photo root becomes one album.
- WebGL album home with a folding-fan cover layout.
- Infinite draggable photo grid for album browsing.
- Photo detail view with keyboard, touch, and download support.
- EXIF metadata extraction for camera, lens, exposure, GPS, dimensions, and file details.
- Original photo serving with Range requests and immutable cache headers.
- Automatic manifest and thumbnail cache generation.
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
| `LUMIFLOW_BUILDER_WORKERS` | `2` | no | Concurrent thumbnail generation workers. Lower it on small NAS devices. |
| `LUMIFLOW_EXCLUDE_REGEX` | built-in NAS/system-file ignore regex | no | Regex for files/directories skipped during scans. |
| `RUST_LOG` | `lumiflow=info,tower_http=warn` | no | Rust log filter. |

## Photo directory layout

```text
/photos/
├── 2024-tokyo/
│   ├── DSC0001.jpg
│   └── DSC0002.heic
└── 2025-kyoto/
    └── IMG_0001.png
```

Only first-level directories under the photo root become albums. Files placed directly in the root are ignored.

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
