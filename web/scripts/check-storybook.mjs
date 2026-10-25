// Run on CI, never on the thermally constrained maintainer machine.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { verifyVisualBaselines } from './visual-baselines.mjs';
import { assertStorybookCoverage, assertRouteTreeContract, captureApplies, parseReservedNames } from './storybook-coverage.mjs';
import { cases, viewports } from '../storybook/catalogue.mjs';

const base = process.env.STORYBOOK_URL ?? 'http://127.0.0.1:6007';
const host = process.env.STORYBOOK_HOST ?? 'oss';
const output = process.env.STORYBOOK_EVIDENCE ?? 'storybook-evidence';
const baselineDir = process.env.STORYBOOK_BASELINES ?? 'storybook/baselines';
const repoRoot = fileURLToPath(new URL('../..', import.meta.url));
const viewerRevision = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: repoRoot, encoding: 'utf8' }).trim();
const testedRevision = process.env.GITHUB_SHA ?? null;
fs.mkdirSync(output, { recursive: true });
const indexResponse = await fetch(`${base}/index.json`);
if (!indexResponse.ok) throw new Error(`Storybook index.json returned ${indexResponse.status}`);
const reservedSource = fs.readFileSync(new URL('../src/routes/reserved.ts', import.meta.url), 'utf8');
const routeSource = fs.readFileSync(new URL('../src/router.tsx', import.meta.url), 'utf8');
assertStorybookCoverage(cases, await indexResponse.json(), parseReservedNames(reservedSource));
assertRouteTreeContract(routeSource);
const browser = await chromium.launch();
const browserVersion = browser.version();
const resultMap = new Map();
const applicable = (item, viewport) => captureApplies(item, viewport, host);
const expected = viewports.flatMap(viewport => cases.filter(item => applicable(item, viewport)).flatMap(item => ['light', 'dark'].map(theme => {
  const row = { id: item.id, host, viewport: viewport.name, theme, status: 'not-run', file: `${host}-${item.id}-${viewport.name}-${theme}.png` };
  resultMap.set(JSON.stringify([row.id, row.host, row.viewport, row.theme]), row);
  return row;
})));
const results = [...resultMap.values()];
const expectedCaptures = expected.length;
let baselineError;
try {
  // Fresh browser contexts isolate selection, theme, service worker and cache.
  for (const viewport of viewports) for (const theme of ['light', 'dark']) {
    const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, hasTouch: viewport.touch, colorScheme: theme, locale: 'en-US', timezoneId: 'UTC', reducedMotion: 'reduce', deviceScaleFactor: 1 });
    const page = await context.newPage();
    const errors = [], external = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('request', request => { if (new URL(request.url()).origin !== new URL(base).origin && /^https?:/.test(request.url())) external.push(request.url()); });
    await page.clock.setFixedTime(new Date('2026-10-19T08:00:00Z'));
    for (const item of cases.filter(item => applicable(item, viewport))) {
      const row = resultMap.get(JSON.stringify([item.id, host, viewport.name, theme]));
      try {
        const storyId = item.storybookId ?? `tolmap-pages--${item.id}`;
        await page.goto(`${base}/iframe.html?id=${storyId}&viewMode=story&globals=theme:${theme};host:${host}`);
        await page.locator(`html[data-visual-ready="${item.id}"]`).waitFor({ timeout: 30000 });
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `horizontal overflow: ${item.id}`);
        assert.equal(await page.locator('html').getAttribute('data-theme'), theme);
        if (item.id === 'map-overview') assert.ok(await page.locator('.map-svg path').count() > 0);
        if (item.id === 'map-freshness') {
          assert.ok(await page.locator('.map-svg path').count() > 0, 'freshness must use the real map');
          if (viewport.name === 'desktop') assert.equal(await page.locator('.desktop-panel').evaluate(panel => getComputedStyle(panel).opacity), '1', 'exercise the default open desktop panel');
          if (['phone', 'landscape'].includes(viewport.name)) assert.equal(await page.locator('[data-control-column]').count(), 1, 'exercise the real phone controls');
          assert.equal(await page.locator('[data-map-status-notes]').count(), 1, 'both notes must share one positioned column');
          const geometry = await page.locator('[data-map-status-notes]').evaluate(container => {
            const notes = [...container.querySelectorAll('[data-detail-status], [data-snapshot-freshness-note]')];
            const visible = element => {
              const box = element.getBoundingClientRect();
              return box.width > 0 && box.height > 0 && box.right > 0 && box.left < innerWidth
                && getComputedStyle(element).visibility !== 'hidden' && getComputedStyle(element).opacity !== '0';
            };
            const overlaps = (left, right) => {
              const a = left.getBoundingClientRect(), b = right.getBoundingClientRect();
              return a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
            };
            const noteGeometry = notes.map(note => {
              const rect = note.getBoundingClientRect();
              const center = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
              const actionHits = [...note.querySelectorAll('a, button')].map(action => {
                const box = action.getBoundingClientRect();
                const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
                return !!hit && action.contains(hit);
              });
              return {
                name: note.hasAttribute('data-detail-status') ? 'detail' : 'freshness',
                inFlow: getComputedStyle(note).position === 'static',
                inViewport: rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight,
                fits: note.scrollWidth <= note.clientWidth && note.scrollHeight <= note.clientHeight,
                centerHit: !!center && note.contains(center),
                actionHits,
              };
            });
            const noteOverlaps = notes.flatMap((note, index) => notes.slice(index + 1)
              .filter(other => overlaps(note, other)).map(other => `${note.hasAttribute('data-detail-status') ? 'detail' : 'freshness'} vs ${other.hasAttribute('data-detail-status') ? 'detail' : 'freshness'}`));
            const controls = [...document.querySelectorAll('.desktop-panel, [data-panel-tab], [data-desktop-actions], [data-command-bar], [data-map-controls], [data-control-column]')].filter(visible);
            const controlOverlaps = notes.flatMap(note => controls.filter(control => overlaps(note, control))
              .map(control => `${note.hasAttribute('data-detail-status') ? 'detail' : 'freshness'} vs ${control.getAttribute('data-panel-tab') !== null ? 'panel tab' : control.className || control.tagName}`));
            const freshness = container.querySelector('[data-snapshot-freshness-note]');
            const retry = freshness?.querySelector('a');
            const retryRect = retry?.getBoundingClientRect();
            const retryHit = retryRect && document.elementFromPoint(retryRect.left + retryRect.width / 2, retryRect.top + retryRect.height / 2);
            return {
              noteCount: notes.length,
              notes: noteGeometry,
              noteOverlaps,
              controlOverlaps,
              retryExists: !!retry,
              retryHit: !!retryHit && !!retry && retry.contains(retryHit),
            };
          });
          assert.equal(geometry.noteCount, 2, 'the detail and freshness notes must both render');
          for (const note of geometry.notes) {
            assert.equal(note.inFlow, true, `${note.name} note must use the shared column position`);
            assert.equal(note.inViewport, true, `${note.name} note must stay in the viewport`);
            assert.equal(note.fits, true, `${note.name} text must wrap without clipping`);
            assert.equal(note.centerHit, true, `${note.name} note must receive pointer hits`);
            assert.equal(note.actionHits.every(Boolean), true, `${note.name} actions must receive pointer hits`);
          }
          assert.deepEqual(geometry.noteOverlaps, [], 'detail and freshness notes must not overlap');
          assert.deepEqual(geometry.controlOverlaps, [], 'both notes must clear the panel and real map controls');
          assert.equal(geometry.retryExists, true, 'failed freshness must show its retry action');
          assert.equal(geometry.retryHit, true, 'retry indexing must be clickable beside the detail note');
        }
        if (item.id === 'source-ready') assert.equal(await page.locator('.cm-content').getAttribute('aria-readonly'), 'true');
        await page.screenshot({ path: path.join(output, row.file), animations: 'disabled' });
        assert.deepEqual(errors, [], `browser errors while rendering ${item.id}`);
        assert.deepEqual(external, [], `external requests while rendering ${item.id}`);
        row.result = 'passed'; row.status = 'passed';
      } catch (error) {
        row.result = 'failed'; row.status = 'failed'; row.error = error.message;
        await page.screenshot({ path: path.join(output, row.file), animations: 'disabled' });
      }
      console.log(`${row.result}: ${host}/${item.id}/${viewport.name}/${theme}`);
      if (row.error) console.error(row.error);
      if (row.result === 'failed') throw new Error(`Capture stopped at ${row.id}; inspect manifest.json before continuing the required matrix`);
    }
    assert.deepEqual(errors, [], 'browser errors');
    assert.deepEqual(external, [], 'external network requests');
    await context.close();
  }
} finally {
  try {
    const fixture = '../web/check-fixtures/onsager-ai__tolmap.json.gz';
    const fixtureSha256 = await hashFile(path.resolve(fixture));
    const provenance = {
      browser: { name: 'Chromium', version: browserVersion },
      environment: {
        locale: 'en-US', timezone: 'UTC', scale: 1, reducedMotion: 'reduce', fixedClock: '2026-10-19T08:00:00Z',
        viewports: viewports.map(({ name, width, height, touch }) => ({ name, width, height, hasTouch: touch })),
      },
      fixture: { path: 'web/check-fixtures/onsager-ai__tolmap.json.gz', sha256: fixtureSha256 },
    };
    await verifyVisualBaselines({ browser, captures: results, evidenceDir: output, baselineDir, provenance, sourceRevision: viewerRevision });
  } catch (error) { baselineError = error; }
  await browser.close();
  const evidence = { status: 'candidate-unapproved', viewerRevision, testedRevision, browser: { name: 'Chromium', version: browserVersion }, locale: 'en-US', timezone: 'UTC', scale: 1, fixedClock: '2026-10-19T08:00:00Z', viewports: viewports.map(({ name, width, height, touch }) => ({ name, width, height, hasTouch: touch })), fixture: 'web/check-fixtures/onsager-ai__tolmap.json.gz', expectedCaptures, completedCaptures: results.filter(row => row.status === 'passed').length, captureComplete: results.length === expectedCaptures && results.every(row => row.status === 'passed'), results };
  fs.writeFileSync(path.join(output, 'manifest.json'), JSON.stringify(evidence, null, 2));
  const escape = value => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;');
  fs.writeFileSync(path.join(output, 'index.html'), `<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content="width=device-width, initial-scale=1"><title>Tolmap visual candidates</title><style>body{font:15px/1.5 system-ui;margin:32px;background:#f4f5f3;color:#202622}main{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:20px}figure{margin:0;background:white;padding:12px;border-radius:12px}img{width:100%;height:240px;object-fit:contain;background:#eee}a{color:inherit}input{padding:12px;width:min(80%,500px);margin-bottom:24px}</style><h1>Tolmap · visual candidates</h1><p>Unapproved reference candidates · viewer ${escape(evidence.viewerRevision)} · tested ${escape(evidence.testedRevision)} · ${host} · <a href=manifest.json>manifest</a></p><input placeholder="Filter page, viewport or theme" aria-label="Filter candidates" oninput="document.querySelectorAll('figure').forEach(f=>f.hidden=!f.textContent.includes(this.value))"><main>${results.map(row => `<figure><a href="${escape(row.file)}"><img loading=lazy src="${escape(row.file)}" alt="${escape(row.id)}"></a><figcaption>${escape(row.id)} · ${row.viewport} · ${row.theme} · ${row.result ?? row.status}</figcaption></figure>`).join('')}</main></html>`);
}
if (baselineError) throw baselineError;
assert.equal(results.filter(row => row.result === 'failed').length, 0, 'Storybook candidates contain failures; inspect manifest.json');
assert.ok(results.length > 0, 'No stories captured');

function hashFile(file) {
  return new Promise((resolve, reject) => {
    const hash = createHash('sha256');
    const stream = createReadStream(file);
    stream.on('data', chunk => hash.update(chunk));
    stream.on('error', reject);
    stream.on('end', () => resolve(hash.digest('hex')));
  });
}
