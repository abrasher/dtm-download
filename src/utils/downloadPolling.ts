export interface DownloadProgress {
  package_name: string;
  bytes_downloaded: number;
  total_bytes: number;
  percentage: number;
  speed_bps: number;
  eta_seconds: number | null;
  status: string;
}

export interface ProcessingProgress {
  stage: string;
  percentage: number;
  message: string;
}

export interface JobStatusResponse {
  download_progress: DownloadProgress[];
  processing_progress: ProcessingProgress | null;
  output_filename: string | null;
  error: string | null;
  status: 'running' | 'complete' | 'error';
  file_ready: boolean;
}

export function buildDownloadProgressMap(entries: DownloadProgress[]): Map<string, DownloadProgress> {
  return new Map(entries.map((entry) => [entry.package_name, entry]));
}

export function shouldTriggerAutoDownload(
  status: JobStatusResponse,
  downloadId: string | null,
  attemptedDownloadId: string | null,
): boolean {
  return Boolean(
    downloadId
    && status.status === 'complete'
    && status.file_ready
    && status.output_filename
    && attemptedDownloadId !== downloadId,
  );
}

export function getProcessingStageDescription(progress: ProcessingProgress | null): string {
  if (!progress) {
    return 'Preparing processing job...';
  }

  switch (progress.stage) {
    case 'preparing_inputs':
      return 'Preparing source rasters before clipping and packaging.';
    case 'clipping':
      return 'Cropping the merged terrain model to your selected area.';
    case 'merging':
      return 'Combining source rasters into one output.';
    case 'creating_cog':
      return 'Building the Cloud Optimized GeoTIFF structure and overviews.';
    case 'finalizing':
      return 'Final checks and preparing your file for download.';
    case 'completed':
      return 'Processing finished. Your file is ready.';
    default:
      return progress.message;
  }
}
