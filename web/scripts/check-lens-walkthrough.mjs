// CI only (issue #248). Steps data/lenses/flask-request.toml, resolved by Rust
// against the exact committed Flask map, on desktop and phone in both themes.
import { chromium } from 'playwright';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { cameraBoxes, settleCamera } from './camera-settle.mjs';
const base = process.argv[2] ?? 'http://127.0.0.1:5176';
const origin = new URL(base).origin;
const commit = 'd73fa1cdcbd8b1465c151db8924ba58b1dd14e35';
const mapBytes = await readFile('lens-fixtures/flask.json');
assert.deepEqual(mapBytes, await readFile('../data/flask.json'), 'fixture map bytes stay unchanged');
const loaded = JSON.parse(await readFile('lens-fixtures/loaded.json', 'utf8'));
const unresolved = JSON.parse(await readFile('lens-fixtures/unresolved.json', 'utf8'));
const path = loaded.paths.find(item => item.name === 'request-to-template');
assert.ok(path && path.stops.every(stop => stop.target), 'the flask path resolves every stop');
const broken = unresolved.paths.find(item => item.name === 'missing-stop');
assert.ok(broken.stops[0].target && !broken.stops[1].target, 'the unresolved sample has a resolved then a missing stop');
const screenshots = [];
let activePage;
await mkdir('shots/lens-walkthrough', {recursive:true});
const browser = await chromium.launch({headless:true});
try {
  for (const profile of ['desktop', 'phone']) for (const theme of ['light', 'dark']) {
    const phone = profile === 'phone';
    const context = await browser.newContext({ viewport: phone ? {width:390,height:844} : {width:1440,height:900}, hasTouch:phone, isMobile:phone, colorScheme:theme, reducedMotion:'reduce' });
    const page = await context.newPage();
    activePage = page;
    const errors = [];
    const mapResponses = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('request', request => {
      const url = new URL(request.url());
      if (url.pathname.startsWith('/api/maps/pallets/flask') && !url.pathname.includes('/lenses') && request.method() !== 'GET') errors.push(`map write: ${request.method()} ${url.pathname}`);
    });
    await page.route('**/api/**', async route => {
      const url = new URL(route.request().url());
      if (url.origin !== origin || !url.pathname.startsWith('/api/')) return route.continue();
      const p = url.pathname;
      let json;
      if (p === '/api/healthz') json = {status:'ok'};
      else if (p === '/api/maps') json = [];
      else if (p === '/api/maps/pallets/flask') { mapResponses.push(p); return route.fulfill({body:mapBytes, contentType:'application/json', headers:{'x-tolmap-commit':commit}}); }
      else if (p.endsWith('/symbols')) json = JSON.parse(await readFile(`lens-fixtures/flask.symbols/${Number(url.searchParams.get('district'))}.json`, 'utf8'));
      else if (p.endsWith('/freshness')) return route.fulfill({status:404,json:{error:'not_found'}});
      else if (p.endsWith('/lenses')) json = {repository:'pallets/flask',commit,names:['flask-request','unresolved']};
      else if (p.includes('/lenses/')) { const name = p.split('/').pop(); json = {repository:'pallets/flask',commit,name,report:name === 'unresolved' ? unresolved : loaded}; }
      else if (p.endsWith('/provenance')) json = {version:1,repository:'pallets/flask',commit};
      else if (p.endsWith('/source')) json = {version:1,repository:'pallets/flask',commit,path:url.searchParams.get('path'),status:'ready',text:'# Synthetic source response for viewer interaction qualification.\nclass Flask:\n    pass\n',bytes:84,long_line:false};
      else return route.fulfill({status:404,json:{error:'not_found'}});
      await route.fulfill({json});
    });
    const url = `${base}/pallets/flask?commit=${commit}`;
    const params = () => new URL(page.url()).searchParams;
    const walk = page.locator('[data-lens-walkthrough]');
    const shot = async state => {
      if (phone) assert.equal(await page.locator('[data-phone-shell]').evaluate(el => el.scrollTop), 0, 'walkthrough never scrolls the phone shell');
      await page.evaluate(() => document.fonts.ready);
      await settleCamera(page);
      const file = `${profile}-${theme}-${state}.png`;
      await page.screenshot({path:`shots/lens-walkthrough/${file}`,animations:'disabled'});
      screenshots.push({profile,theme,state,file,commit,lens:'data/lenses/flask-request.toml'});
    };
    // The stop marker is drawn only above its gates (lensTargetVisible), so
    // a visible marker inside the map's free area is the framing evidence.
    const assertFramed = async (spec, step) => {
      await page.locator(`[data-lens-walkthrough][data-lens-walk-step="${step}"]`).waitFor();
      await settleCamera(page);
      const marker = page.locator(`[data-lens-stop="${spec}"]`).first();
      await marker.waitFor();
      const box = await marker.boundingBox();
      const cover = await page.locator(phone ? '[data-phone-sheet]' : '[data-desktop-panel]').boundingBox();
      const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
      if (phone) assert.ok(cy > 0 && cy < cover.y, `${spec} is framed above the sheet (${cy} vs ${cover.y})`);
      else assert.ok(cx > cover.x + cover.width && cx < 1440 && cy > 0 && cy < 900, `${spec} is framed beside the inspector (${cx}, ${cy})`);
      assert.equal(params().get('step'), String(step));
      assert.equal(params().get('file'), spec.split('#')[0], 'the step opens the stop card');
      await page.locator('[data-sheet-card="file"], [data-sheet-card="symbol"]').first().waitFor();
      if (spec.includes('#')) assert.ok(params().has('sym') || params().has('hsym'), 'a symbol stop opens its symbol card');
    };

    await page.goto(`${url}&lens=flask-request`);
    await page.locator('[data-lens-overlay]').waitFor();
    await settleCamera(page);
    const fitted = await cameraBoxes(page);
    const history = await page.evaluate(() => history.length);
    assert.equal(await page.locator('[data-lens-stop]').count(), 0, 'lens selection alone keeps the overview');
    if (!phone) {
      const panel = page.locator('[data-lens-panel]');
      if (!await panel.evaluate(element => element.open)) await panel.locator(':scope > summary').click();
    }
    assert.deepEqual(await cameraBoxes(page), fitted, 'opening the lens never moves the camera');
    await page.locator(`${phone ? '[data-lens-sheet]' : '[data-lens-panel]'} [data-lens-walk-start="${path.name}"]`).click();
    for (let index = 0; index < path.stops.length; index++) {
      const stop = path.stops[index];
      if (index > 0) {
        if (phone) await walk.locator('[data-lens-walk="next"]').click();
        else await page.keyboard.press('ArrowRight');
      }
      await assertFramed(stop.spec, index + 1);
      const hop = path.hops.find(item => item.to === index);
      const provenance = await walk.locator('[data-lens-walk-hop]').getAttribute('data-lens-walk-hop');
      assert.equal(provenance, index === 0 ? 'start' : hop.source, `step ${index + 1} names its hop provenance`);
      if (hop?.source === 'declared') assert.match(await walk.textContent(), /render_template reads current_app/);
      const state = index === 0 ? 'first' : index === path.stops.length - 1 ? 'last' : 'middle';
      assert.equal(await walk.getAttribute('data-lens-walk-state'), state);
      if (!phone && index === 0) {
        // Just far enough: one keyboard zoom-out drops the stop under its gate.
        await page.keyboard.press('-');
        await settleCamera(page);
        assert.equal(await page.locator(`[data-lens-stop="${stop.spec}"]`).count(), 0, 'the first step zooms only to its gate');
        await page.keyboard.press('ArrowLeft');
        assert.equal(params().get('step'), '1', 'Left at the first step stays there');
        await page.keyboard.press('+');
        await settleCamera(page);
      }
      if (index === 0 || index === path.stops.length - 1) await shot(`step-${index + 1}-${state}`);
      console.log(`${profile}/${theme}: step ${index + 1} ${stop.spec}`);
    }
    assert.equal(await page.evaluate(() => history.length), history, 'steps replace the URL');
    // The source reader opens from the step's card and closes before it.
    await page.getByRole('button',{name:'Read selected source',exact:true}).click();
    await page.locator('[data-source-reader] .cm-content').waitFor();
    await shot('step-source');
    await page.goBack();
    await page.locator('[data-source-reader]').waitFor({state:'detached'});
    assert.equal(params().get('step'), String(path.stops.length), 'closing the reader keeps the step');
    // Back follows §3.4 and never returns to the previous step.
    await page.goBack();
    await page.waitForTimeout(300);
    assert.notEqual(new URL(page.url()).searchParams.get('step'), String(path.stops.length - 1), 'Back never walks back through steps');
    // A deep link restores the step; ending it and fitting shows the same layout.
    await page.goto(`${url}&lens=flask-request&path=${path.name}&step=2`);
    await assertFramed(path.stops[1].spec, 2);
    await walk.locator('[data-lens-walk="exit"]').click();
    assert.equal(params().get('step'), null);
    if (phone) await page.getByRole('button',{name:'Fit map',exact:true}).click();
    else await page.keyboard.press('f');
    await settleCamera(page);
    if (!phone) assert.deepEqual(await cameraBoxes(page), fitted, 'district layout is unchanged after a walkthrough');
    // An unresolved stop shows its reason and leaves the camera where it is.
    await page.goto(`${url}&lens=unresolved&path=missing-stop&step=1`);
    await assertFramed(broken.stops[0].spec, 1);
    const camera = await cameraBoxes(page);
    if (phone) await walk.locator('[data-lens-walk="next"]').click();
    else await page.keyboard.press('ArrowRight');
    await page.locator('[data-lens-walkthrough][data-lens-walk-state="unresolved"]').waitFor();
    assert.match(await walk.textContent(), /missing_path/);
    assert.equal(params().get('file'), null, 'an unresolved step clears the card');
    await settleCamera(page);
    assert.deepEqual(await cameraBoxes(page), camera, 'an unresolved step never moves the camera');
    await shot('step-unresolved');
    assert.ok(mapResponses.length >= 1);
    assert.deepEqual(errors, [], `${profile}/${theme} browser errors`);
    await context.close();
  }
} catch (error) {
  await activePage?.screenshot({path:'shots/lens-walkthrough/failure.png',animations:'disabled'});
  await writeFile('shots/lens-walkthrough/failure.txt', `${error.stack}\nURL: ${activePage?.url()}\n${await activePage?.locator('body').innerText()}`);
  throw error;
} finally {
  await writeFile('shots/lens-walkthrough/manifest.json',JSON.stringify({commit,fixture:'data/flask.json',lens:'data/lenses/flask-request.toml',screenshots},null,2));
  await browser.close();
}
