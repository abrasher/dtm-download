import { useEffect, useMemo, useRef, useState } from 'react';
import L from 'leaflet';
import 'leaflet/dist/leaflet.css';
import {
  type CoverageMode,
  buildProjectOptions,
  getDefaultProjectOptionKey,
  getFallbackProjectOptionKey,
  resolvePackagesForCoverageMode,
} from './utils/projectSelection';
import {
  buildDownloadProgressMap,
  getProcessingStageDescription,
  shouldTriggerAutoDownload,
  type DownloadProgress,
  type JobStatusResponse,
  type ProcessingProgress,
} from './utils/downloadPolling';
import { buildStartDownloadRequest, DEFAULT_COMPRESSION } from './utils/downloadRequest';
import {
  buildDownloadFileUrl,
  buildQgisStyleFilename,
  buildQgisStyleUrl,
  triggerBrowserDownload,
} from './utils/fileDownload';
import { searchOntarioLocations, type LocationSearchResult } from './utils/locationSearch';
import './App.css';

const API_BASE = '/api';

interface Package {
  package_name: string;
  size_gb: number;
  resolution: number;
  download_url: string;
  project: string;
  year_range: string | null;
  coverage_km2: number;
  geometry: {
    type: string;
    coordinates: number[][][];
  };
}

type AppStep = 'extent' | 'packages' | 'download' | 'processing' | 'complete';

const PROJECT_COLORS: Record<string, string> = {
  'OMAFRA Lidar 2016-18': '#2563eb',
  'OMAFRA Lidar 2022': '#7c3aed',
  'LEAP 2009': '#059669',
  'CLOCA Lidar 2018': '#dc2626',
  'SNC Lidar 2018-19': '#ea580c',
  'GTA 2014-18': '#0891b2',
  'York-LakeSimcoe 2019': '#be185d',
  'Ottawa River 2019-20': '#4f46e5',
  'Lake Nipissing 2020': '#16a34a',
  'Ottawa-Gatineau 2019-20': '#9333ea',
  'Hamilton-Niagara 2021': '#e11d48',
  'Belleville 2022': '#0d9488',
  'Eastern Ontario 2021-22': '#c026d3',
  'Huron Shores 2021': '#65a30d',
  'Muskoka 2018': '#b91c1c',
  'Muskoka 2021': '#a21caf',
  'Muskoka 2023': '#86198f',
  'DEDSFM Huron-Georgian Bay': '#15803d',
};

function getProjectColor(project: string): string {
  return PROJECT_COLORS[project] || '#6b7280';
}

function App() {
  const [step, setStep] = useState<AppStep>('extent');
  const [extent, setExtent] = useState<{ minLon: number; minLat: number; maxLon: number; maxLat: number } | null>(null);
  const [packages, setPackages] = useState<Package[]>([]);
  const [selectedProjectKey, setSelectedProjectKey] = useState<string | null>(null);
  const [coverageMode, setCoverageMode] = useState<CoverageMode>('selected-only');
  const [totalSizeGb, setTotalSizeGb] = useState(0);
  const [downloadProgress, setDownloadProgress] = useState<Map<string, DownloadProgress>>(new Map());
  const [processingProgress, setProcessingProgress] = useState<ProcessingProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [downloadId, setDownloadId] = useState<string | null>(null);
  const [outputFilename, setOutputFilename] = useState<string | null>(null);
  const [autoDownloadState, setAutoDownloadState] = useState<'idle' | 'succeeded' | 'failed'>('idle');
  const [downloadActionError, setDownloadActionError] = useState<string | null>(null);
  const [locationQuery, setLocationQuery] = useState('');
  const [locationResults, setLocationResults] = useState<LocationSearchResult[]>([]);
  const [locationSearchError, setLocationSearchError] = useState<string | null>(null);
  const [locationSearchLoading, setLocationSearchLoading] = useState(false);
  const [includeQgisStyle, setIncludeQgisStyle] = useState(false);

  const mapRef = useRef<L.Map | null>(null);
  const rectangleRef = useRef<L.Rectangle | null>(null);
  const footprintsRef = useRef<L.GeoJSON | null>(null);
  const locationMarkerRef = useRef<L.CircleMarker | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const autoDownloadAttemptRef = useRef<string | null>(null);
  const locationSearchAbortRef = useRef<AbortController | null>(null);

  const wgs84ToWebMercator = (lon: number, lat: number): [number, number] => {
    const x = lon * 20037508.34 / 180;
    const y = Math.log(Math.tan((90 + lat) * Math.PI / 360)) / (Math.PI / 180) * 20037508.34 / 180;
    return [x, y];
  };

  const projectOptions = useMemo(() => buildProjectOptions(packages), [packages]);
  const selectedProjectOption = useMemo(
    () => projectOptions.find((option) => option.key === selectedProjectKey) || null,
    [projectOptions, selectedProjectKey]
  );
  const fallbackProjectKey = useMemo(
    () => getFallbackProjectOptionKey(selectedProjectKey, projectOptions),
    [selectedProjectKey, projectOptions]
  );
  const fallbackProjectOption = useMemo(
    () => projectOptions.find((option) => option.key === fallbackProjectKey) || null,
    [projectOptions, fallbackProjectKey]
  );
  const selectedPackages = useMemo(
    () => resolvePackagesForCoverageMode(packages, selectedProjectKey, coverageMode),
    [packages, selectedProjectKey, coverageMode]
  );

  useEffect(() => {
    if (fallbackProjectKey) {
      setCoverageMode('prefer-selected-with-fallback');
      return;
    }
    setCoverageMode('selected-only');
  }, [fallbackProjectKey, selectedProjectKey]);

  useEffect(() => {
    if (!containerRef.current || mapRef.current) return;

    const map = L.map(containerRef.current, {
      zoomControl: false
    }).setView([45.0, -79.0], 6);

    L.control.zoom({ position: 'bottomright' }).addTo(map);

    L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', {
      attribution: '© OpenStreetMap contributors'
    }).addTo(map);

    let isDrawing = false;
    let startCorner: L.LatLng | null = null;

    map.on('mousedown', (e: L.LeafletMouseEvent) => {
      if (e.originalEvent.shiftKey && step === 'extent') {
        isDrawing = true;
        startCorner = e.latlng;
        map.dragging.disable();
      }
    });

    map.on('mousemove', (e: L.LeafletMouseEvent) => {
      if (isDrawing && startCorner) {
        const bounds = L.latLngBounds(startCorner, e.latlng);

        if (rectangleRef.current) {
          rectangleRef.current.setBounds(bounds);
        } else {
          rectangleRef.current = L.rectangle(bounds, {
            color: '#3b82f6',
            weight: 2,
            fillOpacity: 0.2
          }).addTo(map);
        }
      }
    });

    map.on('mouseup', () => {
      if (isDrawing && rectangleRef.current) {
        const bounds = rectangleRef.current.getBounds();
        setExtent({
          minLon: bounds.getWest(),
          minLat: bounds.getSouth(),
          maxLon: bounds.getEast(),
          maxLat: bounds.getNorth(),
        });
      }
      isDrawing = false;
      map.dragging.enable();
    });

    mapRef.current = map;

    return () => {
      map.remove();
      mapRef.current = null;
    };
  }, []);

  useEffect(() => {
    if (!mapRef.current) return;

    if (footprintsRef.current) {
      mapRef.current.removeLayer(footprintsRef.current);
      footprintsRef.current = null;
    }

    if (selectedPackages.length === 0) return;

    const geoJsonData = {
      type: 'FeatureCollection' as const,
      features: selectedPackages.map(pkg => ({
        type: 'Feature' as const,
        properties: {
          name: pkg.package_name,
          project: pkg.project
        },
        geometry: pkg.geometry
      }))
    };

    const geoJsonLayer = L.geoJSON(geoJsonData, {
      style: (feature) => ({
        color: getProjectColor(feature?.properties?.project || ''),
        weight: 2,
        fillOpacity: 0.3,
        opacity: 0.8
      }),
      onEachFeature: (feature, layer) => {
        layer.bindTooltip(feature.properties?.name || '', {
          permanent: false,
          direction: 'center',
          className: 'package-tooltip'
        });
      }
    }).addTo(mapRef.current);

    footprintsRef.current = geoJsonLayer;

    const bounds = geoJsonLayer.getBounds();
    if (bounds.isValid() && !extent) {
      mapRef.current.fitBounds(bounds, { padding: [50, 50] });
    }
  }, [selectedPackages, extent]);

  const focusLocation = (location: LocationSearchResult) => {
    if (!mapRef.current) return;

    if (locationMarkerRef.current) {
      mapRef.current.removeLayer(locationMarkerRef.current);
    }

    locationMarkerRef.current = L.circleMarker([location.latitude, location.longitude], {
      color: '#1d4ed8',
      fillColor: '#3b82f6',
      fillOpacity: 0.9,
      interactive: false,
      radius: 7,
      weight: 3,
    }).addTo(mapRef.current);

    if (location.boundingBox) {
      const [south, north, west, east] = location.boundingBox;
      mapRef.current.fitBounds([[south, west], [north, east]], {
        maxZoom: 15,
        padding: [40, 40],
      });
    } else {
      mapRef.current.setView([location.latitude, location.longitude], 13);
    }
  };

  const handleLocationSearch = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!locationQuery.trim()) return;

    locationSearchAbortRef.current?.abort();
    const controller = new AbortController();
    locationSearchAbortRef.current = controller;
    setLocationSearchLoading(true);
    setLocationSearchError(null);
    setLocationResults([]);

    try {
      const results = await searchOntarioLocations(locationQuery, controller.signal);
      setLocationResults(results);

      if (results.length === 0) {
        setLocationSearchError('No matching location found in Ontario. Try a nearby town or postal code.');
        return;
      }

      focusLocation(results[0]);
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') return;
      setLocationSearchError('Could not search for that location. Please try again.');
    } finally {
      if (locationSearchAbortRef.current === controller) {
        setLocationSearchLoading(false);
      }
    }
  };

  const handleSearchPackages = async () => {
    if (!extent) {
      setError('Please select an extent on the map first.');
      return;
    }

    setLoading(true);
    setError(null);

    try {
      const [min_x, min_y] = wgs84ToWebMercator(extent.minLon, extent.minLat);
      const [max_x, max_y] = wgs84ToWebMercator(extent.maxLon, extent.maxLat);

      const response = await fetch(`${API_BASE}/packages/query`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ min_x, min_y, max_x, max_y }),
      });

      if (!response.ok) {
        const errorText = await response.text();
        throw new Error(`Server error: ${response.status} - ${errorText}`);
      }

      const result = await response.json();

      if (result.packages.length === 0) {
        setError('No DTM packages found for this area. Try selecting a different region.');
        setLoading(false);
        return;
      }

      setPackages(result.packages);
      setTotalSizeGb(result.total_size_gb);
      setSelectedProjectKey(getDefaultProjectOptionKey(result.packages));

      setStep('packages');
    } catch (err) {
      setError(`Failed to query packages: ${err}`);
    } finally {
      setLoading(false);
    }
  };

  const downloadFile = async (id: string, filename: string, automatic = false) => {
    setDownloadActionError(null);

    try {
      triggerBrowserDownload(document, buildDownloadFileUrl(id), filename);
      setAutoDownloadState('succeeded');
    } catch (err) {
      const message = `Failed to download file: ${err}`;
      setDownloadActionError(message);

      if (automatic) {
        setAutoDownloadState('failed');
        return;
      }

      setError(message);
    }
  };

  useEffect(() => {
    if (!downloadId) return;

    let cancelled = false;
    let polling = false;
    let intervalId: number | null = null;

    const pollStatus = async () => {
      if (cancelled || polling) return;
      polling = true;

      try {
        const response = await fetch(`${API_BASE}/download/${downloadId}/progress`, {
          cache: 'no-store',
        });

        if (!response.ok) {
          throw new Error(`Server error: ${response.status}`);
        }

        const status: JobStatusResponse = await response.json();
        if (cancelled) return;

        setDownloadProgress(buildDownloadProgressMap(status.download_progress));
        setProcessingProgress(status.processing_progress);
        setOutputFilename(status.output_filename);

        if (status.status === 'error' && status.error) {
          setError(status.error);
          setStep('packages');
          setDownloadId(null);
          if (intervalId !== null) {
            window.clearInterval(intervalId);
          }
          return;
        }

        if (status.status === 'complete') {
          setStep('complete');
          if (intervalId !== null) {
            window.clearInterval(intervalId);
          }

          if (shouldTriggerAutoDownload(status, downloadId, autoDownloadAttemptRef.current)) {
            autoDownloadAttemptRef.current = downloadId;
            void downloadFile(downloadId, status.output_filename!, true);
          }

          return;
        }

        if (status.processing_progress) {
          setStep('processing');
          return;
        }

        setStep('download');
      } catch (err) {
        if (!cancelled) {
          console.error('Failed to poll job progress', err);
        }
      } finally {
        polling = false;
      }
    };

    void pollStatus();
    intervalId = window.setInterval(() => {
      void pollStatus();
    }, 1000);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [downloadId]);

  const handleStartDownload = async () => {
    if (!selectedProjectKey) {
      setError('Please select a dataset version.');
      return;
    }

    if (selectedPackages.length === 0) {
      setError('No packages selected.');
      return;
    }

    setStep('download');
    setError(null);
    setDownloadActionError(null);
    setAutoDownloadState('idle');
    setOutputFilename(null);
    setDownloadProgress(new Map());
    setProcessingProgress(null);
    autoDownloadAttemptRef.current = null;

    let clip_extent = null;
    if (extent) {
      const [min_x, min_y] = wgs84ToWebMercator(extent.minLon, extent.minLat);
      const [max_x, max_y] = wgs84ToWebMercator(extent.maxLon, extent.maxLat);
      clip_extent = { min_x, min_y, max_x, max_y };
    }

    try {
      const response = await fetch(`${API_BASE}/download/start`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(buildStartDownloadRequest(selectedPackages, clip_extent)),
      });

      if (!response.ok) {
        const errorText = await response.text();
        throw new Error(`Server error: ${response.status} - ${errorText}`);
      }

      const result = await response.json();

      setDownloadId(result.download_id);
    } catch (err) {
      setError(`Failed to start download: ${err}`);
      setStep('packages');
    }
  };

  const formatBytes = (bytes: number): string => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
  };

  const formatSpeed = (bps: number): string => {
    if (bps === 0) return '0 B/s';
    const k = 1024;
    const sizes = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
    const i = Math.floor(Math.log(bps) / Math.log(k));
    return parseFloat((bps / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
  };

  const formatETA = (seconds: number | null): string => {
    if (seconds === null || seconds <= 0) return '';
    if (seconds < 60) return `${seconds}s remaining`;
    if (seconds < 3600) {
      const mins = Math.floor(seconds / 60);
      const secs = seconds % 60;
      return `${mins}m ${secs}s remaining`;
    }
    const hours = Math.floor(seconds / 3600);
    const mins = Math.floor((seconds % 3600) / 60);
    return `${hours}h ${mins}m remaining`;
  };

  const resetApp = () => {
    setStep('extent');
    setExtent(null);
    setPackages([]);
    setSelectedProjectKey(null);
    setCoverageMode('selected-only');
    setTotalSizeGb(0);
    setDownloadProgress(new Map());
    setProcessingProgress(null);
    setDownloadId(null);
    setOutputFilename(null);
    setAutoDownloadState('idle');
    setDownloadActionError(null);
    setLocationQuery('');
    setLocationResults([]);
    setLocationSearchError(null);
    setIncludeQgisStyle(false);
    setError(null);
    locationSearchAbortRef.current?.abort();
    locationSearchAbortRef.current = null;
    autoDownloadAttemptRef.current = null;
    if (rectangleRef.current && mapRef.current) {
      mapRef.current.removeLayer(rectangleRef.current);
      rectangleRef.current = null;
    }
    if (footprintsRef.current && mapRef.current) {
      mapRef.current.removeLayer(footprintsRef.current);
      footprintsRef.current = null;
    }
    if (locationMarkerRef.current && mapRef.current) {
      mapRef.current.removeLayer(locationMarkerRef.current);
      locationMarkerRef.current = null;
    }
    mapRef.current?.setView([45.0, -79.0], 6);
  };

  const formatProcessingStageLabel = (stage: string | null): string => {
    switch (stage) {
      case 'preparing_inputs':
        return 'Preparing Inputs';
      case 'building_vrt':
        return 'Building VRT';
      case 'clipping':
        return 'Clipping Area';
      case 'merging':
        return 'Merging Tiles';
      case 'creating_cog':
        return 'Building COG';
      case 'finalizing':
        return 'Finalizing Output';
      case 'completed':
        return 'Complete';
      default:
        return 'Processing';
    }
  };

  return (
    <div className="app">
      <div ref={containerRef} className="full-map" />
      
      <div className="overlay-ui">
        <header className="app-header">
          <h1>Ontario DTM Downloader</h1>
          <div className="stepper">
            <div className={`step ${step === 'extent' ? 'active' : ''} ${['packages', 'download', 'processing', 'complete'].includes(step) ? 'completed' : ''}`}>
              1. Select
            </div>
            <div className={`step ${step === 'packages' ? 'active' : ''} ${['download', 'processing', 'complete'].includes(step) ? 'completed' : ''}`}>
              2. Choose
            </div>
            <div className={`step ${step === 'download' ? 'active' : ''} ${['processing', 'complete'].includes(step) ? 'completed' : ''}`}>
              3. Retrieve
            </div>
            <div className={`step ${step === 'processing' ? 'active' : ''} ${step === 'complete' ? 'completed' : ''}`}>
              4. Process
            </div>
            <div className={`step ${step === 'complete' ? 'active' : ''}`}>
              5. Done
            </div>
          </div>
        </header>

        {error && (
          <div className="error-banner">
            {error}
            <button onClick={() => setError(null)}>×</button>
          </div>
        )}

        {step === 'extent' && (
          <div className="control-panel">
            <h2>Step 1: Select Your Area</h2>
            <form className="location-search" onSubmit={handleLocationSearch}>
              <label htmlFor="location-search-input">Find a location in Ontario</label>
              <div className="location-search-row">
                <input
                  id="location-search-input"
                  type="search"
                  value={locationQuery}
                  onChange={(event) => setLocationQuery(event.target.value)}
                  placeholder="Address, town, postal code, or landmark"
                  autoComplete="street-address"
                />
                <button type="submit" disabled={!locationQuery.trim() || locationSearchLoading}>
                  {locationSearchLoading ? 'Finding…' : 'Find'}
                </button>
              </div>
            </form>

            {locationSearchError && <p className="location-search-error">{locationSearchError}</p>}

            {locationResults.length > 0 && (
              <div className="location-results" aria-label="Location search results">
                {locationResults.map((result) => (
                  <button
                    key={`${result.latitude}-${result.longitude}-${result.displayName}`}
                    type="button"
                    onClick={() => focusLocation(result)}
                  >
                    {result.displayName}
                  </button>
                ))}
              </div>
            )}

            <p className="hint">Hold <kbd>Shift</kbd> + Click and drag to draw a rectangle</p>
            
            {extent && (
              <div className="extent-info">
                <strong>Selected:</strong>{' '}
                {extent.minLat.toFixed(3)}° to {extent.maxLat.toFixed(3)}° N,{' '}
                {extent.minLon.toFixed(3)}° to {extent.maxLon.toFixed(3)}° W
              </div>
            )}

            <button
              className="primary-button"
              onClick={handleSearchPackages}
              disabled={!extent || loading}
            >
              {loading ? 'Searching...' : 'Search for Packages'}
            </button>
          </div>
        )}

        {step === 'packages' && (
          <div className="control-panel packages-panel">
            <h2>Step 2: Choose Data To Use</h2>
            <p className="summary">
              Found {packages.length} packages ({totalSizeGb.toFixed(2)} GB total)
            </p>
            <p className="hint">Select the dataset version and coverage handling. Cached packages are reused automatically.</p>

            {projectOptions.length > 1 && (
              <div className="project-selector">
                <label>Dataset Version:</label>
                <select
                  value={selectedProjectKey || ''}
                  onChange={(e) => setSelectedProjectKey(e.target.value)}
                >
                  {projectOptions.map((option) => <option key={option.key} value={option.key}>{option.label}</option>)}
                </select>
              </div>
            )}

            {selectedProjectOption && fallbackProjectOption && (
              <div className="coverage-mode-panel">
                <p className="coverage-mode-title">Coverage Handling</p>
                <label className="coverage-mode-option">
                  <input
                    type="radio"
                    name="coverage-mode"
                    value="prefer-selected-with-fallback"
                    checked={coverageMode === 'prefer-selected-with-fallback'}
                    onChange={() => setCoverageMode('prefer-selected-with-fallback')}
                  />
                  <span>
                    Blend (recommended): use {selectedProjectOption.label} where available and fill the rest with {fallbackProjectOption.label}.
                  </span>
                </label>
                <label className="coverage-mode-option">
                  <input
                    type="radio"
                    name="coverage-mode"
                    value="selected-only"
                    checked={coverageMode === 'selected-only'}
                    onChange={() => setCoverageMode('selected-only')}
                  />
                  <span>
                    Use only {selectedProjectOption.label} (fastest, but uncovered areas may be blank).
                  </span>
                </label>
                <label className="coverage-mode-option">
                  <input
                    type="radio"
                    name="coverage-mode"
                    value="fallback-only"
                    checked={coverageMode === 'fallback-only'}
                    onChange={() => setCoverageMode('fallback-only')}
                  />
                  <span>
                    Use only {fallbackProjectOption.label} (single source, consistent vintage).
                  </span>
                </label>
              </div>
            )}

            <div className="packages-scroll">
              <table className="package-table">
                <thead>
                  <tr>
                    <th>Package</th>
                    <th>Year</th>
                    <th>km²</th>
                    <th>Size</th>
                  </tr>
                </thead>
                <tbody>
                  {selectedPackages.map((pkg, i) => (
                    <tr key={i}>
                      <td>{pkg.package_name}</td>
                      <td>{pkg.year_range || '—'}</td>
                      <td>{pkg.coverage_km2.toFixed(0)}</td>
                      <td>{pkg.size_gb.toFixed(2)} GB</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            <div className="package-summary">
              {selectedPackages.length} packages • {selectedPackages.reduce((s, p) => s + p.coverage_km2, 0).toFixed(0)} km² • {selectedPackages.reduce((s, p) => s + p.size_gb, 0).toFixed(2)} GB
            </div>
            <label className="qgis-style-option">
              <input
                type="checkbox"
                checked={includeQgisStyle}
                onChange={(event) => setIncludeQgisStyle(event.target.checked)}
              />
              <span>
                <strong>Include QGIS terrain style</strong>
                <small>Creates a portable .qlr with dynamic elevation colour over an on-the-fly hillshade.</small>
              </span>
            </label>
            <div className="button-group">
              <button className="secondary-button" onClick={() => setStep('extent')}>Back</button>
              <button className="primary-button" onClick={handleStartDownload}>Use Selected Data</button>
            </div>
          </div>
        )}

        {step === 'download' && (
          <div className="control-panel">
            <h2>Step 3: Retrieving Data</h2>
            <div className="download-progress">
              {Array.from(downloadProgress.entries()).map(([name, progress]) => {
                const isExtracting = progress.status === 'Extracting...';
                const isCompleted = progress.status === 'completed';
                const isSkipped = progress.status === 'already downloaded' || progress.status === 'already extracted';
                
                return (
                  <div key={name} className="progress-item">
                    <div className={`progress-label ${isCompleted || isSkipped ? 'status-complete' : isExtracting ? 'status-extracting' : 'status-downloading'}`}>
                      {name}: {isSkipped ? 'Using Cache' : isCompleted ? 'Ready' : isExtracting ? 'Extracting...' : 'Downloading...'}
                    </div>
                    <div className="progress-bar">
                      <div className="progress-fill" style={{ width: `${progress.percentage}%` }} />
                    </div>
                    <div className="progress-details">
                      {isExtracting ? (
                        <span>{progress.bytes_downloaded}/{progress.total_bytes} files</span>
                      ) : isSkipped ? null : (
                        <>
                          <span>{formatBytes(progress.bytes_downloaded)}/{formatBytes(progress.total_bytes)}</span>
                          {progress.speed_bps > 0 && <span> • {formatSpeed(progress.speed_bps)}</span>}
                          {progress.eta_seconds && <span> • {formatETA(progress.eta_seconds)}</span>}
                        </>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {step === 'processing' && (
          <div className="control-panel">
            <h2>Step 4: Processing</h2>
            <p>Large clips can take several minutes. The progress display below will update as each server-side phase completes.</p>
            {processingProgress && (
              <div className="processing-progress">
                <div className="progress-label">{formatProcessingStageLabel(processingProgress.stage)}: {processingProgress.percentage}%</div>
                <div className="progress-bar">
                  <div className="progress-fill" style={{ width: `${processingProgress.percentage}%` }} />
                </div>
                <div className="progress-details">
                  <span>{processingProgress.message}</span>
                </div>
                <p className="processing-stage-description">{getProcessingStageDescription(processingProgress)}</p>
                {processingProgress.message.includes('finished') && processingProgress.stage !== 'completed' && (
                  <p className="processing-stage-note">The previous GDAL command has finished. The next processing phase should begin reporting shortly.</p>
                )}
                {processingProgress.percentage >= 100 && processingProgress.stage !== 'completed' && !processingProgress.message.includes('finished') && (
                  <p className="processing-stage-note">This phase has finished. Large jobs can take a short time before the next phase starts reporting.</p>
                )}
              </div>
            )}
          </div>
        )}

        {step === 'complete' && (
          <div className="control-panel">
            <h2>Complete!</h2>
            <p>Your DTM is ready. If the automatic download did not start, use the button below.</p>
            <div className="info-box">
              <ul>
                <li>Format: Cloud Optimized GeoTIFF</li>
                <li>Compression: {DEFAULT_COMPRESSION.toUpperCase()}</li>
                <li>Resolution: 0.5m</li>
                <li>Vertical Datum: CGVD2013</li>
                {outputFilename && <li>File: {outputFilename}</li>}
              </ul>
            </div>
            {autoDownloadState === 'failed' && (
              <p className="processing-stage-note">Automatic download did not complete. Start it manually below.</p>
            )}
            {downloadActionError && <div className="error">{downloadActionError}</div>}
            <div className="button-group">
              {downloadId && outputFilename && (
                <button className="primary-button" onClick={() => void downloadFile(downloadId, outputFilename)}>
                  Download File
                </button>
              )}
              {includeQgisStyle && downloadId && outputFilename && (
                <button
                  className="secondary-button"
                  onClick={() => triggerBrowserDownload(
                    document,
                    buildQgisStyleUrl(downloadId),
                    buildQgisStyleFilename(outputFilename),
                  )}
                >
                  Download QGIS Style
                </button>
              )}
              <button className="secondary-button" onClick={resetApp}>Start New Download</button>
            </div>
            {includeQgisStyle && (
              <p className="qgis-style-note">Keep the .qlr beside the downloaded .tif, then add the .qlr to QGIS. Its colour range and hillshade redraw as the map canvas changes.</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

export default App;
