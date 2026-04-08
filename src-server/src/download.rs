use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use futures::StreamExt;
use thiserror::Error;
use tokio::sync::broadcast;
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
    #[error("Directory error: {0}")]
    DirectoryError(String),
    #[error("Server does not support range requests")]
    RangeNotSupported,
}

#[derive(Clone)]
pub struct ProgressSender {
    sender: broadcast::Sender<ProgressEvent>,
}

impl ProgressSender {
    pub fn new(sender: broadcast::Sender<ProgressEvent>) -> Self {
        Self { sender }
    }

    pub fn send(&self, event: ProgressEvent) {
        let _ = self.sender.send(event);
    }
}

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

    pub async fn get_expected_size(&self, url: &str) -> Option<u64> {
        let response = self.client.head(url).send().await.ok()?;
        response.content_length()
    }

    fn done_marker_path(zip_path: &str) -> String {
        format!("{}.done", zip_path)
    }

    fn is_download_complete(zip_path: &str) -> bool {
        let marker = Self::done_marker_path(zip_path);
        if Path::new(&marker).exists() && Path::new(zip_path).exists() {
            return true;
        }
        // Backfill marker for zips that existed before marker logic was introduced
        if let Ok(meta) = std::fs::metadata(zip_path) {
            if meta.len() > 0 {
                Self::mark_download_complete(zip_path);
                return true;
            }
        }
        false
    }

    fn mark_download_complete(zip_path: &str) {
        let _ = std::fs::write(Self::done_marker_path(zip_path), b"");
    }

    fn clear_download_marker(zip_path: &str) {
        let _ = std::fs::remove_file(Self::done_marker_path(zip_path));
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
                .map_err(|e| DownloadError::DirectoryError(e.to_string()))?;
        }

        if Self::is_download_complete(output_path) {
            let file_size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
            sender.send(ProgressEvent::Download(DownloadProgressEvent {
                package_name: package_name.to_string(),
                bytes_downloaded: file_size,
                total_bytes: file_size,
                percentage: 100.0,
                speed_bps: 0.0,
                eta_seconds: None,
                status: "already downloaded".to_string(),
            }));
            return Ok(());
        }

        let expected_size = self.get_expected_size(url).await.unwrap_or(0);
        let partial_size = match std::fs::metadata(output_path) {
            Ok(meta) => meta.len(),
            Err(_) => 0,
        };

        // Clear any stale marker (shouldn't exist, but be safe)
        Self::clear_download_marker(output_path);

        let result = if expected_size > 0 && partial_size > 0 && partial_size < expected_size {
            self.download_resume(
                url,
                output_path,
                package_name,
                sender,
                partial_size,
                expected_size,
            )
            .await
        } else {
            if partial_size > 0 {
                let _ = std::fs::remove_file(output_path);
            }
            self.download_fresh(url, output_path, package_name, sender)
                .await
        };

        if result.is_ok() {
            Self::mark_download_complete(output_path);
        }
        result
    }

    async fn download_fresh(
        &self,
        url: &str,
        output_path: &str,
        package_name: &str,
        sender: &ProgressSender,
    ) -> Result<(), DownloadError> {
        let response = self.client.get(url).send().await?;
        let total_bytes = response.content_length().unwrap_or(0);

        sender.send(ProgressEvent::Download(DownloadProgressEvent {
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
                let speed = if elapsed > 0.0 {
                    downloaded as f64 / elapsed
                } else {
                    0.0
                };
                let eta = if speed > 0.0 && total_bytes > downloaded {
                    Some(((total_bytes - downloaded) as f64 / speed) as u64)
                } else {
                    None
                };
                let percentage = if total_bytes > 0 {
                    (downloaded as f64 / total_bytes as f64) * 100.0
                } else {
                    0.0
                };

                sender.send(ProgressEvent::Download(DownloadProgressEvent {
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

        sender.send(ProgressEvent::Download(DownloadProgressEvent {
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

    async fn download_resume(
        &self,
        url: &str,
        output_path: &str,
        package_name: &str,
        sender: &ProgressSender,
        partial_size: u64,
        total_bytes: u64,
    ) -> Result<(), DownloadError> {
        let range_header = format!("bytes={}-", partial_size);
        let response = self
            .client
            .get(url)
            .header("Range", range_header)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DownloadError::RangeNotSupported);
        }

        sender.send(ProgressEvent::Download(DownloadProgressEvent {
            package_name: package_name.to_string(),
            bytes_downloaded: partial_size,
            total_bytes,
            percentage: if total_bytes > 0 {
                (partial_size as f64 / total_bytes as f64) * 100.0
            } else {
                0.0
            },
            speed_bps: 0.0,
            eta_seconds: None,
            status: "resuming".to_string(),
        }));

        let mut file = std::fs::OpenOptions::new().write(true).open(output_path)?;
        file.seek(SeekFrom::End(0))?;

        let mut downloaded = partial_size;
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
                let bytes_this_session = downloaded - partial_size;
                let speed = if elapsed > 0.0 {
                    bytes_this_session as f64 / elapsed
                } else {
                    0.0
                };
                let eta = if speed > 0.0 && total_bytes > downloaded {
                    Some(((total_bytes - downloaded) as f64 / speed) as u64)
                } else {
                    None
                };
                let percentage = if total_bytes > 0 {
                    (downloaded as f64 / total_bytes as f64) * 100.0
                } else {
                    0.0
                };

                sender.send(ProgressEvent::Download(DownloadProgressEvent {
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

        sender.send(ProgressEvent::Download(DownloadProgressEvent {
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

#[derive(Debug, Clone)]
pub struct ExtractedFiles {
    pub tiff_files: Vec<String>,
}

fn is_tiff_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            ext == "tif" || ext == "tiff" || ext == "img"
        })
        .unwrap_or(false)
}

fn collect_tiff_files(root: &Path) -> io::Result<Vec<String>> {
    let mut pending = vec![root.to_path_buf()];
    let mut tiff_files = Vec::new();

    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(path);
                continue;
            }
            if is_tiff_path(&path) {
                tiff_files.push(path.to_string_lossy().to_string());
            }
        }
    }

    tiff_files.sort();
    tiff_files.dedup();
    Ok(tiff_files)
}

fn normalize_tiff_files(tiff_files: Vec<String>) -> Vec<String> {
    let mut unique_files = tiff_files;
    unique_files.sort();
    unique_files.dedup();
    unique_files
}

pub fn check_extraction_complete(zip_path: &str, output_dir: &str) -> Option<ExtractedFiles> {
    let file = File::open(zip_path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;

    let mut tiff_files = Vec::new();

    for i in 0..archive.len() {
        let result = {
            let zip_file = archive.by_index(i).ok()?;
            let name = zip_file.name().to_string();

            if name.ends_with('/') {
                None
            } else {
                let outpath = match zip_file.enclosed_name() {
                    Some(path) => Path::new(output_dir).join(path),
                    None => return None,
                };

                if !outpath.exists() {
                    return None;
                }

                let expected_size = zip_file.size() as u64;
                let actual_size = std::fs::metadata(&outpath).ok()?.len();

                if actual_size != expected_size {
                    return None;
                }

                Some(outpath)
            }
        };

        if let Some(path) = result {
            if is_tiff_path(&path) {
                tiff_files.push(path.to_string_lossy().to_string());
            }
        }
    }

    if tiff_files.is_empty() {
        let scanned_files = collect_tiff_files(Path::new(output_dir)).ok()?;
        if scanned_files.is_empty() {
            return None;
        }
        return Some(ExtractedFiles {
            tiff_files: scanned_files,
        });
    }

    Some(ExtractedFiles {
        tiff_files: normalize_tiff_files(tiff_files),
    })
}

pub async fn extract_zip(
    zip_path: &str,
    output_dir: &str,
    package_name: &str,
    sender: &ProgressSender,
) -> Result<Vec<String>, DownloadError> {
    if let Some(extracted) = check_extraction_complete(zip_path, output_dir) {
        sender.send(ProgressEvent::Download(DownloadProgressEvent {
            package_name: package_name.to_string(),
            bytes_downloaded: 1,
            total_bytes: 1,
            percentage: 100.0,
            speed_bps: 0.0,
            eta_seconds: None,
            status: "already extracted".to_string(),
        }));
        return Ok(extracted.tiff_files);
    }

    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| DownloadError::ZipError(e.to_string()))?;
    std::fs::create_dir_all(output_dir)
        .map_err(|e| DownloadError::DirectoryError(e.to_string()))?;

    let mut extracted_files = Vec::new();
    let total_files = archive.len();
    let mut last_reported_percent = 0.0;

    sender.send(ProgressEvent::Download(DownloadProgressEvent {
        package_name: package_name.to_string(),
        bytes_downloaded: 0,
        total_bytes: total_files as u64,
        percentage: 0.0,
        speed_bps: 0.0,
        eta_seconds: None,
        status: "Extracting...".to_string(),
    }));

    for i in 0..total_files {
        {
            let mut file = archive
                .by_index(i)
                .map_err(|e| DownloadError::ZipError(e.to_string()))?;
            let outpath = match file.enclosed_name() {
                Some(path) => Path::new(output_dir).join(path),
                None => continue,
            };

            if file.name().ends_with('/') {
                std::fs::create_dir_all(&outpath)
                    .map_err(|e| DownloadError::DirectoryError(e.to_string()))?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        std::fs::create_dir_all(p)
                            .map_err(|e| DownloadError::DirectoryError(e.to_string()))?;
                    }
                }

                let expected_size = file.size() as u64;
                let needs_extraction = match std::fs::metadata(&outpath) {
                    Ok(meta) => meta.len() != expected_size,
                    Err(_) => true,
                };

                if needs_extraction {
                    let mut outfile = File::create(&outpath)?;
                    io::copy(&mut file, &mut outfile)?;
                }

                if is_tiff_path(&outpath) {
                    extracted_files.push(outpath.to_string_lossy().to_string());
                }
            }
        }

        let percentage = ((i + 1) as f64 / total_files as f64) * 100.0;

        if percentage - last_reported_percent >= 5.0 || i == total_files - 1 {
            sender.send(ProgressEvent::Download(DownloadProgressEvent {
                package_name: package_name.to_string(),
                bytes_downloaded: (i + 1) as u64,
                total_bytes: total_files as u64,
                percentage,
                speed_bps: 0.0,
                eta_seconds: None,
                status: "Extracting...".to_string(),
            }));
            last_reported_percent = percentage;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    let extracted_files = normalize_tiff_files(extracted_files);
    if !extracted_files.is_empty() {
        return Ok(extracted_files);
    }

    let scanned_files = collect_tiff_files(Path::new(output_dir))?;
    Ok(scanned_files)
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

    fn create_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{}-{}", prefix, unique));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_zip_with_entries(zip_path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();

        for (name, contents) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(contents).unwrap();
        }

        writer.finish().unwrap();
    }

    #[test]
    fn test_download_manager_creation() {
        let manager = DownloadManager::new();
        assert!(manager.client.get("https://example.com").build().is_ok());
    }

    #[test]
    fn test_is_download_complete() {
        assert!(!DownloadManager::is_download_complete("/nonexistent"));
    }

    #[test]
    fn test_check_extraction_complete_detects_uppercase_raster_files() {
        let temp_dir = create_temp_dir("dtm-download-check-extract");
        let zip_path = temp_dir.join("package.zip");
        let output_dir = temp_dir.join("extract");

        write_zip_with_entries(
            &zip_path,
            &[
                ("nested/TILE_001.IMG", b"fake-raster-data"),
                ("nested/readme.txt", b"metadata"),
            ],
        );

        let extracted_raster = output_dir.join("nested").join("TILE_001.IMG");
        std::fs::create_dir_all(extracted_raster.parent().unwrap()).unwrap();
        std::fs::write(&extracted_raster, b"fake-raster-data").unwrap();
        std::fs::write(output_dir.join("nested").join("readme.txt"), b"metadata").unwrap();

        let result =
            check_extraction_complete(zip_path.to_str().unwrap(), output_dir.to_str().unwrap())
                .unwrap();

        assert_eq!(
            result.tiff_files,
            vec![extracted_raster.to_string_lossy().to_string()]
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_extract_zip_returns_uppercase_cached_rasters() {
        let temp_dir = create_temp_dir("dtm-download-extract-zip");
        let zip_path = temp_dir.join("package.zip");
        let output_dir = temp_dir.join("extract");

        write_zip_with_entries(&zip_path, &[("tile/DTM_CACHE.IMG", b"cached-data")]);

        let extracted_raster = output_dir.join("tile").join("DTM_CACHE.IMG");
        std::fs::create_dir_all(extracted_raster.parent().unwrap()).unwrap();
        std::fs::write(&extracted_raster, b"cached-data").unwrap();

        let (tx, _) = broadcast::channel(8);
        let sender = ProgressSender::new(tx);
        let result = extract_zip(
            zip_path.to_str().unwrap(),
            output_dir.to_str().unwrap(),
            "Cached package",
            &sender,
        )
        .await
        .unwrap();

        assert_eq!(result, vec![extracted_raster.to_string_lossy().to_string()]);

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
