// One inventory drives stories, evidence and route coverage. These are candidates.
export const cases = [
  ['home', 'Home', '/', 'home'],
  ['home-empty', 'Home empty', '/', 'empty'],
  ['home-loading', 'Home loading', '/', 'loading'],
  ['home-unavailable', 'Home unavailable', '/', 'unavailable'],
  ...['start', 'confirm', 'running', 'cancelling', 'detection-failed', 'index-failed', 'saved'].map(state => [`local-${state}`, `Local checkout · ${state}`, `local:${state}`, 'native-local']),
  ['job-missing', 'No job selected', '/new', 'home'],
  ...['queued', 'fetch', 'read', 'map', 'detail'].map(state => [`job-${state}`, `Indexing ${state}`, '/new?job=fixture&slug=onsager-ai/tolmap', state]),
  ...['detection_uncertain', 'detection_failed', 'cancelled', 'clone_failed', 'worker_crashed', 'busy', 'server_stopping', 'rate_limited', 'index_failed', 'internal_error', 'unknown'].map(code => [`failed-${code.replaceAll('_', '-')}`, `Failure · ${code}`, '/new?job=fixture&slug=onsager-ai/tolmap', code]),
  ['map-overview', 'Map overview', '/onsager-ai/tolmap', 'map-ready'],
  ['map-loading', 'Map loading', '/onsager-ai/tolmap', 'map-loading'],
  ['map-unavailable', 'Map unavailable', '/onsager-ai/tolmap', 'map-unavailable'],
  ['map-district', 'District selected', 'district', 'map-ready'],
  ['map-file', 'File selected', 'file', 'map-ready'],
  ['map-symbol', 'Symbol selected', 'symbol', 'map-ready'],
  ['map-folder', 'Folder selected', 'folder', 'map-ready'],
  ...['c', 'x', 'p'].map(layer => [`map-layer-${layer}`, `Map layer ${layer}`, `/onsager-ai/tolmap?layer=${layer}`, 'map-ready']),
  ['map-detail-pending', 'Detail still running', '/onsager-ai/tolmap?job=fixture', 'early'],
  ...['search', 'search-empty', 'layers', 'repositories', 'quality', 'keyboard', 'path'].map(overlay => [`map-${overlay}`, `Map · ${overlay}`, overlay === 'path' ? 'file' : '/onsager-ai/tolmap', 'map-ready', overlay]),
  ...['ready', 'loading', 'unavailable', 'missing', 'expired', 'binary', 'large', 'unsupported', 'long-line', 'search', 'search-empty'].map(state => [`source-${state}`, `Source · ${state}`, 'source', `source-${state}`]),
  ...['queued', 'indexing', 'failed', 'stale', 'complete'].map(state => [
    `freshness-${state}`, `Snapshot freshness · ${state}`, '/', 'freshness', '',
    `tolmap-snapshot-freshness--${state}`,
  ]),
  ...['about', 'docs', 'api', 'settings', 'assets', 'static', 'health', 'login', 'admin', 'search'].map(name => [`reserved-${name}`, `Reserved · /${name}`, `/${name}`, 'home']),
].map(([id, name, route, fixture, action, storybookId]) => ({ id, name, route, fixture, action, storybookId }));

export const viewports = [
  { name: 'phone', width: 390, height: 844, touch: true },
  { name: 'landscape', width: 844, height: 390, touch: true },
  { name: 'tablet', width: 820, height: 1180, touch: true },
  { name: 'desktop', width: 1440, height: 1000, touch: false },
];
