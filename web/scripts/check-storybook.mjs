// Run on CI, never on the thermally constrained maintainer machine.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { cases, viewports } from '../storybook/catalogue.mjs';

const base = process.env.STORYBOOK_URL ?? 'http://127.0.0.1:6007';
const host = process.env.STORYBOOK_HOST ?? 'oss';
const output = process.env.STORYBOOK_EVIDENCE ?? 'storybook-evidence';
fs.mkdirSync(output, { recursive: true });
const browser = await chromium.launch();
const results = [];
try {
  // Fresh browser contexts isolate selection, theme, service worker and cache.
  for (const viewport of viewports) for (const theme of ['light', 'dark']) {
    const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, hasTouch: viewport.touch, colorScheme: theme, locale: 'en-US', timezoneId: 'UTC', reducedMotion: 'reduce', deviceScaleFactor: 1 });
    const page = await context.newPage();
    const errors = [], external = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('request', request => { if (new URL(request.url()).origin !== new URL(base).origin && /^https?:/.test(request.url())) external.push(request.url()); });
    await page.clock.setFixedTime(new Date('2026-10-19T08:00:00Z'));
    for (const item of cases.filter(item => host !== 'hub' || (!item.id.startsWith('reserved-') && !item.id.startsWith('local-')))) {
      // Keyboard shortcuts are desktop/tablet UI; phone has no such control.
      if (item.id === 'map-keyboard' && ['phone', 'landscape'].includes(viewport.name)) continue;
      const row = { id: item.id, host, viewport: viewport.name, theme, status: 'candidate', file: `${host}-${item.id}-${viewport.name}-${theme}.png` };
      try {
        await page.goto(`${base}/iframe.html?id=tolmap-pages--${item.id}&viewMode=story&globals=theme:${theme};host:${host}`);
        await page.locator(`html[data-visual-ready="${item.id}"]`).waitFor({ timeout: 30000 });
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `horizontal overflow: ${item.id}`);
        assert.equal(await page.locator('html').getAttribute('data-theme'), theme);
        if (item.id === 'map-overview') assert.ok(await page.locator('.map-svg path').count() > 0);
        if (item.id === 'source-ready') assert.equal(await page.locator('.cm-content').getAttribute('aria-readonly'), 'true');
        await page.screenshot({ path: path.join(output, row.file), animations: 'disabled' });
        row.result = 'passed';
      } catch (error) {
        row.result = 'failed'; row.error = error.message;
        await page.screenshot({ path: path.join(output, row.file), animations: 'disabled' });
      }
      results.push(row);
      console.log(`${row.result}: ${host}/${item.id}/${viewport.name}/${theme}`);
      if (row.result === 'failed') throw new Error(`Capture stopped at ${row.id}; inspect manifest.json before continuing the required matrix`);
    }
    assert.deepEqual(errors, [], 'browser errors');
    assert.deepEqual(external, [], 'external network requests');
    await context.close();
  }
} finally {
  await browser.close();
  const evidence = { status: 'candidate-unapproved', sha: process.env.GITHUB_SHA ?? null, browser: 'Chromium', locale: 'en-US', timezone: 'UTC', scale: 1, fixture: 'web/check-fixtures/onsager-ai__tolmap.json.gz', results };
  fs.writeFileSync(path.join(output, 'manifest.json'), JSON.stringify(evidence, null, 2));
  const escape = value => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;');
  fs.writeFileSync(path.join(output, 'index.html'), `<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content="width=device-width, initial-scale=1"><title>Tolmap visual candidates</title><style>body{font:15px/1.5 system-ui;margin:32px;background:#f4f5f3;color:#202622}main{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:20px}figure{margin:0;background:white;padding:12px;border-radius:12px}img{width:100%;height:240px;object-fit:contain;background:#eee}a{color:inherit}input{padding:12px;width:min(80%,500px);margin-bottom:24px}</style><h1>Tolmap · visual candidates</h1><p>Unapproved reference candidates · ${escape(evidence.sha)} · ${host} · <a href=manifest.json>manifest</a></p><input placeholder="Filter page, viewport or theme" aria-label="Filter candidates" oninput="document.querySelectorAll('figure').forEach(f=>f.hidden=!f.textContent.includes(this.value))"><main>${results.map(row => `<figure><a href="${escape(row.file)}"><img loading=lazy src="${escape(row.file)}" alt="${escape(row.id)}"></a><figcaption>${escape(row.id)} · ${row.viewport} · ${row.theme} · ${row.result}</figcaption></figure>`).join('')}</main></html>`);
}
assert.equal(results.filter(row => row.result === 'failed').length, 0, 'Storybook candidates contain failures; inspect manifest.json');
assert.ok(results.length > 0, 'No stories captured');
