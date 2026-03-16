import { describe, expect, it } from 'vitest';

import { buildStartDownloadRequest, DEFAULT_COMPRESSION } from './downloadRequest';

describe('downloadRequest', () => {
  it('uses deflate compression for every start download request', () => {
    const request = buildStartDownloadRequest(
      [{ package_name: 'Tile A', download_url: 'https://example.com/a.zip' }],
      { min_x: 1, min_y: 2, max_x: 3, max_y: 4 }
    );

    expect(request).toEqual({
      packages: [{ package_name: 'Tile A', download_url: 'https://example.com/a.zip' }],
      clip_extent: { min_x: 1, min_y: 2, max_x: 3, max_y: 4 },
      compression: DEFAULT_COMPRESSION,
    });
  });

  it('preserves a null clip extent while still forcing deflate', () => {
    const request = buildStartDownloadRequest([], null);

    expect(request.clip_extent).toBeNull();
    expect(request.compression).toBe('deflate');
  });
});
