//! Types for the Ontario DTM Package Index API.

use serde::{Deserialize, Serialize};

/// A DTM package from the Ontario Lidar-derived package index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Package {
    /// The package name (e.g., "Cochrane A")
    pub package_name: String,
    /// Size of the package in gigabytes
    pub size_gb: f64,
    /// Resolution in meters (e.g., 0.5 for 50cm)
    pub resolution: f64,
    /// Direct download URL for the ZIP file
    pub download_url: String,
    /// The project name (e.g., "OMAFRA Lidar 2016-18")
    pub project: String,
    /// Year range extracted from project name (e.g., "2016-18")
    pub year_range: Option<String>,
    /// Coverage area in square kilometers
    pub coverage_km2: f64,
    /// The geometry as GeoJSON
    pub geometry: GeoJSONGeometry,
}

/// GeoJSON Geometry representation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "coordinates")]
pub enum GeoJSONGeometry {
    Point(Vec<f64>),
    MultiPoint(Vec<Vec<f64>>),
    LineString(Vec<Vec<f64>>),
    MultiLineString(Vec<Vec<Vec<f64>>>),
    Polygon(Vec<Vec<Vec<f64>>>),
    MultiPolygon(Vec<Vec<Vec<Vec<f64>>>>),
}

impl GeoJSONGeometry {
    /// Create a Polygon from ESRI rings format.
    /// ESRI rings are an array of rings, where each ring is an array of [x, y] points.
    pub fn from_esri_rings(rings: Vec<Vec<Vec<f64>>>) -> Self {
        GeoJSONGeometry::Polygon(rings)
    }
}

/// Bounding box for spatial queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundingBox {
    pub xmin: f64,
    pub ymin: f64,
    pub xmax: f64,
    pub ymax: f64,
    /// Spatial reference WKID (e.g., 3857 for Web Mercator)
    #[serde(default = "default_spatial_reference")]
    pub srid: u32,
}

fn default_spatial_reference() -> u32 {
    3857
}

impl BoundingBox {
    /// Create a new bounding box.
    pub fn new(xmin: f64, ymin: f64, xmax: f64, ymax: f64, srid: u32) -> Self {
        Self {
            xmin,
            ymin,
            xmax,
            ymax,
            srid,
        }
    }

    /// Convert to ESRI geometry JSON string.
    pub fn to_esri_geometry(&self) -> String {
        format!(
            r#"{{"xmin":{},"ymin":{},"xmax":{},"ymax":{},"spatialReference":{{"wkid":{}}}}}"#,
            self.xmin, self.ymin, self.xmax, self.ymax, self.srid
        )
    }
}

// ============================================================
// ArcGIS API Response Types (internal deserialization)
// ============================================================

/// Root response from ArcGIS query endpoint.
#[derive(Debug, Deserialize)]
pub(crate) struct ArcGISQueryResponse {
    pub features: Vec<ArcGISFeature>,
    /// Indicates if more results are available (pagination needed)
    /// Currently unused but kept for potential future use
    #[allow(dead_code)]
    #[serde(default)]
    pub exceeded_transfer_limit: bool,
}

/// A single feature from the ArcGIS response.
#[derive(Debug, Deserialize)]
pub(crate) struct ArcGISFeature {
    pub attributes: ArcGISAttributes,
    #[serde(default)]
    pub geometry: Option<ArcGISPolygonGeometry>,
}

/// Attributes from the ArcGIS feature.
#[derive(Debug, Deserialize)]
pub(crate) struct ArcGISAttributes {
    #[serde(rename = "Package")]
    pub package: Option<String>,
    #[serde(rename = "Size_GB")]
    pub size_gb: Option<f64>,
    #[serde(rename = "Resolution")]
    pub resolution: Option<f64>,
    #[serde(rename = "DownloadLink")]
    pub download_link: Option<String>,
    #[serde(rename = "Project")]
    pub project: Option<String>,
    #[serde(rename = "Shape__Area")]
    pub shape_area: Option<f64>,
}

/// ESRI Polygon geometry with rings.
#[derive(Debug, Deserialize)]
pub(crate) struct ArcGISPolygonGeometry {
    pub rings: Vec<Vec<Vec<f64>>>,
}

// ============================================================
// Web API Types
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub packages: Vec<Package>,
    pub projects: Vec<String>,
    pub total_size_gb: f64,
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

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounding_box_to_esri_geometry() {
        let bbox = BoundingBox::new(-9351879.0, 5097937.0, -8279588.0, 6421965.0, 3857);
        let esri = bbox.to_esri_geometry();
        assert!(esri.contains("xmin"));
        assert!(esri.contains("3857"));
    }

    #[test]
    fn test_extract_url_from_anchor_tag() {
        let html = r#"<a href="https://ws.gisetl.lrc.gov.on.ca/fmedatadownload/Packages/LIDAR2016to18_DTM-Crne-A.zip" target = "_blank">Lidar DTM Cochrane 2016-18 Package A</a>"#;
        let url = extract_download_url(html);
        assert_eq!(
            url,
            Some("https://ws.gisetl.lrc.gov.on.ca/fmedatadownload/Packages/LIDAR2016to18_DTM-Crne-A.zip".to_string())
        );
    }

    #[test]
    fn test_extract_url_with_single_quotes() {
        let html = r#"<a href='https://example.com/file.zip'>Download</a>"#;
        let url = extract_download_url(html);
        assert_eq!(url, Some("https://example.com/file.zip".to_string()));
    }

    #[test]
    fn test_extract_url_no_match() {
        let html = "No link here";
        let url = extract_download_url(html);
        assert_eq!(url, None);
    }

    #[test]
    fn test_extract_url_with_spaces_around_equals() {
        let html = r#"<a href = "https://example.com/file.zip" target = "_blank">Download</a>"#;
        let url = extract_download_url(html);
        assert_eq!(url, Some("https://example.com/file.zip".to_string()));
    }

    #[test]
    fn test_extract_url_with_spaces_single_quotes() {
        let html = r#"<a href = 'https://example.com/file.zip'>Download</a>"#;
        let url = extract_download_url(html);
        assert_eq!(url, Some("https://example.com/file.zip".to_string()));
    }

    #[test]
    fn test_geojson_geometry_from_rings() {
        let rings = vec![vec![
            vec![0.0, 0.0],
            vec![1.0, 0.0],
            vec![1.0, 1.0],
            vec![0.0, 1.0],
            vec![0.0, 0.0],
        ]];
        let geom = GeoJSONGeometry::from_esri_rings(rings);
        match geom {
            GeoJSONGeometry::Polygon(r) => assert_eq!(r.len(), 1),
            _ => panic!("Expected Polygon"),
        }
    }

    #[test]
    fn test_parse_arcgis_response() {
        let json = r#"{
            "features": [{
                "attributes": {
                    "Package": "Test Package",
                    "Size_GB": 1.5,
                    "Resolution": 0.5,
                    "DownloadLink": "<a href=\"https://example.com/test.zip\">Test</a>",
                    "Project": "Test Project",
                    "Shape__Area": 1000000000.0
                },
                "geometry": {
                    "rings": [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]]]
                }
            }],
            "exceeded_transfer_limit": false
        }"#;

        let response: ArcGISQueryResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.features.len(), 1);
        assert!(!response.exceeded_transfer_limit);

        let feature = &response.features[0];
        assert_eq!(feature.attributes.package, Some("Test Package".to_string()));
        assert_eq!(feature.attributes.size_gb, Some(1.5));
        assert_eq!(feature.attributes.shape_area, Some(1000000000.0));
        assert!(feature.geometry.is_some());
    }

    #[test]
    fn test_extract_year_range_with_dash() {
        assert_eq!(
            extract_year_range("OMAFRA Lidar 2016-18"),
            Some("2016-18".to_string())
        );
        assert_eq!(
            extract_year_range("GTA 2014-2018"),
            Some("2014-2018".to_string())
        );
    }

    #[test]
    fn test_extract_year_range_single_year() {
        assert_eq!(extract_year_range("LEAP 2009"), Some("2009".to_string()));
        assert_eq!(
            extract_year_range("Belleville 2022"),
            Some("2022".to_string())
        );
    }

    #[test]
    fn test_extract_year_range_no_match() {
        assert_eq!(extract_year_range("Some Project"), None);
        assert_eq!(extract_year_range(""), None);
    }

    #[test]
    fn test_extract_year_range_19xx() {
        assert_eq!(
            extract_year_range("Old Data 1999"),
            Some("1999".to_string())
        );
    }
}

/// Extract the actual URL from an HTML anchor tag.
/// Handles both double and single quoted href attributes, with optional spaces around =.
pub fn extract_download_url(html: &str) -> Option<String> {
    let html = html.trim();

    // Find href, then skip any whitespace and = and more whitespace
    let href_pos = html.find("href")?;
    let after_href = &html[href_pos + 4..];

    // Skip whitespace
    let after_href = after_href.trim_start();

    // Expect =
    if !after_href.starts_with('=') {
        return None;
    }
    let after_eq = &after_href[1..];

    // Skip whitespace after =
    let after_eq = after_eq.trim_start();

    // Expect quote
    let (quote, rest) = if after_eq.starts_with('"') {
        ('"', &after_eq[1..])
    } else if after_eq.starts_with('\'') {
        ('\'', &after_eq[1..])
    } else {
        return None;
    };

    // Find closing quote
    if let Some(end) = rest.find(quote) {
        return Some(rest[..end].to_string());
    }

    None
}

/// Extract year range from project name.
/// Examples: "OMAFRA Lidar 2016-18" -> "2016-18", "GTA 2014" -> "2014"
pub fn extract_year_range(project: &str) -> Option<String> {
    let re = regex::Regex::new(r"\b(19|20)\d{2}(?:\s*[-–]\s*(?:19|20)?\d{2})?\b").ok()?;
    let match_opt = re.find(project)?;
    Some(match_opt.as_str().replace(" ", ""))
}
