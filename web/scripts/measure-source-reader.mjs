// CI only. Actual production reader/API, measured observations, no invented SLA.
import { chromium } from 'playwright';
import fs from 'node:fs';
import assert from 'node:assert/strict';
const fixture=JSON.parse(fs.readFileSync('../.local-reader/fixture.json'));
const browser=await chromium.launch({headless:true});
const samples=[];
try {
 const page=await browser.newPage({viewport:{width:1280,height:900},reducedMotion:'reduce'});
 const cdp=await page.context().newCDPSession(page);
 await cdp.send('Performance.enable');
 const heap=async()=>{await cdp.send('HeapProfiler.collectGarbage');return (await cdp.send('Performance.getMetrics')).metrics.find(m=>m.name==='JSHeapUsedSize')?.value??null;};
 for(const file of ['perf.py','long.txt']) {
  await page.goto(`http://127.0.0.1:8988/snapshot/${fixture.snapshotA}?file=${file}`);
  await page.getByRole('button',{name:'Read selected source'}).waitFor();
  const source=await (await page.request.get(`http://127.0.0.1:8988/api/maps/snapshot/${fixture.snapshotA}/source?path=${file}`)).json();
  assert(source.status==='ready'&&source.bytes>800000&&source.bytes<=1048576);
  const baselineHeap=await heap();
  for(let iteration=0;iteration<3;iteration++) {
   const start=performance.now();
   await page.getByRole('button',{name:'Read selected source'}).click();
   await page.locator('.cm-content').waitFor();
   const loadMs=performance.now()-start;
   if(file.endsWith('.py')) await page.waitForFunction(()=>document.querySelector('.cm-content .cm-line span[class]'));
   else await page.getByText('Plain text · long line protection',{exact:false}).waitFor();
   const languageReadyMs=performance.now()-start;
   await page.getByRole('button',{name:'Find in file'}).click();
   const searchStart=performance.now();
   await page.locator('.cm-search input[name=search]').fill('PERF_NEEDLE_739');
   await page.locator('.cm-search button[name=next]').click();
   await page.locator('.cm-searchMatch').first().waitFor();
   const searchMs=performance.now()-searchStart;
   const scrollMs=await page.locator('.cm-scroller').evaluate(async el=>{const t=performance.now();el.scrollTop=el.scrollHeight;el.scrollLeft=el.scrollWidth;await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));return performance.now()-t;});
   const mountedHeap=await heap();
   const closeStart=performance.now();
   await page.getByRole('button',{name:'Return to map'}).click();
   await page.waitForFunction(()=>!document.querySelector('.cm-editor'));
   const destroyMs=performance.now()-closeStart;
   samples.push({file,bytes:source.bytes,lines:source.text.split('\n').length,longLine:source.long_line,iteration,loadMs,languageReadyMs,searchMs,scrollMs,destroyMs,baselineHeap,mountedHeap,afterDestroyHeap:await heap()});
  }
 }
 fs.writeFileSync('../reader-evidence/performance.json',JSON.stringify({sourceSha:process.env.GITHUB_SHA??null,runId:process.env.GITHUB_RUN_ID??null,browser:browser.version(),samples,scope:'Desktop Chromium CI; load includes API/React/automation; languageReady includes lazy grammar; scroll is two animation frames; heap is CDP after GC, not process RSS or leak proof; iteration 0 is first mount per file, later iterations warm. No phone/Safari performance claim.'},null,2));
 console.log(JSON.stringify(samples,null,2));
} finally {await browser.close();}
