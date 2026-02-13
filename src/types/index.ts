// Types for Ontario DTM Package Index

export interface BoundingBox {
  xmin: number;
  ymin: number;
  xmax: number;
  ymax: number;
  spatialReference?: {
    wkid: number;
    latestWkid?: number;
  };
}

export interface Extent {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export interface GeoJSONPolygon {
  type: 'Polygon';
  coordinates: number[][][];
}

export interface GeoJSONFeature {
  type: 'Feature';
  geometry: GeoJSONPolygon;
  properties: Record<string, unknown>;
}

export interface DtmPackage {
  id: number;
  packageName: string;
  sizeGb: number;
  resolution: number;
  downloadUrl: string;
  project: string;
  geometry: GeoJSONPolygon;
  extent: Extent;
}

export interface Project {
  name: string;
  year: string;
  packages: DtmPackage[];
  totalSizeGb: number;
  resolution: number;
}

export interface PackageConflict {
  extent: Extent;
  projects: Project[];
  selectedProject?: string;
}

export interface DownloadProgress {
  packageId: number;
  packageName: string;
  bytesDownloaded: number;
  totalBytes: number;
  percentage: number;
  status: 'pending' | 'downloading' | 'extracting' | 'completed' | 'error';
  error?: string;
}

export interface ProcessingProgress {
  stage: 'merging' | 'clipping' | 'compressing' | 'writing_cog' | 'completed' | 'error';
  percentage: number;
  message: string;
  error?: string;
}

export interface OutputOptions {
  format: 'cog' | 'geotiff';
  compression: 'zstd' | 'lzma' | 'deflate' | 'lzw';
  compressionLevel: number;
  tileSize: 256 | 512;
  outputPath: string;
}

export interface AppState {
  step: 'extent' | 'packages' | 'download' | 'processing' | 'complete';
  extent: Extent | null;
  packages: DtmPackage[];
  conflicts: PackageConflict[];
  selectedPackages: DtmPackage[];
  downloadProgress: DownloadProgress[];
  processingProgress: ProcessingProgress | null;
  outputOptions: OutputOptions;
  outputPath: string | null;
  error: string | null;
}
