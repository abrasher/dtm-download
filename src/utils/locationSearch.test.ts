import { describe, expect, it, vi } from 'vitest';

import { searchOntarioLocations } from './locationSearch';

describe('Ontario location search', () => {
  it('scopes searches to Ontario and maps valid results', async () => {
    const fetcher = vi.fn<typeof fetch>(async () => new Response(JSON.stringify([
      {
        display_name: 'Kingston, Ontario, Canada',
        lat: '44.2307',
        lon: '-76.4813',
        boundingbox: ['44.18', '44.30', '-76.60', '-76.40'],
      },
    ])));

    const results = await searchOntarioLocations('  Kingston  ', undefined, fetcher);

    expect(fetcher).toHaveBeenCalledOnce();
    const request = fetcher.mock.calls[0]?.[0];
    if (!request) throw new Error('Expected a location search request');
    const requestUrl = new URL(
      request instanceof Request ? request.url : request.toString(),
      'http://localhost'
    );
    expect(requestUrl.pathname).toBe('/api/locations/search');
    expect(requestUrl.searchParams.get('q')).toBe('Kingston');
    expect(results).toEqual([{
      displayName: 'Kingston, Ontario, Canada',
      latitude: 44.2307,
      longitude: -76.4813,
      boundingBox: [44.18, 44.30, -76.60, -76.40],
    }]);
  });

  it('ignores malformed results', async () => {
    const fetcher = vi.fn<typeof fetch>(async () => new Response(JSON.stringify([
      { display_name: 'Missing coordinates' },
      { display_name: 'Invalid coordinates', lat: 'north', lon: 'west' },
    ])));

    await expect(searchOntarioLocations('somewhere', undefined, fetcher)).resolves.toEqual([]);
  });

  it('does not issue a request for an empty query', async () => {
    const fetcher = vi.fn<typeof fetch>();

    await expect(searchOntarioLocations('   ', undefined, fetcher)).resolves.toEqual([]);
    expect(fetcher).not.toHaveBeenCalled();
  });

  it('reports unsuccessful search responses', async () => {
    const fetcher = vi.fn<typeof fetch>(async () => new Response('', { status: 503 }));

    await expect(searchOntarioLocations('Ottawa', undefined, fetcher)).rejects.toThrow(
      'Location search returned 503'
    );
  });
});
