import { describe, expect, it, vi } from 'vitest';

import { buildDownloadFileUrl, triggerBrowserDownload } from './fileDownload';

describe('fileDownload', () => {
  it('builds a file endpoint URL for the download id', () => {
    expect(buildDownloadFileUrl('job-123')).toBe('/api/download/job-123/file');
  });

  it('encodes reserved characters in the download id', () => {
    expect(buildDownloadFileUrl('job/123?x=1')).toBe('/api/download/job%2F123%3Fx%3D1/file');
  });

  it('triggers a browser-managed attachment download', () => {
    const click = vi.fn();
    const anchor = {
      href: '',
      download: '',
      rel: '',
      click,
    } as unknown as HTMLAnchorElement;
    const appendChild = vi.fn();
    const removeChild = vi.fn();
    const documentRef = {
      createElement: vi.fn(() => anchor),
      body: {
        appendChild,
        removeChild,
      },
    } as unknown as Pick<Document, 'createElement' | 'body'>;

    triggerBrowserDownload(documentRef, '/api/download/job-123/file', 'result.tif');

    expect(documentRef.createElement).toHaveBeenCalledWith('a');
    expect(anchor.href).toBe('/api/download/job-123/file');
    expect(anchor.download).toBe('result.tif');
    expect(anchor.rel).toBe('noopener');
    expect(appendChild).toHaveBeenCalledWith(anchor);
    expect(click).toHaveBeenCalledTimes(1);
    expect(removeChild).toHaveBeenCalledWith(anchor);
  });
});
