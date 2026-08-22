# Ontario DTM Download

i made an easy webui to download ontario dtm for a project area cause the government hasn't

Web app for downloading LiDAR-derived Ontario DTM tiles, clipping to your selected extent, and exporting a Cloud Optimized GeoTIFF.

## Host With Docker (GHCR Image)

This repository automatically publishes a Docker image to GitHub Container Registry on every push to `main` and on tags matching `v*`.

Image URL format:

```bash
ghcr.io/<github-owner>/<repo>:latest
```

For this repo that is typically:

```bash
ghcr.io/<your-github-user>/dtm-download:latest
```

After the first publish, set the package visibility to `Public` in GitHub Packages so anyone can pull and deploy it.

### Docker Compose

The included Compose configuration stores downloaded ZIP files, extracted rasters, and generated outputs under `/mnt/mergerfs/dtms` on the host. Its settings are read from `.env`:

```dotenv
DTM_STORAGE_PATH=/mnt/mergerfs/dtms
```

Create the storage directory and start the app:

```bash
mkdir -p /mnt/mergerfs/dtms
docker compose up -d
```

Compose builds the image from the Dockerfile in the current checkout. To rebuild after making changes:

```bash
docker compose up -d --build
```

Docker assigns an available host port by default, which allows parallel development instances to run without port or container-name collisions. Find the assigned port with:

```bash
docker compose port app 3000
```

To use a fixed port, add `DTM_PORT=3000` (or another port) to `.env`. Each parallel checkout can also set its own `COMPOSE_PROJECT_NAME` if its directory name is not unique.

### Docker CLI

#### 1. Pull the image

```bash
docker pull ghcr.io/<your-github-user>/dtm-download:latest
```

#### 2. Run the container

```bash
docker run -d \
  --name ontario-dtm-download \
  -p 3000:3000 \
  -e DTM_CACHE_DIR=/data/dtms \
  -v /mnt/mergerfs/dtms:/data/dtms \
  ghcr.io/abrasher/dtm-download:latest
```

#### 3. Open the app

Go to:

```text
http://localhost:3000
```

### Optional environment variable

- `DTM_CACHE_DIR`: path inside the container for cached ZIP/extracted files. Default: `/var/cache/ontario-dtm-download`

## Local Development

### Prerequisites

- Node.js 20+
- Rust 1.88+
- GDAL

### Run frontend + backend

```bash
npm install
npm run dev:all
```

### Build locally

```bash
npm run build
cd src-server && cargo build --release
```
