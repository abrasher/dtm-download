import { describe, expect, it } from 'vitest';

import {
  buildProjectOptions,
  getDefaultProjectOptionKey,
  getFallbackProjectOptionKey,
  getPackageOptionKey,
  resolvePackagesForCoverageMode,
} from './projectSelection';

describe('project selection', () => {
  it('prefers the latest version as default', () => {
    const packages = [
      { project: 'GTA', year_range: null, package_name: 'GTA 2015 Tile A' },
      { project: 'GTA', year_range: null, package_name: 'GTA 2023 Tile A' },
    ];

    expect(getDefaultProjectOptionKey(packages)).toBe('GTA::2023');
  });

  it('splits options by inferred version when project names match', () => {
    const packages = [
      { project: 'GTA', year_range: null, package_name: 'GTA 2015 Tile A' },
      { project: 'GTA', year_range: null, package_name: 'GTA 2023 Tile A' },
      { project: 'GTA', year_range: null, package_name: 'GTA 2023 Tile B' },
    ];

    const options = buildProjectOptions(packages);
    expect(options.map((o) => o.key)).toEqual(['GTA::2023', 'GTA::2015']);
  });

  it('keeps matching packages grouped under the same option key', () => {
    const pkgA = { project: 'GTA', year_range: null, package_name: 'GTA 2023 Tile A' };
    const pkgB = { project: 'GTA', year_range: null, package_name: 'GTA 2023 Tile B' };

    expect(getPackageOptionKey(pkgA)).toBe('GTA::2023');
    expect(getPackageOptionKey(pkgB)).toBe('GTA::2023');
  });

  it('finds the latest older fallback option in the same project group', () => {
    const packages = [
      { project: 'GTA 2014-18', year_range: null, package_name: 'GTA 2015 A' },
      { project: 'GTA 2023', year_range: null, package_name: 'GTA2023-DTM-01' },
      { project: 'Muskoka 2021', year_range: null, package_name: 'Muskoka 2021 A' },
    ];

    const options = buildProjectOptions(packages);
    const selectedKey = options.find((option) => option.key === 'GTA 2023::2023')?.key || null;
    const fallbackKey = getPackageOptionKey(packages[0]);
    expect(getFallbackProjectOptionKey(selectedKey, options)).toBe(fallbackKey);
  });

  it('resolves selected-only mode to selected version packages', () => {
    const packages = [
      { project: 'GTA 2014-18', year_range: null, package_name: 'GTA 2015 A' },
      { project: 'GTA 2014-18', year_range: null, package_name: 'GTA 2015 B' },
      { project: 'GTA 2023', year_range: null, package_name: 'GTA2023-DTM-01' },
    ];

    const selected = resolvePackagesForCoverageMode(
      packages,
      'GTA 2023::2023',
      'selected-only'
    );

    expect(selected.map((pkg) => pkg.package_name)).toEqual(['GTA2023-DTM-01']);
  });

  it('resolves blend mode to fallback first then selected', () => {
    const packages = [
      {
        project: 'GTA 2014-18',
        year_range: null,
        package_name: 'GTA 2015 A',
        download_url: 'https://example.com/gta-2015-a.zip',
      },
      {
        project: 'GTA 2014-18',
        year_range: null,
        package_name: 'GTA 2015 B',
        download_url: 'https://example.com/gta-2015-b.zip',
      },
      {
        project: 'GTA 2023',
        year_range: null,
        package_name: 'GTA2023-DTM-01',
        download_url: 'https://example.com/gta-2023-01.zip',
      },
    ];

    const selected = resolvePackagesForCoverageMode(
      packages,
      'GTA 2023::2023',
      'prefer-selected-with-fallback'
    );

    expect(selected.map((pkg) => pkg.package_name)).toEqual([
      'GTA 2015 A',
      'GTA 2015 B',
      'GTA2023-DTM-01',
    ]);
  });

  it('resolves fallback-only mode to fallback version packages', () => {
    const packages = [
      { project: 'GTA 2014-18', year_range: null, package_name: 'GTA 2015 A' },
      { project: 'GTA 2014-18', year_range: null, package_name: 'GTA 2015 B' },
      { project: 'GTA 2023', year_range: null, package_name: 'GTA2023-DTM-01' },
    ];

    const selected = resolvePackagesForCoverageMode(
      packages,
      'GTA 2023::2023',
      'fallback-only'
    );

    expect(selected.map((pkg) => pkg.package_name)).toEqual(['GTA 2015 A', 'GTA 2015 B']);
  });
});
