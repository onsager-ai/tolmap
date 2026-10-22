import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { chromium } from 'playwright';
import { cases } from '../storybook/catalogue.mjs';
import { assertRouteTreeContract, assertStorybookCoverage, captureApplies } from './storybook-coverage.mjs';
import { verifyVisualBaselines } from './visual-baselines.mjs';

let browser;
let root;
const capture = { id: 'home', host: 'oss', viewport: 'desktop', theme: 'light', file: 'home.png', status: 'passed', result: 'passed' };
const provenance = { browser: { name: 'Chromium', version: 'selftest' }, fixture: { sha256: 'fixture' }, environment: { locale: 'en-US', scale: 1 } };
const reserved = ['about', 'docs', 'api', 'new', 'settings', 'assets', 'static', 'health', 'login', 'admin', 'search'];

before(async () => { browser = await chromium.launch(); root = await fs.mkdtemp(path.join(os.tmpdir(), 'tolmap-visual-')); });
after(async () => { await browser?.close(); if (root) await fs.rm(root, { recursive: true, force: true }); });

test('catalogue, Storybook IDs and production route paths stay in exact coverage', async () => {
  const index = { entries: Object.fromEntries(cases.map(item => {
    const id = `tolmap-pages--${item.id}`;
    return [id, { id, type: 'story' }];
  })) };
  assert.equal(assertStorybookCoverage(cases, index, reserved).storyCount, cases.length);
  assert.throws(() => assertStorybookCoverage(cases.slice(1), index, reserved), /Required story states are absent/);
  assert.throws(() => assertStorybookCoverage(cases, { entries: {} }, reserved), /catalogue mismatch/);
  assert.throws(() => assertStorybookCoverage(cases, { entries: { ...index.entries, extra: { id: 'tolmap-pages--unexpected', type: 'story' } } }, reserved), /extra=\[tolmap-pages--unexpected\]/);
  assert.throws(() => assertStorybookCoverage(cases, { entries: { ...index.entries, duplicate: { id: 'tolmap-pages--home', type: 'story' } } }, reserved), /duplicate=\[tolmap-pages--home\]/);
  assert.throws(() => assertStorybookCoverage([...cases, cases[0]], index, reserved), /Duplicate catalogue story IDs/);
  assert.throws(() => assertStorybookCoverage(cases.filter(item => item.id !== 'home'), { entries: Object.fromEntries(cases.filter(item => item.id !== 'home').map(item => [`tolmap-pages--${item.id}`, { id: `tolmap-pages--${item.id}`, type: 'story' }])) }, reserved), /Required story states are absent/);
  assert.equal(captureApplies({ id: 'map-keyboard' }, { name: 'phone' }, 'oss'), false);
  assert.equal(captureApplies({ id: 'reserved-about' }, { name: 'desktop' }, 'hub'), false);
  assert.equal(captureApplies({ id: 'map-overview' }, { name: 'desktop' }, 'hub'), true);
  const routeSource = await fs.readFile(new URL('../src/router.tsx', import.meta.url), 'utf8');
  assert.equal(assertRouteTreeContract(routeSource), true);
  assert.throws(() => assertRouteTreeContract(routeSource.replace('path: "/new",', 'path: "/unexpected", path: "/new",')), /route path set changed/);
});

test('pending approval reports explicitly without pixel pass claims', async () => {
  const dirs = await scenario();
  const report = await verifyVisualBaselines({ browser, captures: [{ ...capture, status: 'not-run', result: undefined }], ...dirs, provenance, sourceRevision: 'candidate-revision' });
  assert.equal(report.status, 'pending-review');
  assert.match(report.note, /pixels were not compared/);
});

test('approved pixels pass only when byte-identical', async () => {
  const dirs = await scenario();
  await makeApproved(dirs);
  const report = await verifyVisualBaselines({ browser, captures: [capture], ...dirs, provenance, sourceRevision: 'candidate-revision' });
  assert.equal(report.status, 'passed');
  assert.equal(report.referenceSourceRevision, 'approved-revision');
});

test('one changed pixel fails and emits actual, reference and diff PNGs', async () => {
  const dirs = await scenario();
  await makeApproved(dirs);
  await writeSinglePixel(dirs.evidenceDir, 'home.png');
  await assert.rejects(verifyVisualBaselines({ browser, captures: [capture], ...dirs, provenance, sourceRevision: 'candidate-revision' }), /Visual baselines failed/);
  const files = await fs.readdir(dirs.evidenceDir);
  assert.ok(files.some(file => file.endsWith('.actual.png')));
  assert.ok(files.some(file => file.endsWith('.reference.png')));
  assert.ok(files.some(file => file.endsWith('.diff.png')));
  const report = JSON.parse(await fs.readFile(path.join(dirs.evidenceDir, 'visual-regression.json'), 'utf8'));
  assert.equal(report.status, 'failed');
  assert.equal(report.results[0].status, 'failed');
  assert.ok(report.results[0].diffEvidence);
});

test('dimension changes fail with a diff image', async () => {
  const dirs = await scenario();
  await makeApproved(dirs);
  await writePng(dirs.evidenceDir, 'home.png', 21, 20, '#00ff00');
  await assert.rejects(verifyVisualBaselines({ browser, captures: [capture], ...dirs, provenance, sourceRevision: 'candidate-revision' }), /Visual baselines failed/);
  assert.ok((await fs.readdir(dirs.evidenceDir)).some(file => file.endsWith('.diff.png')));
});

test('missing approved PNG and unapproved registries fail explicitly', async () => {
  const dirs = await scenario();
  await makeApproved(dirs);
  await fs.rm(path.join(dirs.baselineDir, 'home.png'));
  await assert.rejects(verifyVisualBaselines({ browser, captures: [capture], ...dirs, provenance, sourceRevision: 'candidate-revision' }), /Visual baselines failed/);
  const pending = await scenario({ schemaVersion: 1, status: 'pending-review', sourceRevision: 'invented', review: null, provenance: null, images: [] });
  await assert.rejects(verifyVisualBaselines({ browser, captures: [capture], ...pending, provenance }), /Registry must be pending-review/);
});

test('empty approved matrices, provenance changes, incomplete captures and invalid PNGs fail', async () => {
  const empty = await scenario({ schemaVersion: 1, status: 'approved', sourceRevision: 'approved-revision', review: {}, provenance, images: [] });
  await assert.rejects(verifyVisualBaselines({ browser, captures: [capture], ...empty, provenance }), /Registry must be pending-review/);
  const dirs = await scenario(); await makeApproved(dirs);
  await assert.rejects(verifyVisualBaselines({ browser, captures: [], ...dirs, provenance }), /matrix mismatch/);
  await assert.rejects(verifyVisualBaselines({ browser, captures: [capture], ...dirs, provenance: { ...provenance, browser: { name: 'Chromium', version: 'changed' } } }), /provenance does not match/);
  await assert.rejects(verifyVisualBaselines({ browser, captures: [{ ...capture, status: 'not-run' }], ...dirs, provenance }), /Visual baselines failed/);
  await fs.writeFile(path.join(dirs.evidenceDir, 'home.png'), 'not a png');
  await assert.rejects(verifyVisualBaselines({ browser, captures: [capture], ...dirs, provenance }), /Visual baselines failed/);
});

async function scenario(registry = { schemaVersion: 1, status: 'pending-review', sourceRevision: null, review: null, provenance: null, images: [] }) {
  const base = path.join(root, Math.random().toString(36).slice(2));
  const evidenceDir = path.join(base, 'evidence'), baselineDir = path.join(base, 'baselines');
  await fs.mkdir(evidenceDir, { recursive: true }); await fs.mkdir(baselineDir, { recursive: true });
  await fs.writeFile(path.join(baselineDir, 'approved.json'), JSON.stringify(registry));
  await writePng(evidenceDir, 'home.png', 20, 20, '#0000ff');
  return { evidenceDir, baselineDir };
}

async function makeApproved(dirs) {
  await writePng(dirs.baselineDir, 'home.png', 20, 20, '#0000ff');
  await fs.writeFile(path.join(dirs.baselineDir, 'approved.json'), JSON.stringify({ schemaVersion: 1, status: 'approved', sourceRevision: 'approved-revision', review: { owner: 'fixture', families: ['Home'] }, provenance, images: [{ id: capture.id, host: capture.host, viewport: capture.viewport, theme: capture.theme, file: 'home.png' }] }));
}

async function writePng(dir, file, width, height, color) {
  const page = await browser.newPage();
  try {
    const url = await page.evaluate(({ width, height, color }) => {
      const canvas = document.createElement('canvas'); canvas.width = width; canvas.height = height;
      const context = canvas.getContext('2d'); context.fillStyle = color; context.fillRect(0, 0, width, height);
      return canvas.toDataURL('image/png');
    }, { width, height, color });
    await fs.writeFile(path.join(dir, file), Buffer.from(url.split(',')[1], 'base64'));
  } finally { await page.close(); }
}

async function writeSinglePixel(dir, file) {
  const page = await browser.newPage();
  try {
    const url = await page.evaluate(() => {
      const canvas = document.createElement('canvas'); canvas.width = 20; canvas.height = 20;
      const context = canvas.getContext('2d'); context.fillStyle = '#0000ff'; context.fillRect(0, 0, 20, 20);
      context.fillStyle = '#ff0000'; context.fillRect(0, 0, 1, 1);
      return canvas.toDataURL('image/png');
    });
    await fs.writeFile(path.join(dir, file), Buffer.from(url.split(',')[1], 'base64'));
  } finally { await page.close(); }
}
