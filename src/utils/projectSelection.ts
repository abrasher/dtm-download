export interface ProjectPackage {
  project: string;
  year_range: string | null;
  package_name: string;
  download_url?: string;
}

export interface ProjectOption {
  key: string;
  label: string;
  latestYear: number;
  groupKey: string;
}

const FOUR_DIGIT_YEAR_REGEX = /\b(19|20)\d{2}\b/g;
const RANGE_YEAR_REGEX = /((?:19|20)\d{2})\s*[-/]\s*(\d{2,4})/;
const PROJECT_YEAR_FRAGMENT_REGEX = /\b(?:19|20)\d{2}(?:\s*[-/]\s*(?:\d{2,4}))?\b/g;
const NON_ALPHANUMERIC_REGEX = /[^a-z0-9]+/g;
const UNSPECIFIED_VERSION = 'unspecified';

export type CoverageMode =
  | 'selected-only'
  | 'prefer-selected-with-fallback'
  | 'fallback-only';

function normalizeRangeEnd(startYear: number, rawEndYear: string): number {
  if (rawEndYear.length === 4) {
    return Number(rawEndYear);
  }

  const endTwoDigit = Number(rawEndYear);
  if (Number.isNaN(endTwoDigit)) {
    return Number.NEGATIVE_INFINITY;
  }

  const centuryBase = Math.floor(startYear / 100) * 100;
  let endYear = centuryBase + endTwoDigit;
  if (endYear < startYear) {
    endYear += 100;
  }
  return endYear;
}

function extractLatestYear(value: string | null | undefined): number {
  if (!value) {
    return Number.NEGATIVE_INFINITY;
  }

  let latestYear = Number.NEGATIVE_INFINITY;

  const rangeMatch = value.match(RANGE_YEAR_REGEX);
  if (rangeMatch) {
    const startYear = Number(rangeMatch[1]);
    const endYear = normalizeRangeEnd(startYear, rangeMatch[2]);
    latestYear = Math.max(latestYear, startYear, endYear);
  }

  const matches = value.match(FOUR_DIGIT_YEAR_REGEX);
  if (matches) {
    for (const match of matches) {
      latestYear = Math.max(latestYear, Number(match));
    }
  }

  return latestYear;
}

function getPackageLatestYear(pkg: ProjectPackage): number {
  return Math.max(
    extractLatestYear(pkg.year_range),
    extractLatestYear(pkg.project),
    extractLatestYear(pkg.package_name)
  );
}

function getVersionToken(pkg: ProjectPackage): string {
  if (pkg.year_range && pkg.year_range.trim().length > 0) {
    return pkg.year_range.trim();
  }

  const year = getPackageLatestYear(pkg);
  if (Number.isFinite(year)) {
    return String(year);
  }

  return UNSPECIFIED_VERSION;
}

function projectOptionLabel(project: string, versionToken: string): string {
  if (versionToken === UNSPECIFIED_VERSION) {
    return project;
  }
  if (project.includes(versionToken)) {
    return project;
  }
  return `${project} (${versionToken})`;
}

function normalizeProjectGroup(project: string): string {
  const withoutYears = project.toLowerCase().replace(PROJECT_YEAR_FRAGMENT_REGEX, ' ');
  const normalized = withoutYears.replace(NON_ALPHANUMERIC_REGEX, ' ').trim();
  if (normalized.length > 0) {
    return normalized;
  }
  return project.toLowerCase().trim();
}

function getPackageIdentity(pkg: ProjectPackage): string {
  if (pkg.download_url && pkg.download_url.trim().length > 0) {
    return pkg.download_url.trim();
  }
  return `${pkg.project}::${pkg.package_name}`;
}

export function getPackageOptionKey(pkg: ProjectPackage): string {
  const versionToken = getVersionToken(pkg);
  return `${pkg.project}::${versionToken}`;
}

export function buildProjectOptions(packages: ProjectPackage[]): ProjectOption[] {
  const optionMap = new Map<string, ProjectOption>();

  for (const pkg of packages) {
    const versionToken = getVersionToken(pkg);
    const key = `${pkg.project}::${versionToken}`;
    const latestYear = getPackageLatestYear(pkg);
    const label = projectOptionLabel(pkg.project, versionToken);
    const groupKey = normalizeProjectGroup(pkg.project);
    const existing = optionMap.get(key);

    if (!existing) {
      optionMap.set(key, { key, label, latestYear, groupKey });
      continue;
    }

    existing.latestYear = Math.max(existing.latestYear, latestYear);
  }

  return [...optionMap.values()].sort((a, b) => {
    if (a.latestYear !== b.latestYear) {
      if (Number.isFinite(a.latestYear) && Number.isFinite(b.latestYear)) {
        return b.latestYear - a.latestYear;
      }
      if (Number.isFinite(b.latestYear)) {
        return 1;
      }
      if (Number.isFinite(a.latestYear)) {
        return -1;
      }
    }
    return a.label.localeCompare(b.label);
  });
}

export function getDefaultProjectOptionKey(packages: ProjectPackage[]): string | null {
  const options = buildProjectOptions(packages);
  return options.length > 0 ? options[0].key : null;
}

export function getFallbackProjectOptionKey(
  selectedProjectKey: string | null,
  options: ProjectOption[]
): string | null {
  if (!selectedProjectKey) {
    return null;
  }

  const selectedOption = options.find((option) => option.key === selectedProjectKey);
  if (!selectedOption) {
    return null;
  }

  const candidateFallbacks = options.filter((option) => {
    if (option.key === selectedOption.key) {
      return false;
    }
    if (option.groupKey !== selectedOption.groupKey) {
      return false;
    }
    if (!Number.isFinite(selectedOption.latestYear)) {
      return false;
    }
    if (!Number.isFinite(option.latestYear)) {
      return false;
    }
    return option.latestYear < selectedOption.latestYear;
  });

  if (candidateFallbacks.length === 0) {
    return null;
  }

  candidateFallbacks.sort((a, b) => b.latestYear - a.latestYear || a.label.localeCompare(b.label));
  return candidateFallbacks[0].key;
}

export function resolvePackagesForCoverageMode<T extends ProjectPackage>(
  packages: T[],
  selectedProjectKey: string | null,
  mode: CoverageMode
): T[] {
  if (!selectedProjectKey) {
    return packages;
  }

  const selectedPackages = packages.filter((pkg) => getPackageOptionKey(pkg) === selectedProjectKey);
  if (selectedPackages.length === 0) {
    return [];
  }

  const fallbackProjectKey = getFallbackProjectOptionKey(selectedProjectKey, buildProjectOptions(packages));
  if (!fallbackProjectKey || mode === 'selected-only') {
    return selectedPackages;
  }

  const fallbackPackages = packages.filter((pkg) => getPackageOptionKey(pkg) === fallbackProjectKey);
  if (mode === 'fallback-only') {
    return fallbackPackages;
  }

  const mergedPackages: T[] = [];
  const seenPackages = new Set<string>();

  for (const pkg of [...fallbackPackages, ...selectedPackages]) {
    const identity = getPackageIdentity(pkg);
    if (seenPackages.has(identity)) {
      continue;
    }
    seenPackages.add(identity);
    mergedPackages.push(pkg);
  }

  return mergedPackages;
}
