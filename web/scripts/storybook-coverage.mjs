export const REQUIRED_STORIES = [
  'hosted-list', 'hosted-empty-connect', 'hosted-empty-add', 'add-list', 'add-selected-two', 'add-already-mapped', 'add-loading', 'add-empty', 'add-search-miss', 'add-unavailable', 'add-expired', 'batch-progress',
  'home', 'home-empty', 'home-loading', 'home-unavailable',
  'home-repositories', 'repository-history', 'repository-running', 'repository-empty', 'repository-no-capabilities', 'repository-delete-snapshot',
  'local-start', 'local-confirm', 'local-running', 'local-cancelling', 'local-detection-failed', 'local-index-failed', 'local-saved',
  'job-missing', 'job-queued', 'job-fetch', 'job-read', 'job-map', 'job-detail',
  'failed-detection-uncertain', 'failed-detection-failed', 'failed-cancelled', 'failed-clone-failed', 'failed-worker-crashed', 'failed-busy', 'failed-server-stopping', 'failed-rate-limited', 'failed-index-failed', 'failed-internal-error', 'failed-unknown',
  'map-overview', 'organization-overview', 'organization-card', 'organization-loading', 'organization-error', 'map-loading', 'map-unavailable', 'map-district', 'map-file', 'map-symbol', 'map-folder', 'map-layer-c', 'map-layer-x', 'map-layer-p', 'map-detail-pending',
  'map-lens-loaded', 'map-lens-unresolved', 'map-lens-empty', 'map-lens-error',
  'map-lens-compare-commits', 'map-lens-compare-overlap', 'map-lens-compare-loading', 'map-lens-compare-stale', 'map-lens-compare-error',
  'map-debt-loaded', 'map-debt-empty', 'map-debt-error', 'map-debt-compare',
  'map-search', 'map-search-empty', 'map-layers', 'map-repositories', 'map-quality', 'map-keyboard', 'map-path',
  'map-lens-walk-first', 'map-lens-walk-middle', 'map-lens-walk-last', 'map-lens-walk-unresolved',
  'source-ready', 'source-loading', 'source-unavailable', 'source-missing', 'source-expired', 'source-binary', 'source-large', 'source-unsupported', 'source-long-line', 'source-search', 'source-search-empty',
  'freshness-queued', 'freshness-indexing', 'freshness-failed', 'freshness-stale', 'freshness-complete', 'map-freshness',
  'reserved-about', 'reserved-docs', 'reserved-api', 'reserved-settings', 'reserved-assets', 'reserved-static', 'reserved-health', 'reserved-login', 'reserved-admin', 'reserved-search',
];

export function assertStorybookCoverage(cases, index, reservedNames) {
  const candidateIds = cases.map(item => item.id);
  const duplicateCases = candidateIds.filter((id, i) => candidateIds.indexOf(id) !== i);
  if (duplicateCases.length) throw new Error(`Duplicate catalogue story IDs: ${[...new Set(duplicateCases)].join(', ')}`);
  const candidateStoryIds = cases.map(item => item.storybookId ?? `tolmap-pages--${item.id}`);
  const duplicateStoryIds = candidateStoryIds.filter((id, i) => candidateStoryIds.indexOf(id) !== i);
  if (duplicateStoryIds.length) throw new Error(`Duplicate catalogue Storybook IDs: ${[...new Set(duplicateStoryIds)].join(', ')}`);
  const absentRequired = REQUIRED_STORIES.filter(id => !candidateIds.includes(id));
  if (absentRequired.length) throw new Error(`Required story states are absent: ${absentRequired.join(', ')}`);

  const entries = Object.values(index?.entries ?? {});
  const stories = entries.filter(entry => entry.type === 'story');
  const ids = stories.map(entry => entry.id);
  const duplicateStories = ids.filter((id, i) => ids.indexOf(id) !== i);
  const actual = new Set(ids);
  const expected = new Set(candidateStoryIds);
  const missing = [...expected].filter(id => !actual.has(id));
  const extra = [...actual].filter(id => !expected.has(id));
  if (duplicateStories.length || missing.length || extra.length) {
    throw new Error(`Storybook catalogue mismatch: missing=[${missing.join(', ')}], duplicate=[${[...new Set(duplicateStories)].join(', ')}], extra=[${extra.join(', ')}]`);
  }

  const reservedCandidates = candidateIds.filter(id => id.startsWith('reserved-')).map(id => id.slice('reserved-'.length)).sort();
  const expectedReserved = [...reservedNames].filter(name => name !== 'new' && name !== 'org').sort();
  if (JSON.stringify(reservedCandidates) !== JSON.stringify(expectedReserved)) {
    throw new Error(`Reserved route catalogue mismatch: stories=[${reservedCandidates.join(', ')}], production=[${expectedReserved.join(', ')}]`);
  }
  return { storyCount: expected.size, reservedCount: expectedReserved.length };
}

export function assertRouteTreeContract(source) {
  for (const fragment of ['filter((name) => name !== "new" && name !== "org")', 'path: `/${name}`', 'RESERVED_NAMES.has(params.owner)']) {
    if (!source.includes(fragment)) throw new Error(`Production routeTree is missing reserved-route guard: ${fragment}`);
  }
  const actual = [...source.matchAll(/\bpath:\s*(?:"([^"]+)"|`([^`]+)`)/g)].map(match => match[1] ?? match[2]).sort();
  const expected = ['/', '/new', '/new/batches/$batchId', '/org/$name', '/repositories/$repositoryId/history', '/$owner/$repo', '/${name}'].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error(`Production route path set changed: [${actual.join(', ')}]`);
  return true;
}

export function captureApplies(item, viewport, host) {
  return (!(item.id.startsWith('hosted-') || item.id.startsWith('add-') || item.id === 'batch-progress') || host === 'hub')
    && (host !== 'hub' || (!item.id.startsWith('reserved-') && !item.id.startsWith('local-')))
    && !(item.id === 'map-keyboard' && ['phone', 'landscape'].includes(viewport.name));
}

export function parseReservedNames(source) {
  const match = source.match(/export const RESERVED_NAMES = new Set\(\[([\s\S]*?)\]\);/);
  if (!match) throw new Error('Could not read production RESERVED_NAMES');
  const names = [...match[1].matchAll(/"([a-z-]+)"/g)].map(item => item[1]);
  if (!names.length || new Set(names).size !== names.length) throw new Error('Production RESERVED_NAMES is empty or duplicated');
  return names;
}
