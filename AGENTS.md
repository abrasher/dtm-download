# AGENTS.md - Codebase Guidelines for AI Agents

## Project Overview

Ontario DTM Downloader - Web application for downloading LiDAR-derived Digital Terrain Model data. 
- **Frontend:** React 19, TypeScript, Vite 7, Leaflet, Tailwind CSS 4
- **Backend:** Rust, Axum, Tokio
- **Processing:** GDAL (gdalwarp, gdal_translate)

## Build/Lint/Test Commands

### Frontend (TypeScript/React)

```bash
npm run dev           # Start Vite dev server (localhost:5173)
npm run build         # Typecheck + build for production
npm run preview       # Preview production build

# Type checking only
npx tsc --noEmit      # Run TypeScript type check

# Run single test (when tests are added)
npx vitest run path/to/test.test.ts
npx vitest watch      # Watch mode
```

### Backend (Rust)

```bash
cd src-server && cargo run              # Start backend server (localhost:3000)
cd src-server && cargo build            # Debug build
cd src-server && cargo build --release  # Release build
cd src-server && cargo check            # Fast type check (no codegen)
cd src-server && cargo test             # Run all tests
cd src-server && cargo test test_name   # Run single test by name
cd src-server && cargo clippy           # Lint with clippy
cd src-server && cargo fmt              # Format code
```

### Combined Development

```bash
npm run dev:all        # Run frontend + backend concurrently
npm run dev:server     # Backend only
npm run build:server   # Build backend release binary
```

## Code Style Guidelines

### TypeScript/React

**Imports:** Group by external → internal, alphabetized within groups
```tsx
import { useState, useCallback, useEffect, useRef } from 'react';
import L from 'leaflet';
import 'leaflet/dist/leaflet.css';
import './App.css';
```

**Interfaces:** Define at top of file or in `src/types/`. Use PascalCase.
```tsx
interface Package {
  package_name: string;
  size_gb: number;
  download_url: string;
}

type AppStep = 'extent' | 'packages' | 'download' | 'processing' | 'complete';
```

**Components:** Function components with hooks. Props interfaces defined above component.
```tsx
interface MapSelectorProps {
  onExtentChange: (bounds: L.LatLngBounds) => void;
}

function MapSelector({ onExtentChange }: MapSelectorProps) {
  // ...
}
```

**State Management:** useState for local state, useCallback for event handlers.
```tsx
const [step, setStep] = useState<AppStep>('extent');
const handleMapExtentChange = useCallback((bounds: L.LatLngBounds) => {
  setExtent({ minLon: bounds.getWest(), ... });
}, []);
```

**Error Handling:** Try-catch with user-friendly messages via setError state.
```tsx
try {
  const response = await fetch(`${API_BASE}/packages/query`, { ... });
  if (!response.ok) throw new Error(`Server error: ${response.status}`);
  // ...
} catch (err) {
  setError(`Failed to query packages: ${err}`);
}
```

**Async Operations:** Use async/await. Cleanup EventSource in useEffect return.
```tsx
useEffect(() => {
  const eventSource = new EventSource(url);
  eventSource.onmessage = (event) => { /* ... */ };
  return () => eventSource.close();
}, [dependency]);
```

### Rust

**Imports:** Group by std → external crates → crate modules
```rust
use std::sync::Arc;
use std::collections::HashMap;

use axum::{extract::State, Json};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::api_types::{QueryRequest, Package};
```

**Error Handling:** Use `thiserror` for custom errors, `Result<T, String>` for handlers.
```rust
#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
}
```

**Structs:** Derive Debug, Clone, Serialize, Deserialize as needed.
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub package_name: String,
    pub size_gb: f64,
    pub download_url: String,
}
```

**Naming:** snake_case for functions/variables, PascalCase for types.
```rust
pub async fn query_by_extent(&self, bbox: &BoundingBox) -> Result<Vec<Package>, Error> {
    let mut all_packages = Vec::new();
    // ...
}
```

**Tests:** Use `#[cfg(test)]` modules. Test functions prefixed with `test_`.
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bounding_box_to_esri_geometry() {
        let bbox = BoundingBox::new(-9351879.0, 5097937.0, -9321521.0, 6408629.0, 3857);
        let esri = bbox.to_esri_geometry();
        assert!(esri.contains("xmin"));
    }
}
```

**Async:** Use tokio. Spawn background tasks with `tokio::spawn`.
```rust
tokio::spawn(async move {
    if let Err(e) = run_download_job(...).await {
        eprintln!("Download job error: {}", e);
    }
});
```

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/packages/query` | Query packages by extent |
| POST | `/api/download/start` | Start download job |
| GET | `/api/download/{id}/progress` | SSE progress stream |
| GET | `/api/download/{id}/file` | Download processed file |
| GET | `/api/health` | Health check |

## Key Files

- `src/App.tsx` - Main React component with all UI logic
- `src/types/index.ts` - TypeScript type definitions
- `src-server/src/routes.rs` - API route handlers
- `src-server/src/api_types.rs` - Shared types and ArcGIS response parsing
- `src-server/src/download.rs` - Download manager with progress tracking
- `src-server/src/processing.rs` - GDAL raster processing
- `src-server/src/package_client.rs` - ArcGIS API client

## Testing Requirements

All code changes MUST include tests:
- **Rust:** Unit tests in `#[cfg(test)]` modules within each file
- **TypeScript:** Unit tests with Vitest (to be added)

Run tests before marking work complete:
- Backend: `cd src-server && cargo test`
- Frontend: `npm test` (when available)

## Notes

- Vite proxy forwards `/api/*` to `localhost:3000` in development
- Coordinates are in Web Mercator (EPSG:3857)
- GDAL must be installed for raster processing
- No comments in code unless explicitly requested
