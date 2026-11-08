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
const comparison = JSON.parse(await readFile('lens-fixtures/comparison.json', 'utf8'));
const overlap = JSON.parse(await readFile('lens-fixtures/overlap.json', 'utf8'));
const screenshots = [];
let activePage;
await mkdir('shots/lenses', {recursive:true});
const browser = await chromium.launch({headless:true});
try {
  for (const profile of ['desktop', 'phone']) for (const theme of ['light', 'dark']) {
    const context = await browser.newContext({ viewport: profile === 'phone' ? {width:390,height:844} : {width:1440,height:900}, hasTouch:profile === 'phone', isMobile:profile === 'phone', colorScheme:theme, reducedMotion:'reduce' });
    const page = await context.newPage();
    activePage = page;
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/api/**', async route => {
      const url = new URL(route.request().url());
      if (url.origin !== origin || !url.pathname.startsWith('/api/')) return route.continue();
      const path = url.pathname;
      let json;
      if (path === '/api/healthz') json = {status:'ok'};
      else if (path === '/api/maps') json = [];
      else if (path === '/api/maps/pallets/flask') return route.fulfill({json:map, headers:{'x-tolmap-commit':commit}});
      else if (path.endsWith('/symbols')) json = JSON.parse(await readFile(`lens-fixtures/flask.symbols/${Number(url.searchParams.get('district'))}.json`, 'utf8'));
      else if (path.endsWith('/freshness')) return route.fulfill({status:404,json:{error:'not_found'}});
      else if (path.startsWith('/api/compare/') && path.endsWith('/lenses')) { json = url.searchParams.has('other') ? overlap : comparison; }
      else if (path.endsWith('/lenses')) { assert.equal(url.searchParams.get('commit'),commit); json = {repository:'pallets/flask',commit,names:['flask-request','overlap','unresolved']}; }
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
    const panel = page.locator(profile === 'phone' ? '[data-lens-sheet]' : '[data-lens-panel]');
    const openPanel = async () => {
      if (profile === 'phone') {
        // The active report belongs to the normal detented sheet; Full is
        // only needed to scroll to a particular stop or unresolved item.
        for (let i=0; i<3 && await page.locator('[data-phone-sheet]').getAttribute('data-detent') !== 'full'; i++) await page.locator('[data-sheet-grabber]').click();
      } else if (!await panel.evaluate(element => element.open)) await panel.locator(':scope > summary').click();
    };
    await page.goto(url);
    await page.locator('.map-svg path.hit[data-k^="d:"]').first().waitFor();
    await settleCamera(page);
    const before = await cameraBoxes(page);
    console.log(`${profile}/${theme}: map ready`);
    if (profile === 'phone') await page.getByRole('button',{name:'Map layers',exact:true}).click();
    const picker = page.locator(profile === 'phone' ? '[data-layers-sheet] [data-lens-panel]' : '[data-lens-panel]');
    if (!await picker.evaluate(element => element.open)) await picker.locator(':scope > summary').click();
    await picker.getByRole('combobox',{name:'Saved lens'}).selectOption('flask-request');
    await panel.locator('[data-lens-state="loaded"]').waitFor();
    assert.doesNotMatch(await panel.textContent(), /Computed · computed/);
    if (profile === 'phone') {
      await page.locator('[data-layers-sheet]').waitFor({state:'detached'});
      assert.equal(await page.locator('[data-phone-sheet]').getAttribute('data-detent'),'peek');
      assert.ok((await page.locator('[data-phone-sheet]').boundingBox()).y > 500,'lens leaves the map visible');
    }
    assert.deepEqual(await cameraBoxes(page), before, 'activating a lens never moves the camera');
    assert.equal(new URL(page.url()).searchParams.get('lens'),'flask-request');
    assert.equal(await page.locator('[data-lens-stop]').count(),0,'overview hides all file and symbol stops');
    assert.equal(await page.locator('[data-lens-note]').count(),0,'overview hides anchored note markers');
    if (profile === 'phone') {
      await panel.getByRole('button',{name:'Details',exact:true}).click();
      assert.equal(await page.locator('[data-phone-sheet]').getAttribute('data-detent'),'half');
      await page.goBack();
      await page.waitForFunction(() => document.querySelector('[data-phone-sheet]')?.getAttribute('data-detent') === 'peek');
      await page.getByRole('button',{name:'Map layers',exact:true}).click();
      await page.goBack();
      await page.locator('[data-layers-sheet]').waitFor({state:'detached'});
      assert.equal(new URL(page.url()).searchParams.get('lens'),'flask-request','Back closes Layers while retaining the lens');
      assert.deepEqual(await cameraBoxes(page),before,'Back from Layers leaves the camera fixed');
    }
    assert.equal(await page.locator('[data-lens-area]').count()>0,true);
    assert.equal(await page.locator('[data-lens-hop="declared"]').count(),1);
    assert.equal(await page.locator('[data-lens-hop="declared"]').getAttribute('stroke-dasharray'),'7 5');
    assert.equal(await page.locator('[data-lens-hop="computed"]').evaluateAll(paths => paths.every(path => /[Qc]/.test(path.getAttribute('d')))),true,'computed hops bend');
    assert.equal(await page.locator('[data-lens-overlay]').evaluate(overlay => {
      const label = [...overlay.parentElement.querySelectorAll('text[data-k^="d:"]')].find(text => text.textContent.trim());
      return !!label && !!(overlay.compareDocumentPosition(label) & Node.DOCUMENT_POSITION_FOLLOWING);
    }),true,'district labels draw above lens routes');
    await shot('overview');
    console.log(`${profile}/${theme}: overview captured`);
    const declared = page.locator('[data-lens-hop-hit="declared"]');
    // SVG path's bbox centre need not lie on its stroke. Aim at the path's
    // midpoint using its own geometry, so this exercises actual hit testing.
    const hit = await declared.evaluate(element => {
      const matrix = element.getScreenCTM();
      for (let i=1; i<40; i++) {
        const point = element.getPointAtLength(element.getTotalLength()*i/40);
        const p = new DOMPoint(point.x,point.y).matrixTransform(matrix);
        if (document.elementFromPoint(p.x,p.y)?.closest('[data-k]') === element) return {x:p.x,y:p.y};
      }
      throw new Error('Declared hop has no accessible point outside chrome');
    });
    await page.mouse.click(hit.x,hit.y);
    if (profile === 'phone') {
      await page.locator('[data-structure-card="lens"]').waitFor();
      assert.match(await page.locator('[data-structure-summary]').textContent(),/render_template reads current_app/);
    } else {
      const tooltip = page.locator('.tolmap-hover-card').filter({hasText:'Declared hop'});
      await tooltip.waitFor();
      const tooltipBox = await tooltip.boundingBox(), inspectorBox = await page.locator('[data-desktop-panel]').boundingBox();
      assert.ok(tooltipBox.x >= inspectorBox.x + inspectorBox.width + 7,'hop reason stays outside inspector');
      assert.ok(tooltipBox.x + tooltipBox.width <= 1440 && tooltipBox.y >= 0 && tooltipBox.y + tooltipBox.height <= 900,'hop tooltip stays in map');
    }
    await shot('declared-hop');
    console.log(`${profile}/${theme}: declared hop captured`);
    // Deep link restores the lens independently of history overlay markers.
    await page.goto(`${url}&lens=flask-request`);
    await page.locator('[data-lens-overlay]').waitFor();
    // Zoom in deliberately until file stops qualify. A lens never performs
    // this camera operation itself. Every step waits for the production glide.
    for (let i=0; i<10 && await page.locator('[data-lens-stop]').count() === 0; i++) await zoomInSettled(page,1);
    assert.ok(await page.locator('[data-lens-stop]').count()>0,'zoomed-in stops become visible');
    const stop = page.locator('[data-lens-stop]').first();
    const spec = await stop.getAttribute('data-lens-stop');
    // May be outside the viewport at a particular camera centre. The ordered
    // list uses the same stop action and pans without changing zoom.
    await openPanel();
    await panel.getByRole('button',{name:spec,exact:true}).last().click();
    if (profile === 'phone') await page.locator('[data-phone-sheet] [data-sheet-card="file"], [data-phone-sheet] [data-sheet-card="symbol"]').waitFor();
    else await page.locator('[data-selection-panel]').waitFor();
    assert.equal(new URL(page.url()).searchParams.get('file'),spec.split('#')[0]);
    await settleCamera(page);
    const mapStop = page.locator(`[data-lens-stop="${spec}"]`);
    await mapStop.click();
    assert.equal(new URL(page.url()).searchParams.get('file'),spec.split('#')[0], 'a real map stop tap opens its card');
    if (spec.includes('#')) assert.ok(new URL(page.url()).searchParams.has('sym') || new URL(page.url()).searchParams.has('hsym'), 'symbol stop opens a symbol card');
    await shot('zoomed-stops');
    console.log(`${profile}/${theme}: visible stop and card captured`);
    await page.getByRole('button',{name:'Read selected source',exact:true}).click();
    await page.locator('[data-source-reader] .cm-content').waitFor();
    assert.equal(new URL(page.url()).searchParams.get('lens'),'flask-request');
    await page.goBack();
    await page.locator('[data-source-reader]').waitFor({state:'detached'});
    console.log(`${profile}/${theme}: committed reader back passed`);
    // Resolver failures remain visible alongside the surviving resolved map.
    await page.goto(`${url}&lens=unresolved`);
    await page.locator('[data-lens-overlay]').waitFor();
    await openPanel();
    await page.locator('[data-lens-unresolved]').scrollIntoViewIfNeeded();
    assert.match(await panel.textContent(),/missing_path/);
    assert.match(await panel.textContent(),/Unresolved stop/);
    assert.match(await panel.textContent(),/Unresolved hop/);
    await shot('unresolved');
    // Comparison scenarios are Rust-generated controlled edits of the pinned
    // Flask map, with the same actual symbol geometry. No head layout exists.
    await page.goto(`${url}&lens=flask-request`);
    await panel.locator('[data-lens-state="loaded"]').waitFor();
    await openPanel();
    await panel.getByRole('button',{name:'Compare this lens',exact:true}).click();
    const comparePanel = page.locator('[data-lens-comparison]');
    await comparePanel.getByRole('textbox',{name:'Lens head commit',exact:true}).fill(comparison.head);
    await comparePanel.getByRole('button',{name:'Compare lenses',exact:true}).click();
    await comparePanel.locator('[data-lens-comparison-state="loaded"]').waitFor();
    assert.equal(await page.locator('[data-lens-comparison-stop]').count(),0,'comparison overview hides stops');
    for (const status of ['added','removed','broken','renamed']) assert.ok(await comparePanel.locator(`[data-lens-delta="${status}"]`).count()>0,`comparison reports ${status}`);
    assert.ok(await page.locator('[data-lens-change="broken"]').count()>0,'broken hop retains base projection');
    await shot('comparison-commits');
    for (let i=0; i<10 && await page.locator('[data-lens-comparison-stop]').count()===0; i++) await zoomInSettled(page,1);
    assert.ok(await page.locator('[data-lens-comparison-stop]').count()>0,'comparison stops follow normal zoom gates');
    await shot('comparison-zoomed');
    await comparePanel.getByRole('combobox',{name:'Compare with lens'}).selectOption('overlap');
    await comparePanel.getByRole('button',{name:'Compare lenses',exact:true}).click();
    await comparePanel.locator('[data-lens-comparison-state="loaded"]').waitFor();
    assert.ok(await comparePanel.locator('[data-lens-delta="shared"]').count()>0);
    await shot('comparison-overlap');
    await page.reload();
    await comparePanel.locator('[data-lens-comparison-state="loaded"]').waitFor();
    assert.equal(new URL(page.url()).searchParams.get('other_lens'),'overlap','comparison survives deep link reload');
    // A stale response must be explicit and must remove all comparison paint.
    await page.route('**/api/compare/**/lenses?*', route => route.fulfill({json:{...overlap,base:'3'.repeat(40)}}));
    await page.reload();
    await comparePanel.locator('[data-lens-comparison-state="error"]').waitFor();
    assert.equal(await page.locator('[data-lens-comparison-overlay]').count(),0);
    await shot('comparison-stale');
    assert.deepEqual(errors,[],`${profile}/${theme} browser errors`);
    await context.close();
  }
} catch (error) {
  await activePage?.screenshot({path:'shots/lenses/failure.png',animations:'disabled'});
  await writeFile('shots/lenses/failure.txt', `${error.stack}\nURL: ${activePage?.url()}\n${await activePage?.locator('body').innerText()}`);
  throw error;
} finally {
  await writeFile('shots/lenses/manifest.json',JSON.stringify({commit,fixture:'data/flask.json',lens:'data/lenses/flask-request.toml',screenshots},null,2));
  await browser.close();
}
