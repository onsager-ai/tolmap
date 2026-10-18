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
 const context=await browser.newContext({viewport:{width,height:900},colorScheme,hasTouch:width===390,reducedMotion:'reduce'});
 const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const base=`http://127.0.0.1:8988/snapshot/${fixture.snapshotA}?file=${fixture.file}`;
 await page.goto(base);await page.locator('.map-svg').waitFor();await page.getByRole('button',{name:'Read selected source'}).waitFor();await settleCamera(page);
 const before=await cameraBoxes(page);
 await page.getByRole('button',{name:'Read selected source'}).click();
 await page.locator('.cm-content').waitFor();
 assert.equal(await page.locator('.cm-content').getAttribute('contenteditable'),'false');
 const original=await page.locator('.cm-content').innerText();
 await page.locator('.cm-content').focus();await page.keyboard.type('cannot edit');
 assert.equal(await page.locator('.cm-content').innerText(),original);
 await page.getByRole('button',{name:'Find in file'}).click();
 const search=page.locator('.cm-search input[name=search]');await search.fill('def');await page.locator('.cm-search button[name=next]').click();
 assert(await page.locator('.cm-search').isVisible());
 await search.fill('no_such_fixture_739');await page.locator('.cm-search button[name=next]').click();
 assert(await search.getAttribute('class').then(s=>s.includes('cm-searchField')) || await search.isVisible());
 const afterTyping=await cameraBoxes(page);assert.deepEqual(afterTyping,before,'source typing changed map geometry');
 await page.keyboard.press('Escape');
 await page.screenshot({path:`../reader-evidence/reader-${width}-${colorScheme}.png`,animations:'disabled'});
 await page.getByRole('button',{name:'Return to map'}).click();assert.equal(await page.locator('[data-source-reader]').count(),0);
 await settleCamera(page);assert.deepEqual(await cameraBoxes(page),before);
 await page.getByRole('button',{name:'Read selected source'}).click();await page.locator('.cm-content').waitFor();
 await page.goBack();await page.waitForFunction(()=>!document.querySelector('[data-source-reader]'));
 assert(new URL(page.url()).searchParams.get('file')===fixture.file);await settleCamera(page);assert.deepEqual(await cameraBoxes(page),before);
 await page.goto(base+'&read=true&line=2&end=4');await page.locator('.cm-content').waitFor();await page.reload();await page.locator('.cm-content').waitFor();
 assert((await page.locator('.source-provenance').first().innerText()).includes(fixture.a));
 await page.goto(base.replace(fixture.file,'inert.txt')+'&read=true');await page.locator('.cm-content').waitFor();assert.equal(await page.evaluate(()=>window.sourceInjected),undefined);
 await page.goto(base.replace(fixture.file,'long.txt')+'&read=true');await page.locator('.cm-content').waitFor();assert(await page.getByText('Plain text · long line protection',{exact:false}).isVisible());
 for(const [file,message] of [['binary.dat','Binary or non-UTF-8'],['large.txt','exceeds the 1 MiB'],['absent','missing from this snapshot']]){
  await page.goto(base.replace(fixture.file,file)+'&read=true');await page.getByText(message,{exact:false}).waitFor();
 }
 assert.deepEqual(errors,[]);results.push({width,colorScheme,passed:true,images:'unreviewed candidates',device:'Chromium emulation',browser:browser.version()});await context.close();
}
fs.writeFileSync('../reader-evidence/browser.json',JSON.stringify({fixture,results,visualReview:'pending; host image inspection prohibited',physicalDevice:'not run'},null,2));
}finally{await browser.close()}
