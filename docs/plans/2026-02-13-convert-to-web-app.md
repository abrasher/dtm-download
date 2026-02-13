# Convert Tauri App to Web Application

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Convert the Ontario DTM Downloader from a Tauri desktop app to a web application using Axum backend and SSE for real-time progress.

**Architecture:** 
- Backend: Rust/Axum web server exposing REST API endpoints for package queries, downloads, and processing
- Frontend: React SPA (unchanged structure) using fetch for API calls and EventSource for SSE progress updates
- Downloads streamed directly to browser instead of saved to local filesystem

**Tech Stack:** Axum, Tower, Tokio, SSE (axum/EventSource), React, Leaflet

---

## Task 1: Set Up Axum Backend Structure

**Files:**
- Create: `src-server/Cargo.toml`
- Create: `src-server/src/main.rs`
- Create: `src-server/src/lib.rs`

**Step 1: Create server directory and Cargo.toml**

```toml
[package]
name = "dtm-server"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "dtm-server"
path = "src/main.rs"

[lib]
path = "src/lib.rs"

[dependencies]
axum = { version = "0.8", features = ["macros"] }
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "fs"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json", "stream"] }
futures = "0.3"
thiserror = "2.0"
zip = "2.2"
tokio-util = { version = "0.7", features = ["io"] }
uuid = { version = "1", features = ["v4"] }
```

**Step 2: Create main.rs**

```rust
use dtm_server::create_router;

#[tokio::main]
async fn main() {
    let app = create_router();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Server running on http://localhost:3000");
    axum::serve(listener, app).await.unwrap();
}
```

**Step 3: Create lib.rs with router setup**

```rust
pub mod routes;
pub mod download;
pub mod processing;
pub mod package_client;
pub mod api_types;

use axum::{
    routing::{get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};

pub fn create_router() -> Router {
    Router::new()
        .route("/api/packages/query", post(routes::query_packages))
        .route("/api/download/start", post(routes::start_download))
        .route("/api/download/{id}/progress", get(routes::download_progress))
        .route("/api/download/{id}/file", get(routes::download_file))
        .route("/api/health", get(routes::health))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any))
}
```

**Step 4: Run cargo check**

Run: `cd src-server && cargo check`
Expected: Compiles successfully (may have warnings about unused modules)

**Step 5: Commit**

```bash
git add src-server/
git commit -m "feat: set up Axum server structure"
```

---

## Task 2: Migrate Shared Types (api_types, package_client)

**Files:**
- Copy: `src-tauri/src/api_types.rs` -> `src-server/src/api_types.rs`
- Copy: `src-tauri/src/package_client.rs` -> `src-server/src/package_client.rs`
- Modify: `src-server/src/api_types.rs` (add web-specific types)
- Modify: `src-server/src/package_client.rs` (remove crate reference)

**Step 1: Copy api_types.rs and add web-specific types**

Copy the file, then add at the bottom:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub packages: Vec<Package>,
    pub clip_extent: Option<ClipExtentRequest>,
    pub compression: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipExtentRequest {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadStartResponse {
    pub download_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgressEvent {
    pub package_name: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f64,
    pub speed_bps: f64,
    pub eta_seconds: Option<u64>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessingProgressEvent {
    pub stage: String,
    pub percentage: u8,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub enum ProgressEvent {
    Download(DownloadProgressEvent),
    Processing(ProcessingProgressEvent),
    Complete { output_filename: String },
    Error { message: String },
}
```

**Step 2: Update package_client.rs imports**

Change `crate::api_types` import to use local module:

```rust
use crate::api_types::{
    extract_download_url, ArcGISQueryResponse, BoundingBox,
    GeoJSONGeometry, Package,
};
```

**Step 3: Run cargo check**

Run: `cd src-server && cargo check`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src-server/src/api_types.rs src-server/src/package_client.rs
git commit -m "feat: migrate shared types to server"
```

---

## Task 3: Migrate Download Module (SSE-ready)

**Files:**
- Create: `src-server/src/download.rs`

**Step 1: Create download.rs with SSE broadcaster**

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::io::{self, Write};
use std::path::Path;
use std::fs::File;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{mpsc, RwLock};
use futures::StreamExt;
use serde::Serialize;
use thiserror::Error;
use zip::ZipArchive;

use crate::api_types::{DownloadProgressEvent, ProgressEvent};

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("ZIP extraction failed: {0}")]
    ZipError(String),
}

pub type ProgressSender = mpsc::UnboundedSender<ProgressEvent>;

pub struct DownloadManager {
    client: reqwest::Client,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("OntarioDTMDownloader/1.0")
                .tcp_keepalive(Duration::from_secs(30))
                .build()
                .unwrap(),
        }
    }

    pub async fn download_with_progress(
        &self,
        url: &str,
        output_path: &str,
        package_name: &str,
        sender: &ProgressSender,
    ) -> Result<(), DownloadError> {
        if let Some(parent) = Path::new(output_path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DownloadError::IoError(e))?;
        }

        let response = self.client.get(url).send().await?;
        let total_bytes = response.content_length().unwrap_or(0);

        let _ = sender.send(ProgressEvent::Download(DownloadProgressEvent {
            package_name: package_name.to_string(),
            bytes_downloaded: 0,
            total_bytes,
            percentage: 0.0,
            speed_bps: 0.0,
            eta_seconds: None,
            status: "downloading".to_string(),
        }));

        let mut file = File::create(output_path)?;
        let mut downloaded: u64 = 0;
        let start_time = Instant::now();
        let mut last_update = Instant::now();

        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;

            let now = Instant::now();
            if now.duration_since(last_update).as_millis() > 100 || downloaded == total_bytes {
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed = if elapsed > 0.0 { downloaded as f64 / elapsed } else { 0.0 };
                let eta = if speed > 0.0 && total_bytes > downloaded {
                    Some(((total_bytes - downloaded) as f64 / speed) as u64)
                } else { None };
                let percentage = if total_bytes > 0 { (downloaded as f64 / total_bytes as f64) * 100.0 } else { 0.0 };

                let _ = sender.send(ProgressEvent::Download(DownloadProgressEvent {
                    package_name: package_name.to_string(),
                    bytes_downloaded: downloaded,
                    total_bytes,
                    percentage,
                    speed_bps: speed,
                    eta_seconds: eta,
                    status: "downloading".to_string(),
                }));
                last_update = now;
            }
        }

        let _ = sender.send(ProgressEvent::Download(DownloadProgressEvent {
            package_name: package_name.to_string(),
            bytes_downloaded: downloaded,
            total_bytes: downloaded,
            percentage: 100.0,
            speed_bps: 0.0,
            eta_seconds: None,
            status: "completed".to_string(),
        }));

        Ok(())
    }
}

pub fn extract_zip(zip_path: &str, output_dir: &str, package_name: &str, sender: &ProgressSender) -> Result<Vec<String>, DownloadError> {
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| DownloadError::ZipError(e.to_string()))?;
    std::fs::create_dir_all(output_dir)?;

    let mut extracted_files = Vec::new();
    let total_files = archive.len();

    let _ = sender.send(ProgressEvent::Download(DownloadProgressEvent {
        package_name: package_name.to_string(),
        bytes_downloaded: 0,
        total_bytes: total_files as u64,
        percentage: 0.0,
        speed_bps: 0.0,
        eta_seconds: None,
        status: "Extracting...".to_string(),
    }));

    for i in 0..total_files {
        let mut file = archive.by_index(i).map_err(|e| DownloadError::ZipError(e.to_string()))?;
        let outpath = match file.enclosed_name() {
            Some(path) => Path::new(output_dir).join(path),
            None => continue,
        };

        if file.name().ends_with('/') {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() { std::fs::create_dir_all(p)?; }
            }
            let mut outfile = File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;

            if let Some(ext) = outpath.extension() {
                if ext == "tif" || ext == "tiff" {
                    extracted_files.push(outpath.to_string_lossy().to_string());
                }
            }
        }

        let percentage = ((i + 1) as f64 / total_files as f64) * 100.0;
        let _ = sender.send(ProgressEvent::Download(DownloadProgressEvent {
            package_name: package_name.to_string(),
            bytes_downloaded: (i + 1) as u64,
            total_bytes: total_files as u64,
            percentage,
            speed_bps: 0.0,
            eta_seconds: None,
            status: "Extracting...".to_string(),
        }));
    }

    Ok(extracted_files)
}

impl Default for DownloadManager {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_download_manager_creation() {
        let manager = DownloadManager::new();
        assert!(manager.client.get("https://example.com").build().is_ok());
    }
}
```

**Step 2: Run cargo check**

Run: `cd src-server && cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src-server/src/download.rs
git commit -m "feat: add download module with SSE support"
```

---

## Task 4: Migrate Processing Module

**Files:**
- Create: `src-server/src/processing.rs`

**Step 1: Create processing.rs**

```rust
use std::process::Command;
use std::io;
use thiserror::Error;
use crate::api_types::{ProgressEvent, ProcessingProgressEvent, ProgressSender};

#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("GDAL not found: {0}")]
    GdalNotFound(String),
    #[error("GDAL operation failed: {0}")]
    GdalError(String),
    #[error("No input files provided")]
    NoInputFiles,
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
}

#[derive(Debug, Clone, Copy)]
pub enum CompressionType {
    Zstd, Lzma, Deflate, Lzw,
}

impl CompressionType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "zstd" => CompressionType::Zstd,
            "lzma" => CompressionType::Lzma,
            "lzw" => CompressionType::Lzw,
            _ => CompressionType::Deflate,
        }
    }
    pub fn to_gdal_string(&self) -> &'static str {
        match self {
            CompressionType::Zstd => "ZSTD",
            CompressionType::Lzma => "LZMA",
            CompressionType::Deflate => "DEFLATE",
            CompressionType::Lzw => "LZW",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClipExtent {
    pub min_x: f64, pub min_y: f64, pub max_x: f64, pub max_y: f64,
}

pub async fn merge_to_cog(
    input_files: &[String],
    output_path: &str,
    clip_extent: Option<ClipExtent>,
    compression: CompressionType,
    sender: &ProgressSender,
) -> Result<(), ProcessingError> {
    if input_files.is_empty() {
        return Err(ProcessingError::NoInputFiles);
    }

    let _ = sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "merging".to_string(),
        percentage: 0,
        message: "Starting merge process...".to_string(),
    }));

    let compress_opt = format!("COMPRESS={}", compression.to_gdal_string());
    let temp_path = format!("{}.temp.tif", output_path.trim_end_matches(".tif"));

    let _ = sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "merging".to_string(),
        percentage: 10,
        message: "Merging and clipping rasters...".to_string(),
    }));

    let mut warp_cmd = Command::new("gdalwarp");
    warp_cmd
        .arg("-of").arg("GTiff")
        .arg("-co").arg(&compress_opt)
        .arg("-co").arg("BIGTIFF=YES")
        .arg("-co").arg("NUM_THREADS=ALL_CPUS")
        .arg("-r").arg("near");

    if let Some(extent) = clip_extent {
        warp_cmd
            .arg("-te")
            .arg(extent.min_x.to_string())
            .arg(extent.min_y.to_string())
            .arg(extent.max_x.to_string())
            .arg(extent.max_y.to_string())
            .arg("-te_srs").arg("EPSG:3857");
    }

    for file in input_files { warp_cmd.arg(file); }
    warp_cmd.arg(&temp_path);

    let warp_output = warp_cmd.output()?;
    if !warp_output.status.success() {
        let stderr = String::from_utf8_lossy(&warp_output.stderr);
        return Err(ProcessingError::GdalError(format!("gdalwarp failed: {}", stderr)));
    }

    let _ = sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "creating_cog".to_string(),
        percentage: 60,
        message: "Creating Cloud Optimized GeoTIFF...".to_string(),
    }));

    let translate_output = Command::new("gdal_translate")
        .arg(&temp_path)
        .arg(output_path)
        .arg("-of").arg("COG")
        .arg("-co").arg(&compress_opt)
        .arg("-co").arg("BIGTIFF=YES")
        .arg("-co").arg("BLOCKSIZE=512")
        .arg("-co").arg("NUM_THREADS=ALL_CPUS")
        .output()?;

    if !translate_output.status.success() {
        let stderr = String::from_utf8_lossy(&translate_output.stderr);
        return Err(ProcessingError::GdalError(format!("gdal_translate failed: {}", stderr)));
    }

    let _ = std::fs::remove_file(&temp_path);

    let _ = sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "completed".to_string(),
        percentage: 100,
        message: "Processing complete!".to_string(),
    }));

    Ok(())
}

pub fn check_gdal_available() -> Result<String, ProcessingError> {
    let output = Command::new("gdalinfo")
        .arg("--version")
        .output()
        .map_err(|e| ProcessingError::GdalNotFound(e.to_string()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(ProcessingError::GdalNotFound("gdalinfo failed".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_compression_types() {
        assert_eq!(CompressionType::Zstd.to_gdal_string(), "ZSTD");
        assert_eq!(CompressionType::Deflate.to_gdal_string(), "DEFLATE");
    }
}
```

**Step 2: Run cargo check**

Run: `cd src-server && cargo check`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src-server/src/processing.rs
git commit -m "feat: add processing module for web server"
```

---

## Task 5: Create API Routes

**Files:**
- Create: `src-server/src/routes.rs`
- Modify: `src-server/src/lib.rs` (add state management)

**Step 1: Create routes.rs with all endpoints**

```rust
use axum::{
    extract::{Path, State},
    response::{sse::Event, IntoResponse, Sse},
    Json,
};
use std::sync::Arc;
use std::convert::Infallible;
use tokio::sync::{mpsc, RwLock};
use futures::stream::{self, Stream};
use serde::Deserialize;

use crate::api_types::{
    QueryRequest, QueryResult, DownloadRequest, DownloadStartResponse,
    ProgressEvent, Package,
};
use crate::package_client::PackageClient;
use crate::download::{DownloadManager, extract_zip, ProgressSender};
use crate::processing::{merge_to_cog, CompressionType, ClipExtent};

pub type DownloadState = Arc<RwLock<Option<DownloadJob>>>;

pub struct DownloadJob {
    pub output_path: String,
    pub filename: String,
}

pub struct AppState {
    pub downloads: std::collections::HashMap<String, DownloadState>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            downloads: std::collections::HashMap::new(),
        }
    }
}

pub async fn health() -> &'static str {
    "OK"
}

pub async fn query_packages(
    Json(req): Json<QueryRequest>,
) -> Result<Json<QueryResult>, String> {
    let client = PackageClient::new();
    let bbox = crate::api_types::BoundingBox::new(req.min_x, req.min_y, req.max_x, req.max_y, 3857);
    
    let packages = client.query_by_extent(&bbox).await.map_err(|e| e.to_string())?;
    
    let mut projects: Vec<String> = packages.iter().map(|p| p.project.clone()).collect();
    projects.sort();
    projects.dedup();
    
    let total_size_gb: f64 = packages.iter().map(|p| p.size_gb).sum();
    
    Ok(Json(QueryResult { packages, projects, total_size_gb }))
}

pub async fn start_download(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<DownloadRequest>,
) -> Result<Json<DownloadStartResponse>, String> {
    let download_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join("dtm-downloads").join(&download_id);
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    
    let output_filename = format!("dtm_output_{}.tif", &download_id[..8]);
    let output_path = temp_dir.join(&output_filename).to_string_lossy().to_string();
    
    let job_state: DownloadState = Arc::new(RwLock::new(Some(DownloadJob {
        output_path: output_path.clone(),
        filename: output_filename.clone(),
    })));
    
    {
        let mut state = state.write().await;
        state.downloads.insert(download_id.clone(), job_state.clone());
    }
    
    let (tx, _rx) = mpsc::unbounded_channel::<ProgressEvent>();
    
    tokio::spawn(async move {
        if let Err(e) = run_download_job(req, temp_dir.to_string_lossy().to_string(), output_path, output_filename, tx).await {
            eprintln!("Download job error: {}", e);
        }
    });
    
    Ok(Json(DownloadStartResponse { download_id }))
}

async fn run_download_job(
    req: DownloadRequest,
    temp_dir: String,
    output_path: String,
    output_filename: String,
    sender: ProgressSender,
) -> Result<(), String> {
    let manager = DownloadManager::new();
    let mut all_tiff_files = Vec::new();
    
    for pkg in &req.packages {
        let zip_path = format!("{}/{}.zip", temp_dir, pkg.package_name.replace([' ', '/'], "_"));
        
        manager.download_with_progress(&pkg.download_url, &zip_path, &pkg.package_name, &sender)
            .await.map_err(|e| e.to_string())?;
        
        let tiff_files = extract_zip(&zip_path, &temp_dir, &pkg.package_name, &sender)
            .map_err(|e| e.to_string())?;
        all_tiff_files.extend(tiff_files);
    }
    
    let clip_extent = req.clip_extent.map(|c| ClipExtent {
        min_x: c.min_x, min_y: c.min_y, max_x: c.max_x, max_y: c.max_y,
    });
    let compression = CompressionType::from_str(&req.compression);
    
    merge_to_cog(&all_tiff_files, &output_path, clip_extent, compression, &sender)
        .await.map_err(|e| e.to_string())?;
    
    let _ = sender.send(ProgressEvent::Complete { output_filename });
    
    Ok(())
}

pub async fn download_progress(
    Path(id): Path<String>,
    State(state): State<Arc<RwLock<AppState>>>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, String> {
    let job_state = {
        let state = state.read().await;
        state.downloads.get(&id).cloned()
            .ok_or_else(|| "Download not found".to_string())?
    };
    
    let stream = async_stream::stream! {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Ok(Event::default().data("ping")) = Event::default().data("ping") {
                yield Ok(Event::default().data("ping"));
            }
        }
    };
    
    Ok(Sse::new(stream))
}

pub async fn download_file(
    Path(id): Path<String>,
    State(state): State<Arc<RwLock<AppState>>>,
) -> Result<impl IntoResponse, String> {
    let job_state = {
        let state = state.read().await;
        state.downloads.get(&id).cloned()
            .ok_or_else(|| "Download not found".to_string())?
    };
    
    let job = job_state.read().await;
    let job = job.as_ref().ok_or_else(|| "Job not found".to_string())?;
    
    let file = tokio::fs::File::open(&job.output_path).await
        .map_err(|e| e.to_string())?;
    let stream = tokio_util::io::ReaderStream::new(file);
    
    Ok(axum::response::Response::builder()
        .header("Content-Type", "image/tiff")
        .header("Content-Disposition", format!("attachment; filename=\"{}\"", job.filename))
        .body(axum::body::Body::from_stream(stream))
        .map_err(|e| e.to_string())?)
}
```

**Step 2: Update lib.rs with state**

```rust
pub mod routes;
pub mod download;
pub mod processing;
pub mod package_client;
pub mod api_types;

use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

pub fn create_router() -> Router {
    let state = Arc::new(RwLock::new(routes::AppState::new()));
    
    Router::new()
        .route("/api/packages/query", post(routes::query_packages))
        .route("/api/download/start", post(routes::start_download))
        .route("/api/download/{id}/progress", get(routes::download_progress))
        .route("/api/download/{id}/file", get(routes::download_file))
        .route("/api/health", get(routes::health))
        .with_state(state)
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any))
}
```

**Step 3: Add async_stream dependency to Cargo.toml**

Add to dependencies:
```toml
async-stream = "0.3"
```

**Step 4: Run cargo check**

Run: `cd src-server && cargo check`
Expected: Compiles successfully

**Step 5: Commit**

```bash
git add src-server/src/routes.rs src-server/src/lib.rs src-server/Cargo.toml
git commit -m "feat: add API routes with SSE progress"
```

---

## Task 6: Update Frontend to Use Fetch API

**Files:**
- Modify: `src/App.tsx`
- Modify: `package.json`

**Step 1: Remove Tauri dependencies from package.json**

Remove:
```json
"@tauri-apps/api": "^2",
"@tauri-apps/plugin-dialog": "^2.6.0",
"@tauri-apps/plugin-fs": "^2.4.5",
"@tauri-apps/plugin-http": "^2.5.7",
"@tauri-apps/plugin-opener": "^2",
"@tauri-apps/plugin-shell": "^2.3.5"
```

Also remove `@tauri-apps/cli` from devDependencies.

**Step 2: Rewrite App.tsx imports and API calls**

Replace Tauri imports with fetch-based API calls. Key changes:
- Remove `import { invoke } from '@tauri-apps/api/core'`
- Remove `import { listen } from '@tauri-apps/api/event'`
- Remove `import { open, save } from '@tauri-apps/plugin-dialog'`
- Add `const API_BASE = 'http://localhost:3000/api'`
- Replace `invoke()` with `fetch()`
- Replace `listen()` with `EventSource`
- Remove file dialog code, use browser download

**Step 3: Implement fetch-based query_packages**

```typescript
const result = await fetch(`${API_BASE}/packages/query`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ min_x: minX, min_y: minY, max_x: maxX, max_y: maxY }),
}).then(r => r.json());
```

**Step 4: Implement SSE for progress updates**

```typescript
const eventSource = new EventSource(`${API_BASE}/download/${downloadId}/progress`);
eventSource.onmessage = (event) => {
  const progress = JSON.parse(event.data);
  // Update UI
};
```

**Step 5: Implement file download**

```typescript
const response = await fetch(`${API_BASE}/download/${downloadId}/file`);
const blob = await response.blob();
const url = URL.createObjectURL(blob);
const a = document.createElement('a');
a.href = url;
a.download = 'dtm_output.tif';
a.click();
```

**Step 6: Run npm install and build**

Run: `npm install && npm run build`
Expected: Builds successfully

**Step 7: Commit**

```bash
git add src/App.tsx package.json package-lock.json
git commit -m "feat: convert frontend to use fetch API"
```

---

## Task 7: Add Development Scripts

**Files:**
- Modify: `package.json`

**Step 1: Add concurrent dev script**

```json
{
  "scripts": {
    "dev": "vite",
    "dev:server": "cd src-server && cargo run",
    "dev:all": "concurrently \"npm run dev\" \"npm run dev:server\"",
    "build": "tsc && vite build",
    "build:server": "cd src-server && cargo build --release"
  }
}
```

**Step 2: Add concurrently dependency**

```bash
npm install -D concurrently
```

**Step 3: Test dev:all**

Run: `npm run dev:all`
Expected: Both Vite and Axum server start

**Step 4: Commit**

```bash
git add package.json package-lock.json
git commit -m "feat: add development scripts"
```

---

## Task 8: Update Vite Config for API Proxy

**Files:**
- Modify: `vite.config.ts`

**Step 1: Add proxy configuration**

```typescript
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      }
    }
  }
})
```

**Step 2: Update App.tsx to use relative API paths**

Change `const API_BASE = 'http://localhost:3000/api'` to `const API_BASE = '/api'`

**Step 3: Test proxy**

Run: `npm run dev`
Expected: API calls proxied through Vite dev server

**Step 4: Commit**

```bash
git add vite.config.ts src/App.tsx
git commit -m "feat: add vite proxy for API"
```

---

## Task 9: Remove Tauri Files

**Files:**
- Delete: `src-tauri/` directory
- Delete: `src-tauri/` from any references

**Step 1: Remove src-tauri directory**

```bash
rm -rf src-tauri/
```

**Step 2: Verify build still works**

Run: `npm run build`
Expected: Builds successfully without Tauri

**Step 3: Commit**

```bash
git add -A
git commit -m "chore: remove Tauri desktop app code"
```

---

## Task 10: Add Tests for Server

**Files:**
- Create: `src-server/tests/api_tests.rs`

**Step 1: Create test file**

```rust
use dtm_server::api_types::{QueryRequest, BoundingBox};

#[tokio::test]
async fn test_bounding_box_creation() {
    let bbox = BoundingBox::new(0.0, 0.0, 1.0, 1.0, 3857);
    assert_eq!(bbox.xmin, 0.0);
    assert_eq!(bbox.srid, 3857);
}

#[tokio::test]
async fn test_query_request_serialization() {
    let req = QueryRequest {
        min_x: -9321521.0,
        min_y: 6371205.0,
        max_x: -9284703.0,
        max_y: 6408629.0,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("min_x"));
}
```

**Step 2: Run tests**

Run: `cd src-server && cargo test`
Expected: Tests pass

**Step 3: Commit**

```bash
git add src-server/tests/
git commit -m "test: add server unit tests"
```

---

## Task 11: Update Documentation

**Files:**
- Modify: `README.md`

**Step 1: Update README with new instructions**

Replace Tauri-specific instructions with web app instructions:
- How to run server: `cd src-server && cargo run`
- How to run frontend: `npm run dev`
- How to run both: `npm run dev:all`
- How to build for production

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: update README for web app"
```
