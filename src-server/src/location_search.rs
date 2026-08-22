use axum::{extract::Query, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;

use crate::routes::AppState;

const NOMINATIM_SEARCH_URL: &str = "https://nominatim.openstreetmap.org/search";
const ONTARIO_VIEWBOX: &str = "-95.2,56.9,-74.3,41.7";
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_CACHE_ENTRIES: usize = 1_000;

#[derive(Debug, Deserialize)]
pub struct LocationQuery {
    q: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocationSearchResult {
    boundingbox: Option<Vec<String>>,
    display_name: Option<String>,
    lat: Option<String>,
    lon: Option<String>,
}

#[derive(Clone)]
struct CachedSearch {
    results: Vec<LocationSearchResult>,
    cached_at: Instant,
}

pub struct LocationSearchService {
    client: reqwest::Client,
    cache: RwLock<HashMap<String, CachedSearch>>,
    last_request_at: Mutex<Option<Instant>>,
}

impl LocationSearchService {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Ontario-DTM-Downloader/0.1")
                .build()
                .expect("location search HTTP client should initialize"),
            cache: RwLock::new(HashMap::new()),
            last_request_at: Mutex::new(None),
        }
    }

    async fn search(&self, query: &str) -> Result<Vec<LocationSearchResult>, (StatusCode, String)> {
        let cache_key = cache_key(query);
        if let Some(results) = self.cached_results(&cache_key).await {
            return Ok(results);
        }

        let mut last_request_at = self.last_request_at.lock().await;
        if let Some(delay) = required_delay(*last_request_at, Instant::now()) {
            tokio::time::sleep(delay).await;
        }

        if let Some(results) = self.cached_results(&cache_key).await {
            return Ok(results);
        }

        *last_request_at = Some(Instant::now());
        let results = self.fetch_results(query).await?;
        self.cache_results(cache_key, results.clone()).await;
        Ok(results)
    }

    async fn fetch_results(
        &self,
        search_query: &str,
    ) -> Result<Vec<LocationSearchResult>, (StatusCode, String)> {
        let response = self
            .client
            .get(NOMINATIM_SEARCH_URL)
            .query(&[
                ("format", "jsonv2"),
                ("q", search_query),
                ("countrycodes", "ca"),
                ("viewbox", ONTARIO_VIEWBOX),
                ("bounded", "1"),
                ("limit", "5"),
                ("addressdetails", "1"),
                ("accept-language", "en"),
            ])
            .send()
            .await
            .map_err(bad_gateway)?;

        if !response.status().is_success() {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("Location search returned {}", response.status()),
            ));
        }

        response
            .json::<Vec<LocationSearchResult>>()
            .await
            .map_err(bad_gateway)
    }

    async fn cached_results(&self, key: &str) -> Option<Vec<LocationSearchResult>> {
        let cached = self.cache.read().await.get(key).cloned()?;
        if cached.cached_at.elapsed() <= CACHE_TTL {
            return Some(cached.results);
        }

        self.cache.write().await.remove(key);
        None
    }

    async fn cache_results(&self, key: String, results: Vec<LocationSearchResult>) {
        let mut cache = self.cache.write().await;
        if cache.len() >= MAX_CACHE_ENTRIES {
            if let Some(oldest_key) = cache
                .iter()
                .min_by_key(|(_, cached)| cached.cached_at)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest_key);
            }
        }
        cache.insert(
            key,
            CachedSearch {
                results,
                cached_at: Instant::now(),
            },
        );
    }
}

impl Default for LocationSearchService {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn search_locations(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<AppState>>>,
    Query(query): Query<LocationQuery>,
) -> Result<Json<Vec<LocationSearchResult>>, (StatusCode, String)> {
    let search_query = scoped_search_query(&query.q).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Location query cannot be empty".to_string(),
        )
    })?;
    let service = Arc::clone(&state.read().await.location_search);
    let results = service.search(&search_query).await?;
    Ok(Json(results))
}

fn scoped_search_query(query: &str) -> Option<String> {
    let query = query.trim();
    (!query.is_empty()).then(|| format!("{}, Ontario, Canada", query))
}

fn cache_key(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn required_delay(last_request_at: Option<Instant>, now: Instant) -> Option<Duration> {
    let elapsed = now.checked_duration_since(last_request_at?)?;
    (elapsed < MIN_REQUEST_INTERVAL).then(|| MIN_REQUEST_INTERVAL - elapsed)
}

fn bad_gateway(error: impl std::fmt::Display) -> (StatusCode, String) {
    (
        StatusCode::BAD_GATEWAY,
        format!("Location search failed: {}", error),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scoped_search_query_adds_ontario() {
        assert_eq!(
            scoped_search_query("  Kingston  ").as_deref(),
            Some("Kingston, Ontario, Canada")
        );
    }

    #[test]
    fn test_scoped_search_query_rejects_empty_input() {
        assert_eq!(scoped_search_query("   "), None);
    }

    #[test]
    fn test_cache_key_normalizes_case_and_whitespace() {
        assert_eq!(cache_key("  KINGSTON   Ontario "), "kingston ontario");
    }

    #[test]
    fn test_required_delay_enforces_one_second_interval() {
        let now = Instant::now();
        let last_request_at = now - Duration::from_millis(250);

        assert_eq!(
            required_delay(Some(last_request_at), now),
            Some(Duration::from_millis(750))
        );
        assert_eq!(
            required_delay(Some(now - Duration::from_secs(1)), now),
            None
        );
        assert_eq!(required_delay(None, now), None);
    }

    #[tokio::test]
    async fn test_cache_returns_results_and_discards_expired_entries() {
        let service = LocationSearchService::new();
        let result = LocationSearchResult {
            boundingbox: None,
            display_name: Some("Kingston".to_string()),
            lat: Some("44.23".to_string()),
            lon: Some("-76.48".to_string()),
        };
        service
            .cache_results("kingston".to_string(), vec![result])
            .await;

        assert_eq!(service.cached_results("kingston").await.unwrap().len(), 1);

        service.cache.write().await.insert(
            "expired".to_string(),
            CachedSearch {
                results: Vec::new(),
                cached_at: Instant::now() - CACHE_TTL - Duration::from_secs(1),
            },
        );
        assert!(service.cached_results("expired").await.is_none());
        assert!(!service.cache.read().await.contains_key("expired"));
    }

    #[test]
    fn test_location_result_deserializes_nominatim_response() {
        let result: LocationSearchResult = serde_json::from_str(
            r#"{"display_name":"Kingston, Ontario, Canada","lat":"44.2307","lon":"-76.4813","boundingbox":["44.18","44.30","-76.60","-76.40"]}"#,
        )
        .unwrap();

        assert_eq!(
            result.display_name.as_deref(),
            Some("Kingston, Ontario, Canada")
        );
        assert_eq!(result.boundingbox.unwrap().len(), 4);
    }
}
