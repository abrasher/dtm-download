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

interface DownloadProgress {
  package_name: string;
  bytes_downloaded: number;
  total_bytes: number;
  percentage: number;
  speed_bps: number;
  eta_seconds: number | null;
  status: string;
}

interface ProcessingProgress {
  stage: string;
  percentage: number;
  message: string;
}

interface ProgressEvent {
  Download?: DownloadProgress;
  Processing?: ProcessingProgress;
  Complete?: { output_filename: string };
  Error?: { message: string };
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
  const [compression, setCompression] = useState<string>('deflate');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [downloadId, setDownloadId] = useState<string | null>(null);

  const mapRef = useRef<L.Map | null>(null);
  const rectangleRef = useRef<L.Rectangle | null>(null);
  const footprintsRef = useRef<L.GeoJSON | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

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

  useEffect(() => {
    if (!downloadId) return;

    const eventSource = new EventSource(`${API_BASE}/download/${downloadId}/progress`);
    
    eventSource.onmessage = (event) => {
      if (!event.data.startsWith('{')) return;
      try {
        const progress: ProgressEvent = JSON.parse(event.data);
        
        if (progress.Download) {
          setDownloadProgress(prev => {
            const next = new Map(prev);
            next.set(progress.Download!.package_name, progress.Download!);
            return next;
          });
        } else if (progress.Processing) {
          setProcessingProgress(progress.Processing);
          setStep('processing');
        } else if (progress.Complete) {
          setStep('complete');
          eventSource.close();
          downloadFile(downloadId, progress.Complete.output_filename);
        } else if (progress.Error) {
          setError(progress.Error.message);
          setStep('packages');
          eventSource.close();
        }
      } catch (e) {
        console.error('Failed to parse progress event', e);
      }
    };

    eventSource.onerror = () => {
      console.error('SSE error');
    };

    return () => eventSource.close();
  }, [downloadId]);

  const downloadFile = async (id: string, filename: string) => {
    try {
      const response = await fetch(`${API_BASE}/download/${id}/file`);
      const blob = await response.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
    } catch (err) {
      setError(`Failed to download file: ${err}`);
    }
  };

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

    let clip_extent = null;
    if (extent) {
      const [min_x, min_y] = wgs84ToWebMercator(extent.minLon, extent.minLat);
      const [max_x, max_y] = wgs84ToWebMercator(extent.maxLon, extent.maxLat);
      clip_extent = { min_x, min_y, max_x, max_y };
    }

    try {
      const result = await fetch(`${API_BASE}/download/start`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          packages: selectedPackages,
          clip_extent,
          compression,
        }),
      }).then(r => r.json());

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
    setError(null);
    if (rectangleRef.current && mapRef.current) {
      mapRef.current.removeLayer(rectangleRef.current);
      rectangleRef.current = null;
    }
    if (footprintsRef.current && mapRef.current) {
      mapRef.current.removeLayer(footprintsRef.current);
      footprintsRef.current = null;
    }
    mapRef.current?.setView([45.0, -79.0], 6);
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

            <div className="output-options">
              <label>Compression:</label>
              <select value={compression} onChange={(e) => setCompression(e.target.value)}>
                <option value="deflate">Deflate</option>
                <option value="zstd">ZSTD</option>
                <option value="lzma">LZMA</option>
              </select>
            </div>

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
            <p>Creating Cloud Optimized GeoTIFF...</p>
            {processingProgress && (
              <div className="processing-progress">
                <div className="progress-label">{processingProgress.stage}: {processingProgress.message}</div>
                <div className="progress-bar">
                  <div className="progress-fill" style={{ width: `${processingProgress.percentage}%` }} />
                </div>
              </div>
            )}
          </div>
        )}

        {step === 'complete' && (
          <div className="control-panel">
            <h2>Complete!</h2>
            <p>Your DTM is ready and has been downloaded.</p>
            <div className="info-box">
              <ul>
                <li>Format: Cloud Optimized GeoTIFF</li>
                <li>Compression: {compression.toUpperCase()}</li>
                <li>Resolution: 0.5m</li>
                <li>Vertical Datum: CGVD2013</li>
              </ul>
            </div>
            <button className="primary-button" onClick={resetApp}>Start New Download</button>
          </div>
        )}
      </div>
    </div>
  );
}

export default App;
