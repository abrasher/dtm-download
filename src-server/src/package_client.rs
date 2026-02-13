//! Client for querying the Ontario DTM Package Index via ArcGIS REST API.

use crate::api_types::{
    extract_download_url, extract_year_range, ArcGISQueryResponse, BoundingBox, GeoJSONGeometry,
    Package,
};
use reqwest::Client;
use thiserror::Error;

/// Base URL for the Ontario DTM Package Index Feature Server.
const BASE_URL: &str = "https://services1.arcgis.com/TJH5KDher0W13Kgo/arcgis/rest/services/Ontario_Digital_Terrain_Model_Lidar_Derived_WFL1/FeatureServer/0";

/// Maximum records per request (ArcGIS limit).
const MAX_RECORD_COUNT: usize = 2000;

/// Errors that can occur when querying the package index.
#[derive(Debug, Error)]
pub enum PackageClientError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("Failed to parse JSON response: {0}")]
    JsonParseError(#[from] serde_json::Error),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid geometry in response")]
    InvalidGeometry,
}

/// Client for querying the Ontario DTM Package Index.
#[derive(Debug, Clone)]
pub struct PackageClient {
    client: Client,
    base_url: String,
}

impl Default for PackageClient {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageClient {
    /// Create a new package client with default settings.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: BASE_URL.to_string(),
        }
    }

    /// Create a client with a custom base URL (for testing).
    pub fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// Query packages that intersect with the given bounding box.
    ///
    /// # Arguments
    /// * `bbox` - The bounding box to query
    ///
    /// # Returns
    /// A vector of packages that intersect with the bounding box.
    pub async fn query_by_extent(
        &self,
        bbox: &BoundingBox,
    ) -> Result<Vec<Package>, PackageClientError> {
        let mut all_packages = Vec::new();
        let mut offset = 0;

        loop {
            let packages = self.query_page(bbox, offset).await?;
            let count = packages.len();
            all_packages.extend(packages);

            // If we got fewer than MAX_RECORD_COUNT, we've reached the end
            if count < MAX_RECORD_COUNT {
                break;
            }
            offset += count;
        }

        Ok(all_packages)
    }

    /// Query a single page of results.
    async fn query_page(
        &self,
        bbox: &BoundingBox,
        offset: usize,
    ) -> Result<Vec<Package>, PackageClientError> {
        let geometry = bbox.to_esri_geometry();
        println!("ArcGIS query geometry: {}", geometry);

        let params = [
            ("f", "json"),
            ("where", "1=1"),
            (
                "outFields",
                "Package,Size_GB,Resolution,DownloadLink,Project,Shape__Area",
            ),
            ("geometryType", "esriGeometryEnvelope"),
            ("geometry", &geometry),
            ("spatialRel", "esriSpatialRelIntersects"),
            ("inSR", &bbox.srid.to_string()),
            ("outSR", &bbox.srid.to_string()),
            ("returnGeometry", "true"),
            ("resultOffset", &offset.to_string()),
            ("resultRecordCount", &MAX_RECORD_COUNT.to_string()),
        ];

        let url = format!("{}/query", self.base_url);
        let response = self
            .client
            .post(&url)
            .form(&params)
            .send()
            .await?
            .error_for_status()?;

        let text = response.text().await?;
        println!("ArcGIS response length: {} bytes", text.len());

        if text.contains("\"error\"") {
            eprintln!("ArcGIS error response: {}", text);
        }

        let arcgis_response: ArcGISQueryResponse = serde_json::from_str(&text)?;
        let raw_count = arcgis_response.features.len();
        println!("ArcGIS returned {} raw features", raw_count);

        // Convert ArcGIS features to our Package type
        let packages = arcgis_response
            .features
            .into_iter()
            .filter_map(|f| self.feature_to_package(f).transpose())
            .collect::<Result<Vec<_>, _>>()?;

        if packages.len() != raw_count {
            println!(
                "Filtered: {} features removed (missing fields)",
                raw_count - packages.len()
            );
        }

        Ok(packages)
    }

    /// Query all packages (no spatial filter).
    ///
    /// Use with caution as this may return a large number of results.
    pub async fn query_all(&self) -> Result<Vec<Package>, PackageClientError> {
        let mut all_packages = Vec::new();
        let mut offset = 0;

        loop {
            let packages = self.query_all_page(offset).await?;
            let count = packages.len();
            all_packages.extend(packages);

            if count < MAX_RECORD_COUNT {
                break;
            }
            offset += count;
        }

        Ok(all_packages)
    }

    /// Query a single page without spatial filter.
    async fn query_all_page(&self, offset: usize) -> Result<Vec<Package>, PackageClientError> {
        let params = [
            ("f", "json"),
            ("where", "1=1"),
            (
                "outFields",
                "Package,Size_GB,Resolution,DownloadLink,Project,Shape__Area",
            ),
            ("returnGeometry", "true"),
            ("resultOffset", &offset.to_string()),
            ("resultRecordCount", &MAX_RECORD_COUNT.to_string()),
        ];

        let url = format!("{}/query", self.base_url);
        let response = self
            .client
            .post(&url)
            .form(&params)
            .send()
            .await?
            .error_for_status()?;

        let text = response.text().await?;
        let arcgis_response: ArcGISQueryResponse = serde_json::from_str(&text)?;

        let packages = arcgis_response
            .features
            .into_iter()
            .filter_map(|f| self.feature_to_package(f).transpose())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(packages)
    }

    /// Convert an ArcGIS feature to our Package type.
    fn feature_to_package(
        &self,
        feature: crate::api_types::ArcGISFeature,
    ) -> Result<Option<Package>, PackageClientError> {
        let attrs = feature.attributes;

        let package_name = match &attrs.package {
            Some(name) if !name.is_empty() => name.clone(),
            Some(_) => {
                println!("Skipping feature with empty package name");
                return Ok(None);
            }
            None => {
                println!("Skipping feature with no package name");
                return Ok(None);
            }
        };

        let download_url = match &attrs.download_link {
            Some(html) if !html.is_empty() => match extract_download_url(html) {
                Some(url) => url,
                None => {
                    println!(
                        "Skipping package '{}' - could not extract URL from: {}",
                        package_name, html
                    );
                    return Ok(None);
                }
            },
            Some(_) => {
                println!("Skipping package '{}' - empty download link", package_name);
                return Ok(None);
            }
            None => {
                println!("Skipping package '{}' - no download link", package_name);
                return Ok(None);
            }
        };

        let geometry = match feature.geometry {
            Some(geom) => GeoJSONGeometry::from_esri_rings(geom.rings),
            None => {
                println!("Skipping package '{}' - no geometry", package_name);
                return Ok(None);
            }
        };

        let project = attrs.project.clone().unwrap_or_default();
        let year_range = attrs.project.as_ref().and_then(|p| extract_year_range(p));

        let coverage_km2 = attrs
            .shape_area
            .map(|area| area / 1_000_000.0)
            .unwrap_or(0.0);

        Ok(Some(Package {
            package_name,
            size_gb: attrs.size_gb.unwrap_or(0.0),
            resolution: attrs.resolution.unwrap_or(0.0),
            download_url,
            project,
            year_range,
            coverage_km2,
            geometry,
        }))
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = PackageClient::new();
        assert_eq!(client.base_url, BASE_URL);
    }

    #[test]
    fn test_client_custom_url() {
        let client = PackageClient::with_base_url("https://example.com".to_string());
        assert_eq!(client.base_url, "https://example.com");
    }

    #[test]
    fn test_feature_to_package_conversion() {
        let feature = crate::api_types::ArcGISFeature {
            attributes: crate::api_types::ArcGISAttributes {
                package: Some("Test Package".to_string()),
                size_gb: Some(2.5),
                resolution: Some(0.5),
                download_link: Some(
                    r#"<a href="https://example.com/test.zip">Test</a>"#.to_string(),
                ),
                project: Some("Test Project 2016-18".to_string()),
                shape_area: Some(1_000_000_000.0),
            },
            geometry: Some(crate::api_types::ArcGISPolygonGeometry {
                rings: vec![vec![
                    vec![0.0, 0.0],
                    vec![1.0, 0.0],
                    vec![1.0, 1.0],
                    vec![0.0, 0.0],
                ]],
            }),
        };

        let client = PackageClient::new();
        let result = client.feature_to_package(feature).unwrap();

        assert!(result.is_some());
        let package = result.unwrap();
        assert_eq!(package.package_name, "Test Package");
        assert_eq!(package.size_gb, 2.5);
        assert_eq!(package.resolution, 0.5);
        assert_eq!(package.download_url, "https://example.com/test.zip");
        assert_eq!(package.project, "Test Project 2016-18");
        assert_eq!(package.year_range, Some("2016-18".to_string()));
        assert!((package.coverage_km2 - 1000.0).abs() < 0.01);
    }

    #[test]
    fn test_feature_to_package_skips_missing_name() {
        let feature = crate::api_types::ArcGISFeature {
            attributes: crate::api_types::ArcGISAttributes {
                package: None,
                size_gb: Some(2.5),
                resolution: Some(0.5),
                download_link: Some(
                    r#"<a href="https://example.com/test.zip">Test</a>"#.to_string(),
                ),
                project: Some("Test Project".to_string()),
                shape_area: Some(1_000_000.0),
            },
            geometry: Some(crate::api_types::ArcGISPolygonGeometry {
                rings: vec![vec![]],
            }),
        };

        let client = PackageClient::new();
        let result = client.feature_to_package(feature).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_feature_to_package_skips_invalid_url() {
        let feature = crate::api_types::ArcGISFeature {
            attributes: crate::api_types::ArcGISAttributes {
                package: Some("Test".to_string()),
                size_gb: Some(2.5),
                resolution: Some(0.5),
                download_link: Some("no link here".to_string()),
                project: Some("Test Project".to_string()),
                shape_area: Some(1_000_000.0),
            },
            geometry: Some(crate::api_types::ArcGISPolygonGeometry {
                rings: vec![vec![]],
            }),
        };

        let client = PackageClient::new();
        let result = client.feature_to_package(feature).unwrap();
        assert!(result.is_none());
    }

    // Integration test - requires network access
    // Run with: cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn test_query_by_extent_integration() {
        let client = PackageClient::new();

        // Use a small bounding box in Ontario
        let bbox = BoundingBox::new(-9321521.0, 6371205.0, -9284703.0, 6408629.0, 3857);

        let packages = client.query_by_extent(&bbox).await.unwrap();
        assert!(!packages.is_empty());

        // Check that we got valid packages
        for pkg in &packages {
            assert!(!pkg.package_name.is_empty());
            assert!(pkg.download_url.starts_with("https://"));
            assert!(pkg.size_gb > 0.0);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_query_all_integration() {
        let client = PackageClient::new();

        // Query first page only to keep test fast
        let packages = client.query_all_page(0).await.unwrap();
        assert!(!packages.is_empty());
        assert!(packages.len() <= MAX_RECORD_COUNT);
    }
}
