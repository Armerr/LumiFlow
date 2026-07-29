# LumiFlow

LumiFlow 是一个面向本地/NAS 照片库的自托管相册服务。它既支持文件夹相册，也支持面向杂乱嵌套备份的可选 SQLite 时间线模式；程序生成缓存的 WebP 缩略图，并通过单个 Rust 二进制提供 WebGL 相册界面。原始照片目录始终只读。

[English README](README.md)

默认端口：`4320`。

## 功能

- 按文件夹组织相册：照片根目录下的每个一级子目录就是一个相册。
- 时间线相册：递归索引嵌套照片，先以一级文件夹为硬边界，再在各文件夹内按自然日稳定生成虚拟相册，不移动原图。
- 可选本地 CPU 视觉标签，以及通过低分辨率 contact sheet 生成并缓存的相册描述。
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

默认镜像使用 glibc，且不包含 ONNX 代码或运行时资产，仍同时支持 `linux/amd64` 和 `linux/arm64`。如需本地视觉功能，可用同一份多架构 Dockerfile 启用 `vision-onnx`：

```bash
docker build --build-arg LUMIFLOW_CARGO_FEATURES=vision-onnx -t lumiflow:vision-onnx .
```

`ort` rc.13 会下载对应的 `x86_64-unknown-linux-gnu` 或 `aarch64-unknown-linux-gnu` CPU 归档，并把 ONNX Runtime 静态链接进 LumiFlow 可执行文件。镜像使用 Debian Trixie，因为这些归档依赖其较新的 GNU C++ ABI；运行时会安装 `libstdc++6`，但不需要单独的 ONNX Runtime 共享库或动态加载路径配置。模型和标签向量文件仍需在运行时以只读方式显式挂载。

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
| `LUMIFLOW_ALBUM_MODE` | `folders` | 否 | `folders` 保留一级目录相册；`timeline` 同样保留一级文件夹边界，再递归扫描并在各文件夹内生成每日虚拟相册。 |
| `LUMIFLOW_TIMELINE_TIMEZONE` | `Asia/Shanghai` | 否 | 时间线按日分组使用的 IANA 时区。 |
| `LUMIFLOW_CALENDAR_REGION` | `CN_COMMON` | 否 | 节日命名区域；首个版本支持 `CN_COMMON`。 |
| `LUMIFLOW_PLACE_PROVIDER` | 无 | 否 | 可选逆地理编码服务。设为 `nominatim` 才会显式允许 GPS 查询；未设置时仅使用地点缓存和路径回退，不会通过网络发送 GPS 数据。 |
| `LUMIFLOW_PLACE_BASE_URL` | 无 | Nominatim 必填 | Nominatim 兼容服务基础 URL；大型照片库建议使用自托管端点。也可使用 `https://nominatim.openstreetmap.org`（须遵守其使用政策）；若 URL 末尾没有 `/reverse`，LumiFlow 会自动追加。 |
| `LUMIFLOW_VISION_TAGGER` | `none` | 否 | `none` 或 `onnx-mobileclip`；ONNX 需要启用 feature 的构建和显式本地资产。 |
| `LUMIFLOW_VISION_MODEL_PATH` | 无 | ONNX 必填 | 本地 ONNX 图像编码器路径；LumiFlow 永不自动下载模型。 |
| `LUMIFLOW_VISION_LABELS_PATH` | 无 | ONNX 必填 | 下文所述的本地标签/文本向量 JSON 路径。 |
| `LUMIFLOW_VISION_WORKERS` | `1` | 否 | 正整数，ONNX CPU intra-op 线程数。 |
| `LUMIFLOW_AI_ENABLED` | `false` | 否 | 在确定性相册创建完成后生成缓存描述。 |
| `LUMIFLOW_AI_PROVIDER` | 无 | 否 | 如设置，必须为 `openai-compatible`；服务需实现 Responses API 图片输入 schema。 |
| `LUMIFLOW_AI_BASE_URL` | 无 | AI 必填 | 例如 `https://api.openai.com/v1`，也可直接填写以 `/responses` 结尾的完整 URL。 |
| `LUMIFLOW_AI_API_KEY` | 无 | AI 必填 | Bearer token；不会写入 SQLite 或日志。 |
| `LUMIFLOW_AI_MODEL` | 无 | AI 必填 | 支持视觉输入的 Responses API 模型 ID。 |
| `LUMIFLOW_AI_DESCRIPTION_LANGUAGE` | `zh-CN` | 否 | 相册描述语言。 |

## 时间线相册与可选增强

仅启用递归时间线相册，不启用可选增强：

```yaml
environment:
  LUMIFLOW_ALBUM_MODE: timeline
  LUMIFLOW_TIMELINE_TIMEZONE: Asia/Shanghai
  LUMIFLOW_CALENDAR_REGION: CN_COMMON
  LUMIFLOW_VISION_TAGGER: none
  LUMIFLOW_AI_ENABLED: "false"
```

时间线元数据和每日相册成员关系存放在 `LUMIFLOW_DATA_PATH/lumiflow.sqlite`；按 ID 的 WebP 缩略图位于 `thumbs/by-id/`；AI contact sheet 及其指纹位于 `ai/contact-sheets/`。再次扫描会复用未变化的 EXIF、缩略图、本地标签、contact sheet 和 AI 描述。

扫描、相册重建、缩略图、本地标签和 contact sheet 会在 rescan 响应前完成；远程 AI 请求随后在后台后处理任务中运行，因此缓慢或不可用的服务不会拖延启动、手动 rescan 或 watcher rescan。刷新相册列表即可看到新缓存的描述。

本地视觉需要使用 `--features vision-onnx` 构建二进制，并挂载只读资产：

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

图像编码器必须只有一个 float32 `[1,3,N,N]` 输入，以及一个 float32 `[1,D]` 或 `[D]` 向量输出。标签文件采用严格 JSON：

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

每个向量的实际维度必须与模型输出完全一致；上面的短向量仅用于说明 schema。

可选 AI 配置：

```yaml
environment:
  LUMIFLOW_AI_ENABLED: "true"
  LUMIFLOW_AI_PROVIDER: openai-compatible
  LUMIFLOW_AI_BASE_URL: https://api.openai.com/v1
  LUMIFLOW_AI_API_KEY: ${LUMIFLOW_AI_API_KEY}
  LUMIFLOW_AI_MODEL: gpt-5-mini
  LUMIFLOW_AI_DESCRIPTION_LANGUAGE: zh-CN
```

隐私边界：本地标签只读取缓存缩略图，数据不离开设备。只有启用 AI 时，LumiFlow 才会向配置的服务发送一张低分辨率 contact sheet JPEG 和相册元数据，绝不上传原图。AI 不能改相册名称或成员关系。视觉、缩略图、contact sheet 或 AI 失败不会阻止确定性相册和原图访问；后续 rescan 会重试过期工作。

## 照片目录结构

```text
/photos/
├── 2024-tokyo/
│   ├── DSC0001.jpg
│   └── DSC0002.heic
└── 2025-kyoto/
    └── IMG_0001.png
```

`folders` 模式只有一级子目录会成为相册，根目录照片会被忽略；`timeline` 模式会递归索引任意深度的受支持照片，但不同一级文件夹的照片绝不会合并到同一个生成相册中，按自然日分组只在各自文件夹内进行。根目录照片使用独立的根目录分组桶。

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
| `GET /api/thumbs/by-id/:photo_id` | 按稳定照片 ID 获取时间线缩略图。 |
| `GET /api/photos/by-id/:photo_id` | 按稳定照片 ID 获取时间线原图，支持 Range 和可选 `?download=1`。 |
| `GET /api/exif/by-id/:photo_id` | 按稳定照片 ID 获取时间线 EXIF。 |
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
