use crate::api_types::{ProcessingProgressEvent, ProgressEvent};
use crate::download::ProgressSender;
use serde_json::Value;
use std::io;
use std::process::Stdio;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

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

    let compress_opt = format!("COMPRESS={}", compression.to_gdal_string());
    let predictor_opt = detect_predictor_option(input_files.first().map(|s| s.as_str()))
        .await
        .map(|p| format!("PREDICTOR={}", p));
    let temp_path = format!("{}.temp.tif", output_path.trim_end_matches(".tif"));

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "preparing_inputs".to_string(),
        percentage: 5,
        message: format!(
            "Preparing {} raster(s) for processing...",
            input_files.len()
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

    for file in input_files {
        warp_cmd.arg(file);
    }
    warp_cmd.arg(&temp_path);
    run_gdal_command_with_progress(
        warp_cmd,
        "gdalwarp",
        warp_stage,
        10,
        74,
        warp_message,
        "gdalwarp finished. Preparing intermediate raster for Cloud Optimized GeoTIFF creation...",
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
        sender,
    )
    .await?;

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "finalizing".to_string(),
        percentage: 99,
        message: "Finalizing output and preparing download...".to_string(),
    }));

    let _ = std::fs::remove_file(&temp_path);

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "completed".to_string(),
        percentage: 100,
        message: "Processing complete!".to_string(),
    }));

    Ok(())
}

async fn run_gdal_command_with_progress(
    mut command: Command,
    command_name: &str,
    stage: &str,
    start_percentage: u8,
    end_percentage: u8,
    message: &str,
    completion_message: &str,
    sender: &ProgressSender,
) -> Result<(), ProcessingError> {
    println!(
        "[processing] starting {} for stage '{}' ({})",
        command_name, stage, message
    );
    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: stage.to_string(),
        percentage: start_percentage,
        message: build_command_start_message(command_name, message),
    }));

    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessingError::GdalError("failed to capture GDAL stdout".to_string()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessingError::GdalError("failed to capture GDAL stderr".to_string()))?;

    let progress_sender = sender.clone();
    let stage_name = stage.to_string();
    let stage_message = message.to_string();

    let stdout_task = tokio::spawn(async move {
        let mut parser = GdalProgressParser::new();
        let mut output = Vec::new();
        let mut buffer = [0_u8; 256];

        loop {
            let read = stdout.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            output.extend_from_slice(&buffer[..read]);
            for percent in parser.consume(&buffer[..read]) {
                let percentage = scale_progress(percent, start_percentage, end_percentage);
                progress_sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
                    stage: stage_name.clone(),
                    percentage,
                    message: format!("{} {}%", stage_message, percent),
                }));
            }
        }

        Ok::<(Vec<u8>, u8), io::Error>((output, parser.last_percent()))
    });

    let stderr_task = tokio::spawn(async move {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).await?;
        Ok::<Vec<u8>, io::Error>(output)
    });

    let status = child.wait().await?;
    let (stdout_output, _last_progress) = stdout_task
        .await
        .map_err(|e| ProcessingError::GdalError(e.to_string()))??;
    let stderr_output = stderr_task
        .await
        .map_err(|e| ProcessingError::GdalError(e.to_string()))??;

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

    println!("[processing] {} finished for stage '{}'", command_name, stage);

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: stage.to_string(),
        percentage: end_percentage,
        message: completion_message.to_string(),
    }));

    Ok(())
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

    fn last_percent(&self) -> u8 {
        self.last_percent
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
        assert_eq!(parser.last_percent(), 100);
    }

    #[test]
    fn test_gdal_progress_parser_ignores_duplicates_and_invalid_values() {
        let mut parser = GdalProgressParser::new();
        assert_eq!(parser.consume(b"0...0..."), Vec::<u8>::new());
        assert_eq!(parser.consume(b"10..."), vec![10]);
        assert_eq!(parser.consume(b"105...9..."), Vec::<u8>::new());
        assert_eq!(parser.last_percent(), 10);
    }
}
