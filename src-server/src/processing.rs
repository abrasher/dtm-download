use crate::api_types::{ProcessingProgressEvent, ProgressEvent};
use crate::download::ProgressSender;
use futures::stream::{self, StreamExt};
use serde_json::Value;
use std::io;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("GDAL not found: {0}")]
    GdalNotFound(String),
    #[error("GDAL operation failed: {0}")]
    GdalError(String),
    #[error("No input files provided")]
    NoInputFiles,
    #[error("No source rasters intersect the selected area")]
    NoIntersectingInputFiles,
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
}

#[derive(Debug, Clone, Copy)]
pub enum CompressionType {
    Zstd,
    Lzma,
    Deflate,
    Lzw,
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
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RasterBounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl RasterBounds {
    fn intersects(&self, other: RasterBounds) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }
}

impl ClipExtent {
    fn to_wgs84_bounds(self) -> RasterBounds {
        let (min_lon, min_lat) = web_mercator_to_wgs84(self.min_x, self.min_y);
        let (max_lon, max_lat) = web_mercator_to_wgs84(self.max_x, self.max_y);
        RasterBounds {
            min_x: min_lon.min(max_lon),
            min_y: min_lat.min(max_lat),
            max_x: min_lon.max(max_lon),
            max_y: min_lat.max(max_lat),
        }
    }
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

    let selected_input_files = if let Some(extent) = clip_extent {
        filter_input_files_by_extent(input_files, extent, sender).await?
    } else {
        input_files.to_vec()
    };

    if selected_input_files.is_empty() {
        return Err(ProcessingError::NoIntersectingInputFiles);
    }

    let compress_opt = format!("COMPRESS={}", compression.to_gdal_string());
    let predictor_opt = detect_predictor_option(selected_input_files.first().map(|s| s.as_str()))
        .await
        .map(|p| format!("PREDICTOR={}", p));
    let vrt_path = format!("{}.selected.vrt", output_path.trim_end_matches(".tif"));
    let temp_path = format!("{}.temp.tif", output_path.trim_end_matches(".tif"));

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "preparing_inputs".to_string(),
        percentage: 5,
        message: format!(
            "Preparing {} raster(s) for processing...",
            selected_input_files.len()
        ),
    }));

    let warp_stage = if clip_extent.is_some() {
        "clipping"
    } else {
        "merging"
    };
    let warp_message = if clip_extent.is_some() {
        "Clipping selected area..."
    } else {
        "Merging source rasters..."
    };

    let mut vrt_cmd = Command::new("gdalbuildvrt");
    vrt_cmd.arg("-overwrite").arg(&vrt_path);
    for file in &selected_input_files {
        vrt_cmd.arg(file);
    }
    run_gdal_command_with_progress(
        vrt_cmd,
        "gdalbuildvrt",
        "building_vrt",
        10,
        14,
        "Building a virtual mosaic from the selected source rasters...",
        "gdalbuildvrt finished. Starting the final raster assembly...",
        Some(&vrt_path),
        sender,
    )
    .await?;

    let mut warp_cmd = Command::new("gdalwarp");
    warp_cmd
        .arg("-of")
        .arg("GTiff")
        .arg("-co")
        .arg(&compress_opt)
        .arg("-co")
        .arg("BIGTIFF=YES")
        .arg("-co")
        .arg("NUM_THREADS=ALL_CPUS")
        .arg("-r")
        .arg("near");
    if let Some(predictor) = &predictor_opt {
        warp_cmd.arg("-co").arg(predictor);
    }

    if let Some(extent) = clip_extent {
        warp_cmd
            .arg("-te")
            .arg(extent.min_x.to_string())
            .arg(extent.min_y.to_string())
            .arg(extent.max_x.to_string())
            .arg(extent.max_y.to_string())
            .arg("-te_srs")
            .arg("EPSG:3857");
    }

    warp_cmd.arg(&vrt_path).arg(&temp_path);
    run_gdal_command_with_progress(
        warp_cmd,
        "gdalwarp",
        warp_stage,
        15,
        74,
        warp_message,
        "gdalwarp finished. Preparing intermediate raster for Cloud Optimized GeoTIFF creation...",
        Some(&temp_path),
        sender,
    )
    .await?;

    let mut translate_cmd = Command::new("gdal_translate");
    translate_cmd
        .arg(&temp_path)
        .arg(output_path)
        .arg("-of")
        .arg("COG")
        .arg("-co")
        .arg(&compress_opt);
    if let Some(predictor) = &predictor_opt {
        translate_cmd.arg("-co").arg(predictor);
    }
    translate_cmd
        .arg("-co")
        .arg("BIGTIFF=YES")
        .arg("-co")
        .arg("BLOCKSIZE=512")
        .arg("-co")
        .arg("NUM_THREADS=ALL_CPUS");
    run_gdal_command_with_progress(
        translate_cmd,
        "gdal_translate",
        "creating_cog",
        75,
        97,
        "Creating Cloud Optimized GeoTIFF...",
        "gdal_translate finished. Finalizing Cloud Optimized GeoTIFF output...",
        Some(output_path),
        sender,
    )
    .await?;

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "finalizing".to_string(),
        percentage: 99,
        message: "Finalizing output and preparing download...".to_string(),
    }));

    let _ = std::fs::remove_file(&temp_path);
    let _ = std::fs::remove_file(&vrt_path);

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "completed".to_string(),
        percentage: 100,
        message: "Processing complete!".to_string(),
    }));

    Ok(())
}

async fn filter_input_files_by_extent(
    input_files: &[String],
    clip_extent: ClipExtent,
    sender: &ProgressSender,
) -> Result<Vec<String>, ProcessingError> {
    let total = input_files.len();
    let clip_bounds_wgs84 = clip_extent.to_wgs84_bounds();
    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "preparing_inputs".to_string(),
        percentage: 5,
        message: format!(
            "Inspecting {} raster(s) to match the selected area...",
            total
        ),
    }));

    let mut pending = stream::iter(input_files.iter().cloned().enumerate().map(
        |(index, path)| async move {
            let bounds = detect_raster_bounds(&path).await?;
            Ok::<(usize, String, RasterBounds), ProcessingError>((index, path, bounds))
        },
    ))
    .buffer_unordered(8);

    let mut processed = 0usize;
    let mut candidates = Vec::with_capacity(total);

    while let Some(result) = pending.next().await {
        let (index, path, bounds) = result?;
        processed += 1;
        candidates.push((index, path, bounds));

        if processed == total || processed % 25 == 0 {
            let percentage = 5 + ((processed as f64 / total as f64) * 4.0).round() as u8;
            sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
                stage: "preparing_inputs".to_string(),
                percentage: percentage.min(9),
                message: format!(
                    "Checked {} of {} raster bounds for intersection...",
                    processed, total
                ),
            }));
        }
    }

    let selected_input_files = select_intersecting_input_files(candidates, clip_bounds_wgs84);
    println!(
        "[processing] input filter kept {} of {} raster(s) for the selected area",
        selected_input_files.len(),
        total
    );
    for file in &selected_input_files {
        println!("[processing]   selected input: {}", file);
    }

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "preparing_inputs".to_string(),
        percentage: 9,
        message: format!(
            "Selected {} of {} raster(s) that intersect the requested area.",
            selected_input_files.len(),
            total
        ),
    }));

    Ok(selected_input_files)
}

fn select_intersecting_input_files(
    mut candidates: Vec<(usize, String, RasterBounds)>,
    clip_bounds: RasterBounds,
) -> Vec<String> {
    candidates.sort_by_key(|(index, _, _)| *index);
    candidates
        .into_iter()
        .filter_map(|(_, path, bounds)| clip_bounds.intersects(bounds).then_some(path))
        .collect()
}

async fn run_gdal_command_with_progress(
    mut command: Command,
    command_name: &str,
    stage: &str,
    start_percentage: u8,
    end_percentage: u8,
    message: &str,
    completion_message: &str,
    monitored_output_path: Option<&str>,
    sender: &ProgressSender,
) -> Result<(), ProcessingError> {
    println!(
        "[processing] starting {} for stage '{}' ({})",
        command_name, stage, message
    );
    println!(
        "[processing] command for stage '{}': {}",
        stage,
        format_command_line(&command)
    );
    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: stage.to_string(),
        percentage: start_percentage,
        message: build_command_start_message(command_name, message),
    }));

    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessingError::GdalError("failed to capture GDAL stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessingError::GdalError("failed to capture GDAL stderr".to_string()))?;

    let progress_parser = Arc::new(Mutex::new(GdalProgressParser::new()));
    let stdout_task = tokio::spawn(read_gdal_stream(
        stdout,
        command_name.to_string(),
        "stdout".to_string(),
        stage.to_string(),
        start_percentage,
        end_percentage,
        message.to_string(),
        sender.clone(),
        Arc::clone(&progress_parser),
    ));
    let stderr_task = tokio::spawn(read_gdal_stream(
        stderr,
        command_name.to_string(),
        "stderr".to_string(),
        stage.to_string(),
        start_percentage,
        end_percentage,
        message.to_string(),
        sender.clone(),
        Arc::clone(&progress_parser),
    ));
    let heartbeat_task = monitored_output_path.map(|path| {
        tokio::spawn(log_processing_heartbeat(
            command_name.to_string(),
            stage.to_string(),
            path.to_string(),
        ))
    });

    let status = child.wait().await?;
    let stdout_output = stdout_task
        .await
        .map_err(|e| ProcessingError::GdalError(e.to_string()))??;
    let stderr_output = stderr_task
        .await
        .map_err(|e| ProcessingError::GdalError(e.to_string()))??;
    if let Some(task) = heartbeat_task {
        task.abort();
        let _ = task.await;
    }

    if !status.success() {
        let stderr_text = String::from_utf8_lossy(&stderr_output).trim().to_string();
        let stdout_text = String::from_utf8_lossy(&stdout_output).trim().to_string();
        let details = if !stderr_text.is_empty() {
            stderr_text
        } else if !stdout_text.is_empty() {
            stdout_text
        } else {
            format!("{} exited with status {}", stage, status)
        };
        return Err(ProcessingError::GdalError(details));
    }

    println!(
        "[processing] {} finished for stage '{}'",
        command_name, stage
    );

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: stage.to_string(),
        percentage: end_percentage,
        message: completion_message.to_string(),
    }));

    Ok(())
}

async fn read_gdal_stream<R>(
    mut reader: R,
    command_name: String,
    stream_name: String,
    stage: String,
    start_percentage: u8,
    end_percentage: u8,
    message: String,
    sender: ProgressSender,
    progress_parser: Arc<Mutex<GdalProgressParser>>,
) -> Result<Vec<u8>, io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = [0_u8; 256];
    let mut log_buffer = StreamLogBuffer::default();

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }

        let chunk = &buffer[..read];
        output.extend_from_slice(chunk);

        let text = String::from_utf8_lossy(chunk);
        if looks_like_progress_fragment(&text) {
            let percents = {
                let mut parser = progress_parser
                    .lock()
                    .map_err(|_| io::Error::other("failed to lock GDAL progress parser"))?;
                parser.consume(chunk)
            };
            for percent in percents {
                let percentage = scale_progress(percent, start_percentage, end_percentage);
                sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
                    stage: stage.clone(),
                    percentage,
                    message: format!("{} {}%", message, percent),
                }));
            }
        }

        for line in log_buffer.push(&text) {
            if let Some(line) = normalize_log_line(&line) {
                println!("[processing] {} {}: {}", command_name, stream_name, line);
            }
        }
    }

    if let Some(line) = log_buffer
        .finish()
        .and_then(|line| normalize_log_line(&line))
    {
        println!("[processing] {} {}: {}", command_name, stream_name, line);
    }

    Ok(output)
}

async fn log_processing_heartbeat(
    command_name: String,
    stage: String,
    monitored_output_path: String,
) {
    let started_at = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let output_status = describe_output_file(&monitored_output_path);
        println!(
            "[processing] {} heartbeat for stage '{}': elapsed={}s, output={}",
            command_name,
            stage,
            started_at.elapsed().as_secs(),
            output_status
        );
    }
}

fn build_command_start_message(command_name: &str, message: &str) -> String {
    format!("Starting {}. {}", command_name, message)
}

fn scale_progress(progress: u8, start_percentage: u8, end_percentage: u8) -> u8 {
    if end_percentage <= start_percentage {
        return end_percentage;
    }

    let span = (end_percentage - start_percentage) as f64;
    let scaled = start_percentage as f64 + (progress as f64 / 100.0) * span;
    scaled.round().clamp(0.0, 100.0) as u8
}

#[derive(Default)]
struct GdalProgressParser {
    current_digits: String,
    last_percent: u8,
}

#[derive(Default)]
struct StreamLogBuffer {
    pending: String,
}

impl GdalProgressParser {
    fn new() -> Self {
        Self::default()
    }

    fn consume(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut parsed = Vec::new();

        for byte in bytes {
            let ch = *byte as char;
            if ch.is_ascii_digit() {
                self.current_digits.push(ch);
                continue;
            }

            if let Some(percent) = self.finish_number() {
                parsed.push(percent);
            }
        }

        parsed
    }

    fn finish_number(&mut self) -> Option<u8> {
        if self.current_digits.is_empty() {
            return None;
        }

        let digits = std::mem::take(&mut self.current_digits);
        let percent = digits.parse::<u8>().ok()?;
        if percent > 100 || percent < self.last_percent {
            return None;
        }
        if percent == self.last_percent {
            return None;
        }

        self.last_percent = percent;
        Some(percent)
    }
}

impl StreamLogBuffer {
    fn push(&mut self, text: &str) -> Vec<String> {
        self.pending.push_str(text);

        let mut completed = Vec::new();
        let mut segment_start = 0;
        for (index, ch) in self.pending.char_indices() {
            if ch == '\n' || ch == '\r' {
                completed.push(self.pending[segment_start..index].to_string());
                segment_start = index + ch.len_utf8();
            }
        }

        self.pending = self.pending[segment_start..].to_string();
        completed
    }

    fn finish(&mut self) -> Option<String> {
        let remaining = self.pending.trim();
        if remaining.is_empty() {
            self.pending.clear();
            return None;
        }
        let line = remaining.to_string();
        self.pending.clear();
        Some(line)
    }
}

fn format_command_line(command: &Command) -> String {
    let command = command.as_std();
    let mut parts = Vec::new();
    parts.push(shell_escape_arg(&command.get_program().to_string_lossy()));
    parts.extend(
        command
            .get_args()
            .map(|arg| shell_escape_arg(&arg.to_string_lossy())),
    );
    parts.join(" ")
}

fn shell_escape_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '=' | ':'))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\\''"))
}

fn looks_like_progress_fragment(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '%' | ' '))
}

fn normalize_log_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || looks_like_progress_fragment(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

fn describe_output_file(path: &str) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => format!("{} ({})", path, format_byte_count(metadata.len())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => format!("{} (not created yet)", path),
        Err(err) => format!("{} (stat failed: {})", path, err),
    }
}

fn format_byte_count(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn web_mercator_to_wgs84(x: f64, y: f64) -> (f64, f64) {
    let lon = x * 180.0 / 20_037_508.34;
    let lat = (y * std::f64::consts::PI / 20_037_508.34)
        .exp()
        .atan()
        .mul_add(360.0 / std::f64::consts::PI, -90.0);
    (lon, lat)
}

async fn detect_predictor_option(input_file: Option<&str>) -> Option<u8> {
    let input_file = input_file?;
    let data_type = detect_raster_data_type(input_file).await.ok()?;
    if is_float_raster_type(&data_type) {
        return Some(3);
    }
    None
}

async fn detect_raster_data_type(path: &str) -> Result<String, ProcessingError> {
    let output = Command::new("gdalinfo")
        .arg("-json")
        .arg(path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProcessingError::GdalError(format!(
            "gdalinfo failed: {}",
            stderr
        )));
    }

    let json_text = String::from_utf8_lossy(&output.stdout);
    parse_band_data_type(&json_text).ok_or_else(|| {
        ProcessingError::GdalError("gdalinfo output missing band data type".to_string())
    })
}

async fn detect_raster_bounds(path: &str) -> Result<RasterBounds, ProcessingError> {
    let output = Command::new("gdalinfo")
        .arg("-json")
        .arg(path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProcessingError::GdalError(format!(
            "gdalinfo failed for {}: {}",
            path, stderr
        )));
    }

    let json_text = String::from_utf8_lossy(&output.stdout);
    parse_raster_bounds(&json_text).ok_or_else(|| {
        ProcessingError::GdalError(format!(
            "gdalinfo output missing raster bounds for {}",
            path
        ))
    })
}

fn parse_band_data_type(gdalinfo_json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(gdalinfo_json).ok()?;
    value
        .get("bands")?
        .as_array()?
        .first()?
        .get("type")?
        .as_str()
        .map(|s| s.to_string())
}

fn parse_raster_bounds(gdalinfo_json: &str) -> Option<RasterBounds> {
    let value: Value = serde_json::from_str(gdalinfo_json).ok()?;
    if let Some(bounds) = value.get("wgs84Extent").and_then(parse_geojson_bounds) {
        return Some(bounds);
    }

    let corners = value.get("cornerCoordinates")?;
    bounds_from_corner_coordinates(corners)
}

fn parse_geojson_bounds(value: &Value) -> Option<RasterBounds> {
    let coordinates = value.get("coordinates")?;
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    collect_geojson_coordinate_pairs(coordinates, &mut xs, &mut ys);
    if xs.is_empty() || ys.is_empty() {
        return None;
    }

    Some(RasterBounds {
        min_x: xs.iter().copied().fold(f64::INFINITY, f64::min),
        min_y: ys.iter().copied().fold(f64::INFINITY, f64::min),
        max_x: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        max_y: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

fn collect_geojson_coordinate_pairs(value: &Value, xs: &mut Vec<f64>, ys: &mut Vec<f64>) {
    if let Some((x, y)) = parse_coordinate_pair(value) {
        xs.push(x);
        ys.push(y);
        return;
    }

    if let Some(items) = value.as_array() {
        for item in items {
            collect_geojson_coordinate_pairs(item, xs, ys);
        }
    }
}

fn bounds_from_corner_coordinates(corners: &Value) -> Option<RasterBounds> {
    let mut xs = Vec::with_capacity(4);
    let mut ys = Vec::with_capacity(4);

    for key in ["upperLeft", "lowerLeft", "lowerRight", "upperRight"] {
        let (x, y) = parse_coordinate_pair(corners.get(key)?)?;
        xs.push(x);
        ys.push(y);
    }

    Some(RasterBounds {
        min_x: xs.iter().copied().fold(f64::INFINITY, f64::min),
        min_y: ys.iter().copied().fold(f64::INFINITY, f64::min),
        max_x: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        max_y: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

fn parse_coordinate_pair(value: &Value) -> Option<(f64, f64)> {
    let coords = value.as_array()?;
    let x = coords.first()?.as_f64()?;
    let y = coords.get(1)?.as_f64()?;
    Some((x, y))
}

fn is_float_raster_type(data_type: &str) -> bool {
    matches!(data_type, "Float32" | "Float64" | "CFloat32" | "CFloat64")
}

pub async fn check_gdal_available() -> Result<String, ProcessingError> {
    let output = Command::new("gdalinfo")
        .arg("--version")
        .output()
        .await
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

    #[test]
    fn test_parse_band_data_type() {
        let json = r#"{"bands":[{"band":1,"type":"Float32"}]}"#;
        assert_eq!(parse_band_data_type(json), Some("Float32".to_string()));
    }

    #[test]
    fn test_parse_band_data_type_missing() {
        let json = r#"{"bands":[]}"#;
        assert_eq!(parse_band_data_type(json), None);
    }

    #[test]
    fn test_parse_raster_bounds() {
        let json = r#"{
            "cornerCoordinates": {
                "upperLeft": [100.0, 500.0],
                "lowerLeft": [100.0, 200.0],
                "lowerRight": [300.0, 200.0],
                "upperRight": [300.0, 500.0]
            }
        }"#;
        assert_eq!(
            parse_raster_bounds(json),
            Some(RasterBounds {
                min_x: 100.0,
                min_y: 200.0,
                max_x: 300.0,
                max_y: 500.0,
            })
        );
    }

    #[test]
    fn test_parse_raster_bounds_missing_corner_coordinates() {
        let json = r#"{"cornerCoordinates":{"upperLeft":[0.0, 1.0]}}"#;
        assert_eq!(parse_raster_bounds(json), None);
    }

    #[test]
    fn test_clip_extent_intersects_raster_bounds() {
        let clip_bounds = RasterBounds {
            min_x: 100.0,
            min_y: 100.0,
            max_x: 200.0,
            max_y: 200.0,
        };
        assert!(clip_bounds.intersects(RasterBounds {
            min_x: 150.0,
            min_y: 150.0,
            max_x: 250.0,
            max_y: 250.0,
        }));
        assert!(!clip_bounds.intersects(RasterBounds {
            min_x: 201.0,
            min_y: 201.0,
            max_x: 300.0,
            max_y: 300.0,
        }));
    }

    #[test]
    fn test_select_intersecting_input_files_preserves_original_order() {
        let selected = select_intersecting_input_files(
            vec![
                (
                    2,
                    "tile-c.tif".to_string(),
                    RasterBounds {
                        min_x: 500.0,
                        min_y: 500.0,
                        max_x: 600.0,
                        max_y: 600.0,
                    },
                ),
                (
                    0,
                    "tile-a.tif".to_string(),
                    RasterBounds {
                        min_x: 0.0,
                        min_y: 0.0,
                        max_x: 100.0,
                        max_y: 100.0,
                    },
                ),
                (
                    1,
                    "tile-b.tif".to_string(),
                    RasterBounds {
                        min_x: 90.0,
                        min_y: 90.0,
                        max_x: 180.0,
                        max_y: 180.0,
                    },
                ),
            ],
            RasterBounds {
                min_x: 50.0,
                min_y: 50.0,
                max_x: 150.0,
                max_y: 150.0,
            },
        );

        assert_eq!(
            selected,
            vec!["tile-a.tif".to_string(), "tile-b.tif".to_string()]
        );
    }

    #[test]
    fn test_is_float_raster_type() {
        assert!(is_float_raster_type("Float32"));
        assert!(is_float_raster_type("Float64"));
        assert!(!is_float_raster_type("UInt16"));
    }

    #[test]
    fn test_scale_progress() {
        assert_eq!(scale_progress(0, 10, 90), 10);
        assert_eq!(scale_progress(50, 10, 90), 50);
        assert_eq!(scale_progress(100, 10, 90), 90);
    }

    #[test]
    fn test_build_command_start_message() {
        assert_eq!(
            build_command_start_message("gdalwarp", "Clipping selected area..."),
            "Starting gdalwarp. Clipping selected area..."
        );
    }

    #[test]
    fn test_gdal_progress_parser_reads_chunked_progress() {
        let mut parser = GdalProgressParser::new();
        assert_eq!(parser.consume(b"0...10..."), vec![10]);
        assert_eq!(parser.consume(b"20...3"), vec![20]);
        assert_eq!(parser.consume(b"0...100 - done."), vec![30, 100]);
        assert_eq!(parser.last_percent, 100);
    }

    #[test]
    fn test_gdal_progress_parser_ignores_duplicates_and_invalid_values() {
        let mut parser = GdalProgressParser::new();
        assert_eq!(parser.consume(b"0...0..."), Vec::<u8>::new());
        assert_eq!(parser.consume(b"10..."), vec![10]);
        assert_eq!(parser.consume(b"105...9..."), Vec::<u8>::new());
        assert_eq!(parser.last_percent, 10);
    }

    #[test]
    fn test_stream_log_buffer_splits_newlines_and_carriage_returns() {
        let mut buffer = StreamLogBuffer::default();
        assert_eq!(
            buffer.push("line one\nline two\rline"),
            vec!["line one".to_string(), "line two".to_string()]
        );
        assert_eq!(buffer.finish(), Some("line".to_string()));
    }

    #[test]
    fn test_looks_like_progress_fragment() {
        assert!(looks_like_progress_fragment("0...10...20..."));
        assert!(looks_like_progress_fragment(" 30...40... "));
        assert!(!looks_like_progress_fragment(
            "Warning 1: something happened"
        ));
    }

    #[test]
    fn test_normalize_log_line_filters_progress_only_lines() {
        assert_eq!(normalize_log_line("0...10..."), None);
        assert_eq!(
            normalize_log_line("Warning 1: source has nodata"),
            Some("Warning 1: source has nodata".to_string())
        );
    }

    #[test]
    fn test_format_byte_count() {
        assert_eq!(format_byte_count(999), "999 B");
        assert_eq!(format_byte_count(2048), "2.00 KB");
    }
}
