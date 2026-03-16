export const DEFAULT_COMPRESSION = 'deflate';

interface ClipExtentPayload {
  min_x: number;
  min_y: number;
  max_x: number;
  max_y: number;
}

export interface StartDownloadRequest<TPackage> {
  packages: TPackage[];
  clip_extent: ClipExtentPayload | null;
  compression: typeof DEFAULT_COMPRESSION;
}

export function buildStartDownloadRequest<TPackage>(
  packages: TPackage[],
  clipExtent: ClipExtentPayload | null
): StartDownloadRequest<TPackage> {
  return {
    packages,
    clip_extent: clipExtent,
    compression: DEFAULT_COMPRESSION,
  };
}
