export function buildDownloadFileUrl(downloadId: string): string {
  return `/api/download/${encodeURIComponent(downloadId)}/file`;
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
