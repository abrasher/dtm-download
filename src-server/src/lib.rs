pub mod api_types;
pub mod download;
pub mod package_client;
pub mod processing;
pub mod routes;

use axum::{
    routing::{get, post},
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};

pub fn create_router() -> Router {
    create_router_with_frontend_dist(resolve_frontend_dist_dir())
}

fn create_router_with_frontend_dist(frontend_dist_dir: Option<PathBuf>) -> Router {
    let state = Arc::new(RwLock::new(routes::AppState::new()));

    let router = Router::new()
        .route("/api/packages/query", post(routes::query_packages))
        .route("/api/download/start", post(routes::start_download))
        .route(
            "/api/download/{id}/progress",
            get(routes::download_progress),
        )
        .route("/api/download/{id}/file", get(routes::download_file))
        .route("/api/health", get(routes::health))
        .with_state(state)
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any));

    if let Some(dist_dir) = frontend_dist_dir {
        let index_file = dist_dir.join("index.html");
        let static_assets = ServeDir::new(dist_dir).not_found_service(ServeFile::new(index_file));
        return router.fallback_service(static_assets);
    }

    router
}

fn resolve_frontend_dist_dir() -> Option<PathBuf> {
    if let Ok(dist_dir) = std::env::var("FRONTEND_DIST") {
        let trimmed = dist_dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    let default_dist = PathBuf::from("dist");
    if default_dist.exists() {
        return Some(default_dist);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_route_is_available_without_frontend_dist() {
        let app = create_router_with_frontend_dist(None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body, "OK");
    }

    #[tokio::test]
    async fn test_root_serves_index_when_frontend_dist_exists() {
        let temp_dir = create_temp_frontend_dist();
        let index_path = temp_dir.join("index.html");
        std::fs::write(
            &index_path,
            "<!doctype html><title>Ontario DTM Download</title>",
        )
        .unwrap();
        let app = create_router_with_frontend_dist(Some(temp_dir.clone()));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(std::str::from_utf8(&body)
            .unwrap()
            .contains("Ontario DTM Download"));
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_root_is_not_found_without_frontend_dist() {
        let app = create_router_with_frontend_dist(None);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    fn create_temp_frontend_dist() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dtm-frontend-dist-{}", unique));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
