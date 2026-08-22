export interface LocationSearchResult {
  displayName: string;
  latitude: number;
  longitude: number;
  boundingBox: [number, number, number, number] | null;
}

interface NominatimResult {
  boundingbox?: string[];
  display_name?: string;
  lat?: string;
  lon?: string;
}

export async function searchOntarioLocations(
  query: string,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch
): Promise<LocationSearchResult[]> {
  const trimmedQuery = query.trim();
  if (!trimmedQuery) return [];

  const params = new URLSearchParams({
    q: trimmedQuery,
  });
  const response = await fetcher(`/api/locations/search?${params}`, { signal });

  if (!response.ok) {
    throw new Error(`Location search returned ${response.status}`);
  }

  const results = await response.json() as NominatimResult[];
  return results.flatMap((result) => {
    const latitude = Number(result.lat);
    const longitude = Number(result.lon);
    if (!result.display_name || !Number.isFinite(latitude) || !Number.isFinite(longitude)) {
      return [];
    }

    const bounds = result.boundingbox?.map(Number);
    const boundingBox = bounds?.length === 4 && bounds.every(Number.isFinite)
      ? [bounds[0], bounds[1], bounds[2], bounds[3]] as [number, number, number, number]
      : null;

    return [{
      displayName: result.display_name,
      latitude,
      longitude,
      boundingBox,
    }];
  });
}
