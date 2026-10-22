import { chromium } from 'playwright';
import fs from 'node:fs';
import {cameraBoxes,settleCamera} from './camera-settle.mjs';
import assert from 'node:assert/strict';
const fixture=JSON.parse(fs.readFileSync('../.local-reader/fixture.json'));
fs.mkdirSync('../reader-evidence',{recursive:true});
const browser=await chromium.launch({headless:true});
const results=[];
async function copySelection(page, expected){
 await page.locator('.cm-content').focus();
 await page.waitForFunction(value=>window.getSelection()?.toString()===value,expected);
 await page.keyboard.press('Control+c');
 await page.waitForFunction(async value=>(await navigator.clipboard.readText())===value,expected);
}
try{
for(const width of [390,1280])for(const colorScheme of ['light','dark']){
 const context=await browser.newContext({viewport:{width,height:900},colorScheme,hasTouch:width===390,reducedMotion:'reduce',permissions:['clipboard-read','clipboard-write']});
 const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.addInitScript(()=>{window.readerInitialHistoryLength=history.length;window.readerHistory=[{kind:'load',url:location.href,state:history.state}];for(const kind of ['pushState','replaceState']){const original=history[kind].bind(history);history[kind]=(...args)=>{original(...args);window.readerHistory.push({kind,url:location.href,state:history.state});};}addEventListener('popstate',()=>window.readerHistory.push({kind:'pop',url:location.href,state:history.state}));});
 await page.goto('http://127.0.0.1:8988/');await page.getByRole('heading',{name:'Your local repository'}).waitFor();await page.screenshot({path:`../reader-evidence/local-${width}-${colorScheme}.png`,animations:'disabled'});
 const base=`http://127.0.0.1:8988/snapshot/${fixture.snapshotA}?file=${fixture.file}`;
 await page.goto(base);await page.locator('.map-svg').waitFor();await page.getByRole('button',{name:'Read selected source'}).waitFor();await settleCamera(page);
 const before=await cameraBoxes(page);
 const sourceAction=page.getByRole('button',{name:'Read selected source'});
 if(width===390)await sourceAction.tap();else await sourceAction.click();
 await page.locator('.cm-content').waitFor();
 assert.equal(await page.locator('.cm-content').getAttribute('contenteditable'),'false');
 const original=await page.locator('.cm-content').innerText();
 await page.waitForFunction(()=>document.querySelector('.cm-content .cm-line span[class]'));
 await page.locator('.cm-content').focus();await page.keyboard.type('cannot edit');
 assert.equal(await page.locator('.cm-content').innerText(),original);
 await page.keyboard.press('Control+a');await page.keyboard.press('Control+c');
 assert((await page.evaluate(()=>navigator.clipboard.readText())).includes('def '));
 await page.locator('.cm-content').evaluate(el=>{const data=new DataTransfer();data.setData('text/plain','mutated paste');el.dispatchEvent(new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true}));el.dispatchEvent(new DragEvent('drop',{dataTransfer:data,bubbles:true,cancelable:true}));});assert.equal(await page.locator('.cm-content').innerText(),original);
 await page.getByRole('button',{name:'Find in file'}).click();
 const search=page.locator('.cm-search input[name=search]');await search.fill('def');await page.locator('.cm-search button[name=next]').click();
 assert(await page.locator('.cm-search').isVisible());
 await search.fill('no_such_fixture_739');await page.locator('.cm-search button[name=next]').click();
 await page.getByText('No results',{exact:true}).waitFor();
 const afterTyping=await cameraBoxes(page);assert.deepEqual(afterTyping,before,'source typing changed map geometry');
 await page.keyboard.press('Escape');
 await page.screenshot({path:`../reader-evidence/reader-${width}-${colorScheme}.png`,animations:'disabled'});
 await page.getByRole('button',{name:'Return to map'}).click();assert.equal(await page.locator('[data-source-reader]').count(),0);
 assert(await page.getByRole('button',{name:'Read selected source'}).evaluate(el=>el===document.activeElement));
 await settleCamera(page);assert.deepEqual(await cameraBoxes(page),before);
 await page.getByRole('button',{name:'Read selected source'}).click();await page.locator('.cm-content').waitFor();
 await page.goBack();await page.waitForFunction(()=>!document.querySelector('[data-source-reader]'));
 assert(new URL(page.url()).searchParams.get('file')===fixture.file);await settleCamera(page);assert.deepEqual(await cameraBoxes(page),before);
 await page.goto(base+'&read=true&line=2.5&end=4.5');await page.locator('.cm-content').waitFor();
 if(width===390)assert.equal(await page.evaluate(()=>history.length-window.readerInitialHistoryLength),2,'selection and reader must create distinct native history entries');
 await page.reload();await page.locator('.cm-content').waitFor();
 assert((await page.locator('.source-provenance').first().innerText()).includes(fixture.a));
 const committed=await(await page.request.get(`http://127.0.0.1:8988/api/maps/snapshot/${fixture.snapshotA}/source?commit=${fixture.a}&path=${encodeURIComponent(fixture.file)}`)).json();
 await copySelection(page,committed.text.split('\n').slice(1,4).join('\n'));
 await page.goBack();await page.waitForFunction(()=>!document.querySelector('[data-source-reader]'));assert.equal(new URL(page.url()).searchParams.get('file'),fixture.file,JSON.stringify(await page.evaluate(()=>window.readerHistory)));
 await page.goto(base.replace(fixture.file,'inert.txt')+'&read=true');await page.locator('.cm-content').waitFor();assert.equal(await page.evaluate(()=>window.sourceInjected),undefined);assert.equal(await page.locator('.cm-content script').count(),0);
 await page.goto(base.replace(fixture.file,'long.txt')+'&read=true');await page.locator('.cm-content').waitFor();assert(await page.getByText('Plain text · long line protection',{exact:false}).isVisible());
 for(const [file,message] of [['binary.dat','Binary or non-UTF-8'],['large.txt','exceeds the 1 MiB'],['absent','missing from this snapshot']]){
  await page.goto(base.replace(fixture.file,file)+'&read=true');await page.getByText(message,{exact:false}).waitFor();
 }
 // Deliberate browser network failure exercises loading/error/retry on the
 // production reader. It is distinct from the real API/process evidence.
 await page.route('**/api/maps/snapshot/*/source?*',route=>route.abort());
 await page.goto(base+'&read=true');await page.getByRole('button',{name:'Retry source'}).waitFor();
 await page.unroute('**/api/maps/snapshot/*/source?*');await page.getByRole('button',{name:'Retry source'}).click();await page.locator('.cm-content').waitFor();
 await page.getByRole('button',{name:'Return to map'}).click();assert.equal(await page.locator('.cm-editor').count(),0);
 if(width===1280){
  // The actual desktop panel remains usable beside the reader. A new
  // selection must replace the source/range/query atomically, and closing
  // reading must preserve that live map selection rather than the old one.
  await page.goto(base);await page.getByRole('button',{name:'Read selected source'}).click();await page.locator('.cm-content').waitFor();
  await page.getByRole('button',{name:'Find in file'}).click();await page.locator('.cm-search input[name=search]').fill('missing_before_navigation_739');await page.locator('.cm-search button[name=next]').click();await page.getByText('No results',{exact:true}).waitFor();await page.keyboard.press('Escape');
  const map=await(await page.request.get(`http://127.0.0.1:8988/api/maps/snapshot/${fixture.snapshotA}`)).json();
  const symbol=page.locator('[data-desktop-panel] button[data-outline-row]').first();await symbol.waitFor();const global=Number(await symbol.getAttribute('data-outline-row'));
  const district=map.N[map.F.indexOf(fixture.file)][0];
  const symbols=await(await page.request.get(`http://127.0.0.1:8988/api/maps/snapshot/${fixture.snapshotA}/symbols?district=${district}`)).json();
  const row=symbols.symbols[symbols.symbol_indices.indexOf(global)];assert(row);
  await symbol.click();await page.locator('.cm-content').waitFor();
  assert.equal(Number(new URL(page.url()).searchParams.get('line')),row[3]);assert.equal(Number(new URL(page.url()).searchParams.get('end')),row[4]);
  await page.getByText('Search this file',{exact:true}).waitFor();assert.equal(await page.locator('.cm-search').count(),0);
  await copySelection(page,committed.text.split('\n').slice(row[3]-1,row[4]).join('\n'));
  await page.locator('[data-desktop-panel] [data-link-count=out]').click();const target=page.locator('[data-desktop-panel] [data-sheet-file]').first();await target.waitFor();const file=map.F[Number(await target.getAttribute('data-sheet-file'))];assert.notEqual(file,fixture.file);
  await target.click();await page.locator('.cm-content').waitFor();assert.equal(new URL(page.url()).searchParams.get('file'),file);assert.equal(new URL(page.url()).searchParams.get('line'),'1');assert.equal(new URL(page.url()).searchParams.get('end'),'1');
  assert.equal(await page.locator('.source-reader header strong').innerText(),file);await page.getByText('Search this file',{exact:true}).waitFor();
  const nextSource=await(await page.request.get(`http://127.0.0.1:8988/api/maps/snapshot/${fixture.snapshotA}/source?commit=${fixture.a}&path=${encodeURIComponent(file)}`)).json();
  await copySelection(page,nextSource.text.split('\n')[0]);
  await settleCamera(page);const selectedCamera=await cameraBoxes(page);
  await page.getByRole('button',{name:'Return to map'}).click();await page.waitForFunction(()=>!document.querySelector('[data-source-reader]'));assert.equal(new URL(page.url()).searchParams.get('file'),file);await settleCamera(page);assert.deepEqual(await cameraBoxes(page),selectedCamera);
  await page.getByRole('button',{name:'Read selected source'}).click();await page.locator('.cm-content').waitFor();await page.goBack();await page.waitForFunction(()=>!document.querySelector('[data-source-reader]'));assert.equal(new URL(page.url()).searchParams.get('file'),file);
 }
 assert.deepEqual(errors,[]);results.push({width,colorScheme,passed:true,images:'unreviewed candidates',device:'Chromium emulation',browser:browser.version()});await context.close();
}
fs.writeFileSync('../reader-evidence/browser.json',JSON.stringify({fixture,results,visualReview:'pending; host image inspection prohibited',physicalDevice:'not run'},null,2));
}finally{await browser.close()}
