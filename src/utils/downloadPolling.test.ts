import { describe, expect, it } from 'vitest';

import {
  buildDownloadProgressMap,
  getProcessingStageDescription,
  shouldTriggerAutoDownload,
  type JobStatusResponse,
} from './downloadPolling';

function createStatus(overrides: Partial<JobStatusResponse> = {}): JobStatusResponse {
  return {
    download_progress: [],
    processing_progress: null,
    output_filename: null,
    error: null,
    status: 'running',
    file_ready: false,
    ...overrides,
  };
}

describe('downloadPolling', () => {
  it('builds a package progress map from snapshot entries', () => {
    const progressMap = buildDownloadProgressMap([
      {
        package_name: 'Tile A',
        bytes_downloaded: 5,
        total_bytes: 10,
        percentage: 50,
        speed_bps: 25,
        eta_seconds: 2,
        status: 'downloading',
      },
    ]);

    expect(progressMap.get('Tile A')?.percentage).toBe(50);
  });

  it('only triggers auto download once a completed file is ready', () => {
    const status = createStatus({
      status: 'complete',
      file_ready: true,
      output_filename: 'result.tif',
    });

    expect(shouldTriggerAutoDownload(status, 'job-1', null)).toBe(true);
    expect(shouldTriggerAutoDownload(status, 'job-1', 'job-1')).toBe(false);
  });

  it('returns user-facing copy for long processing stages', () => {
    expect(
      getProcessingStageDescription({
        stage: 'creating_cog',
        percentage: 88,
        message: 'Creating Cloud Optimized GeoTIFF...'
      }),
    ).toContain('Cloud Optimized GeoTIFF');
  });
});
