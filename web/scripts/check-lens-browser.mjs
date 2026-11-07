// CI only. Rust resolves data/lenses/flask-request.toml against the exact
// committed Flask map, with symbols extracted from its pinned source commit.
import { chromium } from 'playwright';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { cameraBoxes, settleCamera, zoomInSettled } from './camera-settle.mjs';
const base = process.argv[2] ?? 'http://127.0.0.1:5176';
const origin = new URL(base).origin;
const commit = 'd73fa1cdcbd8b1465c151db8924ba58b1dd14e35';
const map = JSON.parse(await readFile('lens-fixtures/flask.json', 'utf8'));
const loaded = JSON.parse(await readFile('lens-fixtures/loaded.json', 'utf8'));
const unresolved = JSON.parse(await readFile('lens-fixtures/unresolved.json', 'utf8'));
assert.deepEqual(await readFile('lens-fixtures/flask.json'), await readFile('../data/flask.json'), 'fixture map bytes stay unchanged');
assert.equal(loaded.unresolved.length, 0);
const screenshots = [];
await mkdir('shots/lenses', {recursive:true});
const browser = await chromium.launch({headless:true});
try {
  for (const profile of ['desktop', 'phone']) for (const theme of ['light', 'dark']) {
    const context = await browser.newContext({ viewport: profile === 'phone' ? {width:390,height:844} : {width:1440,height:900}, hasTouch:profile === 'phone', isMobile:profile === 'phone', colorScheme:theme, reducedMotion:'reduce' });
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/api/**', async route => {
      const url = new URL(route.request().url());
      assert.equal(url.origin, origin);
      const path = url.pathname;
      let json;
      if (path === '/api/healthz') json = {status:'ok'};
      else if (path === '/api/maps') json = [];
      else if (path === '/api/maps/pallets/flask') return route.fulfill({json:map, headers:{'x-tolmap-commit':commit}});
      else if (path.endsWith('/symbols')) json = JSON.parse(await readFile(`lens-fixtures/flask.symbols/${Number(url.searchParams.get('district'))}.json`, 'utf8'));
      else if (path.endsWith('/freshness')) return route.fulfill({status:404,json:{error:'not_found'}});
      else if (path.endsWith('/lenses')) { assert.equal(url.searchParams.get('commit'),commit); json = {repository:'pallets/flask',commit,names:['flask-request','unresolved']}; }
      else if (path.includes('/lenses/')) { assert.equal(url.searchParams.get('commit'),commit); const name = path.split('/').pop(); json = {repository:'pallets/flask',commit,name,report:name === 'unresolved' ? unresolved : loaded}; }
      else if (path.endsWith('/provenance')) json = {version:1,repository:'pallets/flask',commit};
      else if (path.endsWith('/source')) { assert.equal(url.searchParams.get('commit'),commit); json = {version:1,repository:'pallets/flask',commit,path:url.searchParams.get('path'),status:'ready',text:'# Synthetic source response for viewer interaction qualification.\nclass Flask:\n    pass\n',bytes:84,long_line:false}; }
      else return route.fulfill({status:404,json:{error:'not_found'}});
      await route.fulfill({json});
    });
    const url = `${base}/pallets/flask?commit=${commit}`;
    const shot = async state => {
      await page.evaluate(() => document.fonts.ready);
      await settleCamera(page);
      const file = `${profile}-${theme}-${state}.png`;
      await page.screenshot({path:`shots/lenses/${file}`,animations:'disabled'});
      screenshots.push({profile,theme,state,file,commit,lens:'data/lenses/flask-request.toml'});
    };
    const panel = page.locator('[data-lens-panel]');
    const openPanel = async () => {
      if (profile === 'phone') await page.getByRole('button',{name:'Map layers',exact:true}).click();
      if (!await panel.evaluate(element => element.open)) await panel.locator(':scope > summary').click();
    };
    const closePanel = async () => {
      if (profile === 'phone') await page.getByRole('button',{name:'Close layers',exact:true}).click();
    };
    await page.goto(url);
    await page.locator('.map-svg path.hit[data-k^="d:"]').first().waitFor();
    await settleCamera(page);
    const before = await cameraBoxes(page);
    await openPanel();
    await panel.getByRole('combobox',{name:'Saved lens'}).selectOption('flask-request');
    await page.locator('[data-lens-state="loaded"]').waitFor();
    await closePanel();
    assert.deepEqual(await cameraBoxes(page), before, 'activating a lens never moves the camera');
    assert.equal(new URL(page.url()).searchParams.get('lens'),'flask-request');
    assert.equal(await page.locator('[data-lens-stop]').count(),0,'overview hides all file and symbol stops');
    assert.equal(await page.locator('[data-lens-area]').count()>0,true);
    assert.equal(await page.locator('[data-lens-hop="declared"]').count(),1);
    assert.equal(await page.locator('[data-lens-hop="declared"]').getAttribute('stroke-dasharray'),'7 5');
    await shot('overview');
    const declared = page.locator('[data-lens-hop-hit="declared"]');
    // SVG path's bbox centre need not lie on its stroke. Aim at the path's
    // midpoint using its own geometry, so this exercises actual hit testing.
    const hit = await declared.evaluate(element => { const point = element.getPointAtLength(element.getTotalLength()/2); const matrix = element.getScreenCTM(); const p = new DOMPoint(point.x,point.y).matrixTransform(matrix); return {x:p.x,y:p.y}; });
    await page.mouse.click(hit.x,hit.y);
    if (profile === 'phone') {
      await page.locator('[data-structure-card="lens"]').waitFor();
      assert.match(await page.locator('[data-structure-summary]').textContent(),/render_template reads current_app/);
    } else {
      await page.locator('.tolmap-hover-card').filter({hasText:'Declared hop'}).waitFor();
    }
    await shot('declared-hop');
    // Deep link restores the lens independently of history overlay markers.
    await page.goto(`${url}&lens=flask-request`);
    await page.locator('[data-lens-overlay]').waitFor();
    // Zoom in deliberately until file stops qualify. A lens never performs
    // this camera operation itself. Every step waits for the production glide.
    for (let i=0; i<10 && await page.locator('[data-lens-stop]').count() === 0; i++) await zoomInSettled(page,1);
    assert.ok(await page.locator('[data-lens-stop]').count()>0,'zoomed-in stops become visible');
    await shot('zoomed-stops');
    const stop = page.locator('[data-lens-stop]').first();
    const spec = await stop.getAttribute('data-lens-stop');
    // May be outside the viewport at a particular camera centre. The ordered
    // list uses the same stop action and pans without changing zoom.
    await openPanel();
    await panel.getByRole('button',{name:spec,exact:true}).last().click();
    if (profile === 'phone') await page.locator('[data-phone-sheet] [data-sheet-card="file"], [data-phone-sheet] [data-sheet-card="symbol"]').waitFor();
    else await page.locator('[data-selection-panel]').waitFor();
    assert.equal(new URL(page.url()).searchParams.get('file'),spec.split('#')[0]);
    await page.getByRole('button',{name:'Read selected source',exact:true}).click();
    await page.locator('[data-source-reader] .cm-content').waitFor();
    assert.equal(new URL(page.url()).searchParams.get('lens'),'flask-request');
    await page.goBack();
    await page.locator('[data-source-reader]').waitFor({state:'detached'});
    // Resolver failures remain visible alongside the surviving resolved map.
    await page.goto(`${url}&lens=unresolved`);
    await page.locator('[data-lens-overlay]').waitFor();
    await openPanel();
    await page.locator('[data-lens-unresolved]').scrollIntoViewIfNeeded();
    assert.match(await panel.textContent(),/missing_path/);
    assert.match(await panel.textContent(),/Unresolved stop/);
    assert.match(await panel.textContent(),/Unresolved hop/);
    await shot('unresolved');
    assert.deepEqual(errors,[],`${profile}/${theme} browser errors`);
    await context.close();
  }
} finally {
  await writeFile('shots/lenses/manifest.json',JSON.stringify({commit,fixture:'data/flask.json',lens:'data/lenses/flask-request.toml',screenshots},null,2));
  await browser.close();
}
