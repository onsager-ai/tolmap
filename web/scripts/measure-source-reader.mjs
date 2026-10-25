// CI only. Actual production reader/API, measured observations, no invented SLA.
import { chromium } from 'playwright';
import fs from 'node:fs';
import assert from 'node:assert/strict';
const fixture=JSON.parse(fs.readFileSync('../.local-reader/fixture.json'));
const runEvent=process.env.GITHUB_EVENT_PATH?JSON.parse(fs.readFileSync(process.env.GITHUB_EVENT_PATH,'utf8')):{};
const sourceSha=runEvent.pull_request?.head?.sha??process.env.GITHUB_SHA??null;
const baseSha=runEvent.pull_request?.base?.sha??null;
const testedSha=process.env.GITHUB_SHA??null;
const browser=await chromium.launch({headless:true});
const samples=[],summaries=[];
const cycles=14,warmupCycles=2;
const classificationRule='Observation heuristic, not an SLA: linear growth observed when the warm-cycle least-squares slope exceeds max(65536 bytes/cycle, 1% of mean warm after-destroy heap/cycle); otherwise plateau observed at this resolution. Inspect residuals and retention counts; neither label proves leak presence or absence.';
function summarize(file){
 const warm=samples.filter(s=>s.file===file&&!s.warmup);
 const mean=values=>values.reduce((a,b)=>a+b,0)/values.length;
 const x=mean(warm.map(s=>s.iteration)),y=mean(warm.map(s=>s.afterDestroyHeap));
 const slope=warm.reduce((sum,s)=>sum+(s.iteration-x)*(s.afterDestroyHeap-y),0)/warm.reduce((sum,s)=>sum+(s.iteration-x)**2,0);
 const residualRms=Math.sqrt(mean(warm.map(s=>(s.afterDestroyHeap-(y+slope*(s.iteration-x)))**2)));
 const threshold=Math.max(65536,y*.01);
 return{file,cycles,warmupCycles,measuredCycles:warm.length,meanAfterDestroyHeap:y,slopeBytesPerCycle:slope,residualRmsBytes:residualRms,thresholdBytesPerCycle:threshold,classification:slope>threshold?'linear growth observed':'plateau observed',maxDetachedEditors:Math.max(...warm.map(s=>s.retention.detachedEditors)),maxConnectedEditorsAfterDestroy:Math.max(...warm.map(s=>s.retention.connectedEditors)),classificationRule};
}
try {
 const page=await browser.newPage({viewport:{width:1280,height:900},reducedMotion:'reduce'});
 const cdp=await page.context().newCDPSession(page);
 await cdp.send('Performance.enable');
 const settle=()=>page.evaluate(async()=>{
  await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
  await new Promise(r=>{if('requestIdleCallback' in window)requestIdleCallback(()=>r(),{timeout:100});else setTimeout(r,50);});
 });
 const heap=async()=>{await cdp.send('HeapProfiler.collectGarbage');const value=(await cdp.send('Performance.getMetrics')).metrics.find(m=>m.name==='JSHeapUsedSize')?.value;assert(Number.isFinite(value),'CDP heap metric missing');return value;};
 // Query live DIV objects after GC, including detached roots unreachable by
 // document.querySelector. Release handles so this probe cannot retain editors.
 // This does not count non-DOM EditorView objects or process RSS.
 const retention=async()=>{
  const objectGroup='reader-retention';
  try{
   const {result:prototype}=await cdp.send('Runtime.evaluate',{expression:'HTMLDivElement.prototype',objectGroup});
   const {objects}=await cdp.send('Runtime.queryObjects',{prototypeObjectId:prototype.objectId,objectGroup});
   const {result}=await cdp.send('Runtime.callFunctionOn',{objectId:objects.objectId,functionDeclaration:'function(){let detachedEditors=0,connectedEditors=0;for(const el of this){if(el.classList?.contains("cm-editor")){if(el.isConnected)connectedEditors++;else detachedEditors++;}}return {detachedEditors,connectedEditors};}',returnByValue:true});
   assert(result.value,'CDP retention probe missing');return result.value;
  }finally{await cdp.send('Runtime.releaseObjectGroup',{objectGroup});}
 };
 for(const file of ['shop/perf.py','shop/perf_long.py']) {
  await page.goto(`http://127.0.0.1:8988/snapshot/${fixture.snapshotA}?file=${file}`);
  await page.getByRole('button',{name:'Read selected source'}).waitFor();
  const source=await (await page.request.get(`http://127.0.0.1:8988/api/maps/snapshot/${fixture.snapshotA}/source?path=${file}`)).json();
  assert(source.status==='ready'&&source.bytes>800000&&source.bytes<=1048576);
  await settle();const baselineHeap=await heap();
  for(let iteration=0;iteration<cycles;iteration++) {
   const start=performance.now();
   await page.getByRole('button',{name:'Read selected source'}).click();
   await page.locator('.cm-content').waitFor();
   const loadMs=performance.now()-start;
   if(!source.long_line) await page.waitForFunction(()=>document.querySelector('.cm-content .cm-line span[class]'));
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
   await settle();const afterDestroyHeap=await heap();
   samples.push({file,bytes:source.bytes,lines:source.text.split('\n').length,longLine:source.long_line,iteration,warmup:iteration<warmupCycles,loadMs,languageReadyMs,searchMs,scrollMs,destroyMs,baselineHeap,mountedHeap,afterDestroyHeap,retention:await retention()});
  }
  summaries.push(summarize(file));
 }
 fs.mkdirSync('../reader-evidence',{recursive:true});
 fs.writeFileSync('../reader-evidence/performance.json',JSON.stringify({sourceSha,baseSha,testedSha,runId:process.env.GITHUB_RUN_ID??null,browser:browser.version(),cycles,warmupCycles,classificationRule,summaries,samples,scope:'Desktop Chromium CI; load includes API/React/automation; languageReady includes lazy grammar; after destroy settles two animation frames plus idle before CDP GC. First two cycles are warmup. DIV queryObjects probe counts connected/detached cm-editor roots and releases handles each cycle; it cannot detect all non-DOM retention. Heap is not process RSS or proof of a leak. No phone/Safari performance claim.'},null,2));
 console.log(JSON.stringify({summaries},null,2));
} finally {await browser.close();}
