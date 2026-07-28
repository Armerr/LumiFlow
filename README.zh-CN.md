# LumiFlow

LumiFlow 是一个面向本地/NAS 照片库的自托管相册服务。它扫描只读照片根目录，把每个一级子目录作为一个相册，生成缓存的 WebP 缩略图，并通过单个 Rust 二进制提供 WebGL 相册界面。

[English README](README.md)

默认端口：`4320`。

## 功能

- 按文件夹组织相册：照片根目录下的每个一级子目录就是一个相册。
- WebGL 折扇式相册首页。
- 可拖拽的无限照片网格相册页。
- 照片详情页支持键盘、触摸和原图下载。
- EXIF 元数据提取：相机、镜头、曝光、GPS、尺寸和文件信息。
- 原图接口支持 Range 请求和不可变缓存头。
- 自动生成 manifest 和缩略图缓存。
- 优先支持 Docker 部署，适合 NAS 和家用服务器。

## Docker 镜像

已发布镜像：

```text
armerr/lumiflow:latest
```

`latest` 标签支持：

- `linux/amd64`
- `linux/arm64`

如需本地构建镜像：

```bash
docker build -t lumiflow:local .
```

## Docker Compose 快速开始

复制 Compose 示例并按宿主机环境修改：

```bash
cp docker-compose.example.yml docker-compose.yml
```

修改 `docker-compose.yml` 里的这些行：

```yaml
user: "1000:1000"
ports:
  - "127.0.0.1:4320:4320"
volumes:
  - /你的/照片/目录:/photos:ro
  - ./lumiflow-data:/data
```

启动服务：

```bash
docker compose up -d
```

打开：

```text
http://127.0.0.1:4320
```

如需局域网直接访问，修改 `ports` 里的发布地址：

```yaml
ports:
  - "0.0.0.0:4320:4320"
```

如使用 Cloudflare Tunnel、Nginx、Caddy 或其他反向代理，建议保持默认的本机绑定，并把代理目标设为：

```text
http://127.0.0.1:4320
```

## Compose 文件

仓库包含 `docker-compose.example.yml`：

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

## 环境变量

| 变量 | 默认值 | 必填 | 说明 |
|---|---:|---:|---|
| `LUMIFLOW_PHOTOS_PATH` | 无 | 是 | 程序看到的照片根目录。Docker 内通常是 `/photos`。 |
| `LUMIFLOW_DATA_PATH` | 无 | 是 | manifest 和缩略图缓存目录，必须可写。Docker 内通常是 `/data`。 |
| `LUMIFLOW_BIND_ADDRESS` | `0.0.0.0` | 否 | Rust 服务监听地址。Docker 应保持 `0.0.0.0`。 |
| `LUMIFLOW_PORT` | `4320` | 否 | Rust 服务监听端口。 |
| `LUMIFLOW_BUILDER_WORKERS` | `2` | 否 | 缩略图并发生成数量。小型 NAS 可以调低。 |
| `LUMIFLOW_EXCLUDE_REGEX` | 内置 NAS/系统文件忽略正则 | 否 | 扫描时跳过的文件/目录正则。 |
| `RUST_LOG` | `lumiflow=info,tower_http=warn` | 否 | Rust 日志过滤规则。 |

## 照片目录结构

```text
/photos/
├── 2024-tokyo/
│   ├── DSC0001.jpg
│   └── DSC0002.heic
└── 2025-kyoto/
    └── IMG_0001.png
```

只有照片根目录下的一级子目录会成为相册。直接放在根目录里的文件会被忽略。

## 从源码运行

Rust 二进制会嵌入 `web/dist`，所以必须先构建前端：

```bash
cd web
npm ci
npm run build
cd ..

cargo build --release --locked
LUMIFLOW_PHOTOS_PATH=/你的/照片/目录 \
LUMIFLOW_DATA_PATH=./lumiflow-data \
./target/release/lumiflow
```

打开：

```text
http://127.0.0.1:4320
```

前端开发时在 `web/` 里运行 Vite；`web/vite.config.ts` 会把 `/api` 代理到 `127.0.0.1:4320`。

## API

| 接口 | 用途 |
|---|---|
| `GET /` | Web 应用。 |
| `GET /api/albums` | 相册列表。 |
| `GET /api/albums/:name` | 单个相册内的照片列表。 |
| `GET /api/thumbs/:album/:file` | 缓存/生成的 WebP 缩略图。 |
| `GET /api/photos/:album/:file` | 原图，支持 Range。 |
| `GET /api/photos/:album/:file?download=1` | 以附件形式下载原图。 |
| `GET /api/exif/:album/:file` | EXIF 和文件元数据。 |
| `POST /api/rescan` | 手动触发重新扫描。 |

## 支持的图片格式

- 可索引并作为原图服务的扩展名：`jpg`、`jpeg`、`png`、`webp`、`gif`、`heic`、`heif`、`avif`、`tif`、`tiff`。
- 默认构建可生成缩略图并做影调分析的格式：JPEG、PNG、WebP、GIF。
- HEIC、HEIF、AVIF 缩略图需要自定义 Rust 构建并启用 `heic` feature，同时准备系统级 libheif 依赖。
- TIFF 原图可以服务，但默认构建没有启用 TIFF 缩略图解码。

缩略图生成失败不会影响原图接口。

## 开发说明

已忽略的本地文件包括依赖目录、构建产物、本地数据/缓存目录、本地 Compose 覆盖文件、日志和临时文件。

需要保留提交的文件包括：

- `docker-compose.example.yml`
- `Dockerfile`
- `Cargo.lock`
- `web/package-lock.json`

## 排障

- 相册为空：确认 `volumes` 里的第一个宿主机路径指向照片根目录，并且其下有一级相册目录。
- 权限错误：确认 Compose `user` 对照片目录有读权限，对缓存目录有写权限。
- HEIC/AVIF/TIFF 没有缩略图：默认构建不解码这些格式的缩略图。
- 只通过反向代理访问：保持端口只绑定到 `127.0.0.1`，例如 `127.0.0.1:4320:4320`。
- 需要局域网直接访问：发布到 `0.0.0.0`，例如 `0.0.0.0:4320:4320`。

## 许可证

MIT。详见 [LICENSE](LICENSE)。
