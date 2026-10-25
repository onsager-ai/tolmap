// Synthetic protocol fixture over the production map/panel/reader. CI only.
import { chromium } from 'playwright';
import { readFile, mkdir } from 'node:fs/promises';
import assert from 'node:assert/strict';
const baseUrl = process.argv[2] ?? 'http://127.0.0.1:5176';
const base = 'a'.repeat(40), head = 'b'.repeat(40);
const map = JSON.parse(await readFile(new URL('../../data/flask.json', import.meta.url), 'utf8'));
const paths = map.F.slice(0, 4);
const d = map.N[0][0];
const file = (path, status, base_path = null) => ({path,status,base_path,district:status === 'added' ? null : d,placed:false});
const value = {id:'fixture-comparison',report:{version:1,base,head,base_files:map.F.length,base_districts:Object.keys(map.districts).length,districts_crossed:1,districts:[{id:d,name:map.names[String(d)] ?? 'fixture district',changed_files:3}],files:[file(paths[0],'modified'),file(paths[1],'deleted'),file('renamed.py','renamed',paths[2]),file('alone.py','added')],unplaced_files:['alone.py'],modularity_base:0.5,modularity_head:0.499,delta_q:-0.001,edges_added:[{source:paths[0],target:paths[3],source_district:d,target_district:d+1,weight:0.2,static_import:true}],edges_removed:[{source:paths[1],target:paths[3],source_district:d,target_district:d+1,weight:0.3,static_import:false}],landmark_touches:[{file:paths[0],kind:'hazard',detail:'Synthetic hazard context'}],thresholds:{source:'default',max_districts:4,max_dq:0.0001},verdict:'fail',lower_bound:true},provenance:{repository:'fixture/town',base,head,report_version:1,thresholds:{source:'default',max_districts:4,max_dq:0.0001},base_map_source:'stored',base_map_sha256:'fixture-sha256',base_files:map.F.length,base_districts:Object.keys(map.districts).length,indexed_changes:4,outside_graph:[file('README.md','modified')],tolmap_version:'fixture',build_revision:'fixture',resolution:1.1,prune:'node-relative',seed:7,refs:'hand',created_at:'0'}};
await mkdir('shots/comparison', {recursive:true});
const browser = await chromium.launch({headless:true});
try {
  for (const [profile, viewport] of [['desktop',{width:1440,height:900}],['phone',{width:390,height:844}]]) {
    const context = await browser.newContext({viewport,hasTouch:profile === 'phone',isMobile:profile === 'phone'});
    const page = await context.newPage();
    let state = 'ready';
    await page.route('**/api/**', async route => {
      const url = new URL(route.request().url());
      let json;
      if (url.pathname === '/api/healthz') json = {status:'ok'};
      else if (url.pathname === '/api/maps') json = [];
      else if (url.pathname === '/api/compare/fixture/town') {
        if (state === 'error') return route.fulfill({status:404,json:{error:'comparison_unavailable',message:'Committed source unavailable'}});
        json = state === 'stale' ? {...value,report:{...value.report,head:base}} : value;
      } else if (url.pathname.endsWith('/provenance')) json = {version:1,repository:'fixture/town',commit:url.searchParams.get('commit')};
      else if (url.pathname.endsWith('/source')) json = {version:1,repository:'fixture/town',commit:url.searchParams.get('commit'),path:url.searchParams.get('path'),status:'ready',text:'COMPARISON_SOURCE = 1\n',bytes:22,long_line:false};
      else if (url.pathname === '/api/maps/fixture/town') { assert.equal(url.searchParams.get('commit'),base); json = map; }
      else return route.fulfill({status:404,json:{error:'not_found'}});
      return route.fulfill({json});
    });
    const url = `${baseUrl}/fixture/town?compare=true&base=${base}&head=${head}`;
    await page.goto(url);
    const panel = page.getByRole('complementary', {name:'Change impact'});
    await panel.getByText('Structural signal:',{exact:false}).waitFor();
    for (const name of ['Affected districts','Changed files','Unplaced files','Outside the graph','Cross-district edges added','Cross-district edges removed','Landmark touches']) await panel.getByRole('heading',{name,exact:true}).waitFor();
    const item = panel.locator('li').filter({has:page.getByRole('button',{name:paths[1],exact:true})});
    assert.equal(await item.getByRole('link',{name:'head source'}).count(),0,'deleted file has no head');
    const rename = panel.locator('li').filter({has:page.getByRole('button',{name:`${paths[2]} → renamed.py`,exact:true})});
    assert.equal(new URL(await rename.getByRole('link',{name:'base source'}).getAttribute('href'),baseUrl).searchParams.get('file'),paths[2]);
    const first = panel.getByRole('button',{name:'Return to map',exact:true});
    await first.focus(); await page.keyboard.press('ArrowDown');
    assert.equal(await page.getByRole('textbox',{name:'Base commit',exact:true}).evaluate(e => e === document.activeElement),true,'arrow navigation');
    await panel.getByRole('heading',{name:'Change impact',exact:true}).scrollIntoViewIfNeeded();
    await page.screenshot({path:`shots/comparison/${profile}-form.png`});
    await panel.getByText('Structural signal:',{exact:false}).scrollIntoViewIfNeeded();
    await page.screenshot({path:`shots/comparison/${profile}-result.png`});
    const link = panel.locator('li').filter({has:page.getByRole('button',{name:paths[0],exact:true})}).getByRole('link',{name:'head source'});
    await link.click();
    await page.locator('.cm-content').getByText('COMPARISON_SOURCE = 1').waitFor();
    assert.equal(new URL(page.url()).searchParams.get('base'),base);
    assert.equal(new URL(page.url()).searchParams.get('source_commit'),head);
    await page.getByRole('button',{name:'Return to map',exact:true}).first().click();
    await panel.waitFor();
    await page.reload();
    await panel.getByText('Structural signal:',{exact:false}).waitFor();
    for (const mode of ['stale','error']) {
      state = mode; await page.reload();
      await panel.getByRole('alert').waitFor();
      assert.equal(await panel.getByText('Structural signal:',{exact:false}).count(),0,'old result hidden');
    }
    await context.close();
  }
} finally { await browser.close(); }
console.log('comparison desktop/phone, lists, keyboard, source sides, reload and stale/unavailable states passed');
