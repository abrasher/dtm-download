export function buildDownloadFileUrl(downloadId: string): string {
  return `/api/download/${encodeURIComponent(downloadId)}/file`;
}

export function buildQgisStyleUrl(downloadId: string): string {
  return `/api/download/${encodeURIComponent(downloadId)}/qgis-style`;
}

export function buildQgisStyleFilename(rasterFilename: string): string {
  const lastDot = rasterFilename.lastIndexOf('.');
  const stem = lastDot > 0 ? rasterFilename.slice(0, lastDot) : rasterFilename;
  return `${stem || 'dtm_output'}_terrain.qlr`;
}

export function triggerBrowserDownload(
  documentRef: Pick<Document, 'createElement' | 'body'>,
  downloadUrl: string,
  filename: string,
): void {
  const anchor = documentRef.createElement('a');
  anchor.href = downloadUrl;
  anchor.download = filename;
  anchor.rel = 'noopener';
  documentRef.body.appendChild(anchor);
  anchor.click();
  documentRef.body.removeChild(anchor);
}
