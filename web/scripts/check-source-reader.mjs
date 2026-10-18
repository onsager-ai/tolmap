import { chromium } from 'playwright';
import fs from 'node:fs';
import {cameraBoxes,settleCamera} from './camera-settle.mjs';
import assert from 'node:assert/strict';
const fixture=JSON.parse(fs.readFileSync('../.local-reader/fixture.json'));
fs.mkdirSync('../reader-evidence',{recursive:true});
const browser=await chromium.launch({headless:true});
const results=[];
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
 await page.locator('.cm-content').focus();await page.keyboard.press('Control+c');
 assert.equal(await page.evaluate(()=>navigator.clipboard.readText()),committed.text.split('\n').slice(1,4).join('\n'),'deep link selects the exact committed line range');
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
 assert.deepEqual(errors,[]);results.push({width,colorScheme,passed:true,images:'unreviewed candidates',device:'Chromium emulation',browser:browser.version()});await context.close();
}
fs.writeFileSync('../reader-evidence/browser.json',JSON.stringify({fixture,results,visualReview:'pending; host image inspection prohibited',physicalDevice:'not run'},null,2));
}finally{await browser.close()}
