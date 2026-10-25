import { FreshnessStatus } from '../src/components/SnapshotFreshnessNote';
import type { SnapshotFreshness } from '../../bindings/SnapshotFreshness';
export default { title: 'Tolmap/Snapshot freshness', component: FreshnessStatus };
const base: SnapshotFreshness = { indexed_commit: '1234567890abcdef', requested_commit: 'abcdef0123456789', state: 'queued', failure_reason: null, failure_code: null, retryable: false, job_id: null };
export const Queued = { args: { value: base } };
export const Indexing = { args: { value: { ...base, state: 'indexing' } } };
export const Failed = { args: { value: { ...base, state: 'failed', failure_reason: 'Worker stopped; the saved snapshot is available.', retryable: true } } };
export const Stale = { args: { value: { ...base, state: 'stale' } } };
export const Complete = { args: { value: { ...base, state: 'complete', requested_commit: base.indexed_commit } } };
