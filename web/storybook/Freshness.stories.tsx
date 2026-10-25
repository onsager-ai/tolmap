import { FreshnessStatus } from '../src/components/SnapshotFreshnessNote';
import type { SnapshotFreshness } from '../../bindings/SnapshotFreshness';
export default { title: 'Tolmap/Snapshot freshness', component: FreshnessStatus };
const base: SnapshotFreshness = { indexed_commit: '1234567890abcdef', requested_commit: 'abcdef0123456789', state: 'queued', failure_reason: null, failure_code: null, retryable: false, job_id: null };
const story = (state: SnapshotFreshness['state'], value: SnapshotFreshness) => ({
  args: { value },
  play: async () => { document.documentElement.dataset.visualReady = `freshness-${state}`; },
});
export const Queued = story('queued', base);
export const Indexing = story('indexing', { ...base, state: 'indexing' });
export const Failed = story('failed', { ...base, state: 'failed', failure_reason: 'Worker stopped; the saved snapshot is available.', retryable: true });
export const Stale = story('stale', { ...base, state: 'stale' });
export const Complete = story('complete', { ...base, state: 'complete', requested_commit: base.indexed_commit });
