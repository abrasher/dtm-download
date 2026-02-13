use crate::api_types::{ProcessingProgressEvent, ProgressEvent};
use crate::download::ProgressSender;
use serde_json::Value;
use std::io;
use std::process::Command;
use thiserror::Error;

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

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "merging".to_string(),
        percentage: 0,
        message: "Starting merge process...".to_string(),
    }));

    let compress_opt = format!("COMPRESS={}", compression.to_gdal_string());
    let predictor_opt = detect_predictor_option(input_files.first().map(|s| s.as_str()))
        .map(|p| format!("PREDICTOR={}", p));
    let temp_path = format!("{}.temp.tif", output_path.trim_end_matches(".tif"));

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "merging".to_string(),
        percentage: 10,
        message: "Merging and clipping rasters...".to_string(),
    }));

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

    let warp_output = warp_cmd.output()?;
    if !warp_output.status.success() {
        let stderr = String::from_utf8_lossy(&warp_output.stderr);
        return Err(ProcessingError::GdalError(format!(
            "gdalwarp failed: {}",
            stderr
        )));
    }

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "creating_cog".to_string(),
        percentage: 60,
        message: "Creating Cloud Optimized GeoTIFF...".to_string(),
    }));

    let translate_output = Command::new("gdal_translate")
        .arg(&temp_path)
        .arg(output_path)
        .arg("-of")
        .arg("COG")
        .arg("-co")
        .arg(&compress_opt)
        .args(
            predictor_opt
                .as_ref()
                .map(|p| vec!["-co", p.as_str()])
                .unwrap_or_default(),
        )
        .arg("-co")
        .arg("BIGTIFF=YES")
        .arg("-co")
        .arg("BLOCKSIZE=512")
        .arg("-co")
        .arg("NUM_THREADS=ALL_CPUS")
        .output()?;

    if !translate_output.status.success() {
        let stderr = String::from_utf8_lossy(&translate_output.stderr);
        return Err(ProcessingError::GdalError(format!(
            "gdal_translate failed: {}",
            stderr
        )));
    }

    let _ = std::fs::remove_file(&temp_path);

    sender.send(ProgressEvent::Processing(ProcessingProgressEvent {
        stage: "completed".to_string(),
        percentage: 100,
        message: "Processing complete!".to_string(),
    }));

    Ok(())
}

fn detect_predictor_option(input_file: Option<&str>) -> Option<u8> {
    let input_file = input_file?;
    let data_type = detect_raster_data_type(input_file).ok()?;
    if is_float_raster_type(&data_type) {
        return Some(3);
    }
    None
}

fn detect_raster_data_type(path: &str) -> Result<String, ProcessingError> {
    let output = Command::new("gdalinfo").arg("-json").arg(path).output()?;

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
}
