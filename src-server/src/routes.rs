use axum::{
    extract::{Path, State},
    response::{sse::Event, IntoResponse, Sse},
    Json,
};
use std::collections::HashMap;
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::Stream;
use tokio::sync::{broadcast, RwLock};

use crate::api_types::{
    DownloadRequest, DownloadStartResponse, Package, ProgressEvent, QueryRequest, QueryResult,
};
use crate::download::{extract_zip, DownloadManager, ProgressSender};
use crate::package_client::PackageClient;
use crate::processing::{merge_to_cog, ClipExtent, CompressionType};

pub struct DownloadJob {
    pub output_path: String,
    pub filename: String,
    pub sender: broadcast::Sender<ProgressEvent>,
}

pub struct AppState {
    pub downloads: HashMap<String, Arc<RwLock<Option<DownloadJob>>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            downloads: HashMap::new(),
        }
    }
}

pub async fn health() -> &'static str {
    "OK"
}

pub async fn query_packages(Json(req): Json<QueryRequest>) -> Result<Json<QueryResult>, String> {
    println!(
        "Query request: min_x={}, min_y={}, max_x={}, max_y={}",
        req.min_x, req.min_y, req.max_x, req.max_y
    );

    let client = PackageClient::new();
    let bbox = crate::api_types::BoundingBox::new(req.min_x, req.min_y, req.max_x, req.max_y, 3857);

    let packages = client.query_by_extent(&bbox).await.map_err(|e| {
        eprintln!("Query error: {}", e);
        format!("Failed to query ArcGIS API: {}", e)
    })?;

    println!("Found {} packages", packages.len());

    let mut projects: Vec<String> = packages.iter().map(|p| p.project.clone()).collect();
    projects.sort();
    projects.dedup();

    let total_size_gb: f64 = packages.iter().map(|p| p.size_gb).sum();

    Ok(Json(QueryResult {
        packages,
        projects,
        total_size_gb,
    }))
}

pub async fn start_download(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<DownloadRequest>,
) -> Result<Json<DownloadStartResponse>, String> {
    let download_id = uuid::Uuid::new_v4().to_string();
    let work_dir = std::env::temp_dir()
        .join("dtm-downloads")
        .join(&download_id);
    std::fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;
    let cache_root = cache_root_dir();
    let zip_cache_dir = cache_root.join("zips");
    let extract_cache_dir = cache_root.join("extracts");
    std::fs::create_dir_all(&zip_cache_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&extract_cache_dir).map_err(|e| e.to_string())?;

    let output_filename = format!("dtm_output_{}.tif", &download_id[..8]);
    let output_path = work_dir
        .join(&output_filename)
        .to_string_lossy()
        .to_string();

    let (tx, _) = broadcast::channel::<ProgressEvent>(64);

    let job = DownloadJob {
        output_path: output_path.clone(),
        filename: output_filename.clone(),
        sender: tx.clone(),
    };

    let job_state: Arc<RwLock<Option<DownloadJob>>> = Arc::new(RwLock::new(Some(job)));

    {
        let mut state = state.write().await;
        state
            .downloads
            .insert(download_id.clone(), job_state.clone());
    }

    let packages = req.packages.clone();
    let clip_extent = req.clip_extent.clone();
    let compression = req.compression.clone();
    let zip_cache_dir_str = zip_cache_dir.to_string_lossy().to_string();
    let extract_cache_dir_str = extract_cache_dir.to_string_lossy().to_string();

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Err(e) = run_download_job(
            packages,
            zip_cache_dir_str,
            extract_cache_dir_str,
            output_path,
            clip_extent,
            compression,
            tx,
        )
        .await
        {
            eprintln!("Download job error: {}", e);
        }
    });

    Ok(Json(DownloadStartResponse { download_id }))
}

async fn run_download_job(
    packages: Vec<Package>,
    zip_cache_dir: String,
    extract_cache_dir: String,
    output_path: String,
    clip_extent: Option<crate::api_types::ClipExtentRequest>,
    compression: String,
    sender: broadcast::Sender<ProgressEvent>,
) -> Result<(), String> {
    let progress_sender = ProgressSender::new(sender.clone());

    let manager = DownloadManager::new();
    let mut all_tiff_files = Vec::new();

    for pkg in &packages {
        let cache_key = package_cache_key(pkg);
        let zip_path = format!("{}/{}.zip", zip_cache_dir, cache_key);
        let extract_dir = format!("{}/{}", extract_cache_dir, cache_key);

        manager
            .download_with_progress(
                &pkg.download_url,
                &zip_path,
                &pkg.package_name,
                &progress_sender,
            )
            .await
            .map_err(|e| e.to_string())?;

        let tiff_files = extract_zip(&zip_path, &extract_dir, &pkg.package_name, &progress_sender)
            .await
            .map_err(|e| e.to_string())?;
        all_tiff_files.extend(tiff_files);
    }

    let clip = clip_extent.map(|c| ClipExtent {
        min_x: c.min_x,
        min_y: c.min_y,
        max_x: c.max_x,
        max_y: c.max_y,
    });
    let comp = CompressionType::from_str(&compression);

    merge_to_cog(&all_tiff_files, &output_path, clip, comp, &progress_sender)
        .await
        .map_err(|e| e.to_string())?;

    let _ = sender.send(ProgressEvent::Complete {
        output_filename: "dtm_output.tif".to_string(),
    });

    Ok(())
}

fn cache_root_dir() -> PathBuf {
    if let Ok(cache_dir) = std::env::var("DTM_CACHE_DIR") {
        let trimmed = cache_dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    if let Ok(home_dir) = std::env::var("HOME") {
        #[cfg(target_os = "macos")]
        {
            return PathBuf::from(home_dir)
                .join("Library")
                .join("Caches")
                .join("dtm-download");
        }
        #[cfg(not(target_os = "macos"))]
        {
            return PathBuf::from(home_dir).join(".cache").join("dtm-download");
        }
    }

    std::env::temp_dir().join("dtm-download-cache")
}

fn sanitize_for_path(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn package_cache_key(pkg: &Package) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    pkg.download_url.hash(&mut hasher);
    let url_hash = hasher.finish();
    let package_name = sanitize_for_path(&pkg.package_name);
    format!("{}_{:016x}", package_name, url_hash)
}

pub async fn download_progress(
    Path(id): Path<String>,
    State(state): State<Arc<RwLock<AppState>>>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, String> {
    let sender = {
        let state = state.read().await;
        let job_state = state
            .downloads
            .get(&id)
            .ok_or_else(|| "Download not found".to_string())?;
        let job = job_state.read().await;
        job.as_ref()
            .map(|j| j.sender.clone())
            .ok_or_else(|| "Job not found".to_string())?
    };

    let mut rx = sender.subscribe();

    let stream = async_stream::stream! {
        while let Ok(event) = rx.recv().await {
            let json = serde_json::to_string(&event).unwrap_or_default();
            yield Ok(Event::default().data(json));
        }
    };

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    ))
}

pub async fn download_file(
    Path(id): Path<String>,
    State(state): State<Arc<RwLock<AppState>>>,
) -> Result<impl IntoResponse, String> {
    let job_state = {
        let state = state.read().await;
        state
            .downloads
            .get(&id)
            .cloned()
            .ok_or_else(|| "Download not found".to_string())?
    };

    let (output_path, filename) = {
        let job = job_state.read().await;
        let j = job.as_ref().ok_or_else(|| "Job not found".to_string())?;
        (j.output_path.clone(), j.filename.clone())
    };

    let file = tokio::fs::File::open(&output_path)
        .await
        .map_err(|e| e.to_string())?;
    let stream = tokio_util::io::ReaderStream::new(file);

    Ok(axum::response::Response::builder()
        .header("Content-Type", "image/tiff")
        .header(
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(axum::body::Body::from_stream(stream))
        .map_err(|e| e.to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_package(package_name: &str, download_url: &str) -> Package {
        Package {
            package_name: package_name.to_string(),
            size_gb: 1.0,
            resolution: 0.5,
            download_url: download_url.to_string(),
            project: "Test Project".to_string(),
            year_range: Some("2023".to_string()),
            coverage_km2: 1.0,
            geometry: crate::api_types::GeoJSONGeometry::Polygon(vec![]),
        }
    }

    #[test]
    fn test_package_cache_key_changes_with_url() {
        let pkg_a = test_package("GTA A", "https://example.com/a.zip");
        let pkg_b = test_package("GTA A", "https://example.com/b.zip");
        assert_ne!(package_cache_key(&pkg_a), package_cache_key(&pkg_b));
    }

    #[test]
    fn test_package_cache_key_is_stable_for_same_input() {
        let pkg = test_package("GTA / 2023", "https://example.com/a.zip");
        assert_eq!(package_cache_key(&pkg), package_cache_key(&pkg));
    }

    #[test]
    fn test_sanitize_for_path_replaces_separators() {
        assert_eq!(sanitize_for_path("A/B C"), "A_B_C");
    }

    #[test]
    fn test_cache_root_dir_uses_override() {
        let original = std::env::var("DTM_CACHE_DIR").ok();
        std::env::set_var("DTM_CACHE_DIR", "/tmp/dtm-cache-override");
        assert_eq!(cache_root_dir(), PathBuf::from("/tmp/dtm-cache-override"));
        if let Some(value) = original {
            std::env::set_var("DTM_CACHE_DIR", value);
        } else {
            std::env::remove_var("DTM_CACHE_DIR");
        }
    }
}
