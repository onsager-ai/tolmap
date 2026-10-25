import { chromium } from 'playwright';
import fs from 'node:fs';
import {createHash} from 'node:crypto';
import {cameraBoxes,settleCamera} from './camera-settle.mjs';
import assert from 'node:assert/strict';
const fixture=JSON.parse(fs.readFileSync('../.local-reader/fixture.json'));
const runEvent=process.env.GITHUB_EVENT_PATH?JSON.parse(fs.readFileSync(process.env.GITHUB_EVENT_PATH,'utf8')):{};
const sourceSha=runEvent.pull_request?.head?.sha??process.env.GITHUB_SHA??null;
const baseSha=runEvent.pull_request?.base?.sha??null;
const testedSha=process.env.GITHUB_SHA??null;
fs.mkdirSync('../reader-evidence',{recursive:true});
const browser=await chromium.launch({headless:true});
const results=[];
const browserOrigin='http://127.0.0.1:8988';
const contrast=(a,b)=>{const lum=s=>{const rgb=s.match(/[\d.]+/g).slice(0,3).map(Number).map(x=>{x/=255;return x<=.04045?x/12.92:((x+.055)/1.055)**2.4});return rgb[0]*.2126+rgb[1]*.7152+rgb[2]*.0722;};const [x,y]=[lum(a),lum(b)].sort((x,y)=>y-x);return(x+.05)/(y+.05);};
async function inspectReaderContrast(page){
 const result=await page.locator('.cm-content').evaluate(el=>{
  const editor=el.closest('.cm-editor'),selection=editor.querySelector('.cm-selectionBackground');
  if(!selection)throw new Error('deep-linked selection background is missing');
  const background=getComputedStyle(editor).backgroundColor,selectionBackground=getComputedStyle(selection).backgroundColor;
  const tokens=[...el.querySelectorAll('span[class]')].filter(x=>x.textContent.trim()).map(x=>({text:x.textContent.slice(0,40),color:getComputedStyle(x).color}));
  return{background,selectionBackground,tokens};
 });
 assert(result.tokens.length,'no syntax tokens rendered');
 assert(new Set(result.tokens.map(x=>x.color)).size>=3,`expected three syntax colors, got ${JSON.stringify([...new Set(result.tokens.map(x=>x.color))])}`);
 for(const token of result.tokens){assert(contrast(token.color,result.background)>=4.5,`token on editor background: ${JSON.stringify(token)}`);assert(contrast(token.color,result.selectionBackground)>=4.5,`token on selected range: ${JSON.stringify(token)}`);}
 assert(contrast(result.selectionBackground,result.background)>=1.5,`selection tint is too close to editor background: ${JSON.stringify(result)}`);
 return{background:result.background,selectionBackground:result.selectionBackground,selectionContrast:contrast(result.selectionBackground,result.background),distinctTokenColors:[...new Set(result.tokens.map(x=>x.color))],minimumTokenContrast:Math.min(...result.tokens.flatMap(x=>[contrast(x.color,result.background),contrast(x.color,result.selectionBackground)]))};
}
async function inspectFindTargets(page,width,height){
 const panel=await page.locator('.cm-search').evaluate(el=>{
  const rect=x=>{const r=x.getBoundingClientRect();return{left:r.left,top:r.top,right:r.right,bottom:r.bottom,width:r.width,height:r.height};};
  const style=x=>{const s=getComputedStyle(x);return{display:s.display,padding:s.padding,paddingRight:s.paddingRight,overflow:s.overflow,maxHeight:s.maxHeight,background:s.backgroundColor,border:s.border,borderStyle:s.borderStyle,borderWidth:s.borderWidth,borderRightWidth:s.borderRightWidth};};
  const panelStyle=getComputedStyle(el),panelRect=rect(el),close=el.querySelector('button[name=close]');
  if(!close)return{panelRect,panelStyle:style(el),missingClose:true,targets:[]};
  const closeStyle=getComputedStyle(close),closeRect=rect(close),paddingRight=parseFloat(panelStyle.paddingRight)||0,borderRight=parseFloat(panelStyle.borderRightWidth)||0;
  const contentRight=panelRect.right-paddingRight-borderRight;
  const targets=[...el.querySelectorAll('label,input,button:not([name=close])')].map(node=>{
   const r=rect(node),targetStyle=getComputedStyle(node),tag=node.tagName.toLowerCase(),text=tag==='label'?node.innerText.trim().replace(/\s+/g,' '):'';
   const textRects=[];
   if(tag==='label'){
    const walker=document.createTreeWalker(node,NodeFilter.SHOW_TEXT);
    while(walker.nextNode()){
     const textNode=walker.currentNode;if(!textNode.textContent.trim())continue;
     const range=document.createRange();range.selectNodeContents(textNode);
     for(const textRect of range.getClientRects()){
      const t={left:textRect.left,top:textRect.top,right:textRect.right,bottom:textRect.bottom,width:textRect.width,height:textRect.height};
      const points=[[t.left+t.width/2,t.top+t.height/2],[Math.max(t.left,t.right-.5),t.top+t.height/2]];
      textRects.push({...t,hits:points.map(([x,y])=>{const hit=document.elementFromPoint(x,y);return!!hit&&node.contains(hit);})});
     }
    }
   }
   return{id:tag==='label'?`label:${text}`:`${tag}${node.getAttribute('type')?`[${node.getAttribute('type')}]`:''}${node.getAttribute('name')?`[name=${node.getAttribute('name')}]`:''}`,tag,type:node.getAttribute('type'),name:node.getAttribute('name'),text,rect:r,scrollWidth:node.scrollWidth,clientWidth:node.clientWidth,font:parseFloat(targetStyle.fontSize),textRects};
  });
  return{panelRect,panelStyle:style(el),contentRight,close:{rect:closeRect,style:{width:closeStyle.width,height:closeStyle.height,minWidth:closeStyle.minWidth,minHeight:closeStyle.minHeight,padding:closeStyle.padding,border:closeStyle.border,borderStyle:closeStyle.borderStyle,borderWidth:closeStyle.borderWidth,background:closeStyle.backgroundColor,color:closeStyle.color,position:closeStyle.position,right:closeStyle.right,top:closeStyle.top}},targets};
 });
 const at=`${width}x${height}`,details=JSON.stringify({panel:panel.panelStyle,panelRect:panel.panelRect,contentRight:panel.contentRight,close:panel.close});
 assert(!panel.missingClose,`Find close control missing at ${at}: ${details}`);
 assert(panel.targets.length,`Find panel has no measurable controls at ${at}: ${details}`);
 const close=panel.close.rect;
 assert(close.width>=44&&close.height>=44,`Find close target is smaller than 44x44 at ${at}: ${details}`);
 assert(close.left>=panel.panelRect.left-.5&&close.top>=panel.panelRect.top-.5&&close.right<=panel.panelRect.right+.5&&close.bottom<=panel.panelRect.bottom+.5,`Find close target is outside its panel at ${at}: ${details}`);
 assert(close.left>=-.5&&close.top>=-.5&&close.right<=width+.5&&close.bottom<=height+.5,`Find close target is outside viewport at ${at}: ${details}`);
 const closeBackground=panel.close.style.background;
 const visibleBorder=!['none','hidden'].includes(panel.close.style.borderStyle)&&parseFloat(panel.close.style.borderWidth)>=1;
 assert(visibleBorder||closeBackground!=='rgba(0, 0, 0, 0)',`Find close target has no visible border or background at ${at}: ${details}`);
 const overlaps=(a,b)=>Math.min(a.right,b.right)-Math.max(a.left,b.left)>.5&&Math.min(a.bottom,b.bottom)-Math.max(a.top,b.top)>.5;
 for(const target of panel.targets){
  const where=`${at} ${target.id}`,t=target.rect;
  assert(!overlaps(t,close),`Find control intersects close target at ${where}: ${JSON.stringify({control:t,close,contentRight:panel.contentRight})}`);
  assert(t.right<=panel.contentRight+.5,`Find control crosses panel right padding edge at ${where}: ${JSON.stringify({control:t,contentRight:panel.contentRight})}`);
  if(target.tag==='label'){
   assert(target.scrollWidth<=target.clientWidth,`Find option text is clipped at ${where}: ${JSON.stringify({scrollWidth:target.scrollWidth,clientWidth:target.clientWidth,rect:t})}`);
   assert(target.textRects.length,`Find option has no measurable text at ${where}`);
   for(const r of target.textRects)assert(r.left>=-.5&&r.right<=panel.contentRight+.5&&r.hits.length===2&&r.hits.every(Boolean),`Find option text is obscured at ${where}: ${JSON.stringify({textRect:r,close})}`);
  }
  if(target.tag==='input')assert(target.font>=16,`Find input is below 16px at ${where}: ${JSON.stringify(target)}`);
  if(target.tag==='button')assert(t.height>=44&&t.width>=44,`Find button target is smaller than 44x44 at ${where}: ${JSON.stringify(target)}`);
 }
 assert(panel.targets.some(x=>x.tag==='label'&&/\bby\s+word\b/i.test(x.text)),`Find panel is missing the by word option at ${at}`);
 return{panel:panel.panelStyle,panelRect:panel.panelRect,contentRight:panel.contentRight,close:panel.close,targets:panel.targets};
}
async function assertPhoneActionAndControls(page,width,height){
 const action=page.getByRole('button',{name:'Read selected source'}),actionBox=await action.boundingBox();
 assert(await action.isVisible()&&actionBox,`source action missing at ${width}x${height}`);
 assert(actionBox.x>=0&&actionBox.y>=0&&actionBox.x+actionBox.width<=width+1&&actionBox.y+actionBox.height<=height+1,`source action outside viewport: ${JSON.stringify(actionBox)}`);
 assert(await action.evaluate(el=>{const r=el.getBoundingClientRect();return el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2));}),'source action is covered');
 const column=page.locator('[data-control-column]'),buttons=column.locator('button'),count=await buttons.count();
 if(count){
  const actionRect={left:actionBox.x,right:actionBox.x+actionBox.width,top:actionBox.y,bottom:actionBox.y+actionBox.height};
  for(const button of await buttons.all()){
   const box=await button.boundingBox();assert(box&&box.x>=0&&box.y>=0&&box.x+box.width<=width+1&&box.y+box.height<=height+1,`map control outside viewport: ${JSON.stringify(box)}`);
   assert(await button.evaluate(el=>{const r=el.getBoundingClientRect();return el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2));}),'map control is covered');
   assert(box.x>=actionRect.right||box.x+box.width<=actionRect.left||box.y>=actionRect.bottom||box.y+box.height<=actionRect.top,`source action overlaps map control: ${JSON.stringify({actionBox,box})}`);
  }
  assert(count>=4,`partial map control column at ${width}x${height}: ${count}`);
 }
 return{detent:await page.locator('[data-phone-sheet]').getAttribute('data-detent'),sourceAction:actionBox,controlColumnVisible:count>0,controlCount:count};
}
async function matchStyle(locator){
 return locator.evaluate(el=>{const style=getComputedStyle(el),text=el.querySelector('span')??el;return{background:style.backgroundColor,color:getComputedStyle(text).color,outlineStyle:style.outlineStyle,outlineWidth:style.outlineWidth,outlineColor:style.outlineColor};});
}
async function inspectDOMSelection(page){
 return page.locator('.cm-content').evaluate(content=>{
  const selection=window.getSelection();
  if(!selection?.anchorNode||!selection.focusNode||!content.contains(selection.anchorNode)||!content.contains(selection.focusNode))return null;
  const offset=(node,position)=>{const range=document.createRange();range.selectNodeContents(content);range.setEnd(node,position);return range.toString().length;};
  const anchor=offset(selection.anchorNode,selection.anchorOffset),focus=offset(selection.focusNode,selection.focusOffset);
  return{from:Math.min(anchor,focus),to:Math.max(anchor,focus),text:selection.toString()};
 });
}
async function assertFindGeometryNegativeControl(page,width,height){
 const style=await page.addStyleTag({content:'.source-editor .cm-editor .cm-panel.cm-search{padding-right:4px!important}.source-editor .cm-editor .cm-panel.cm-search button[name=close]{box-sizing:border-box!important;width:18px!important;height:18px!important;min-width:0!important;min-height:0!important;padding:2px!important;border:0!important}'});
 let failureText='';
 try{await inspectFindTargets(page,width,height);}catch(error){failureText=error instanceof Error?error.message:String(error);}
 finally{await style.evaluate(element=>element.remove());}
 assert(failureText,`Find geometry oracle passed its injected negative control at ${width}x${height}`);
 assert(/Find close target is smaller than 44x44|Find control intersects close target/.test(failureText),`negative control failed for an unrelated reason at ${width}x${height}: ${failureText}`);
 return{label:'negative control of the geometry oracle; injected unreserved panel and undersized close target',failureText};
}
async function copySelection(page, expected){
 await page.locator('.cm-content').focus();
 await page.waitForFunction(value=>window.getSelection()?.toString()===value,expected);
 await page.keyboard.press('Control+c');
 await page.waitForFunction(async value=>(await navigator.clipboard.readText())===value,expected);
}
try{
for(const [width,height] of [[320,568],[390,844],[844,390],[768,1024],[1280,900]])for(const colorScheme of ['light','dark']){
 const narrow=width<=820||height<=500;
 const context=await browser.newContext({viewport:{width,height},colorScheme,hasTouch:narrow,reducedMotion:'reduce',permissions:['clipboard-read','clipboard-write']});
 const outbound=[];await context.route('**/*',route=>{const url=new URL(route.request().url());if(url.protocol==='http:'&&url.hostname==='127.0.0.1'&&url.port==='8988')return route.continue();if(['data:','blob:'].includes(url.protocol))return route.continue();outbound.push(url.origin);return route.abort();});
 const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.addInitScript(()=>{window.readerInitialHistoryLength=history.length;window.readerHistory=[{kind:'load',url:location.href,state:history.state}];for(const kind of ['pushState','replaceState']){const original=history[kind].bind(history);history[kind]=(...args)=>{original(...args);window.readerHistory.push({kind,url:location.href,state:history.state});};}addEventListener('popstate',()=>window.readerHistory.push({kind:'pop',url:location.href,state:history.state}));});
 await page.goto(`${browserOrigin}/`);await page.getByRole('heading',{name:'Your local repository'}).waitFor();
 if(width===320&&colorScheme==='light'){
  const homeText=await page.locator('[data-local-page]').innerText(),storagePath=homeText.match(/^Storage:\s*(.+)$/m)?.[1];assert(storagePath?.startsWith('/')&&/[\\/]store$/.test(storagePath),'owner-local storage path disclosure is missing');assert(homeText.includes('Only committed HEAD is analyzed'));
  await page.getByRole('button',{name:'Refresh from checkout HEAD'}).click();await page.getByRole('heading',{name:'Confirm analysis scope'}).waitFor();
  await page.getByRole('button',{name:'Index detected source'}).click();
  const saved=page.getByRole('link',{name:/Open/}).last();await saved.waitFor({timeout:120000});await saved.click();await page.locator('.map-svg').waitFor();
  await page.locator('[data-open-search]').click();await page.getByRole('combobox',{name:'Search districts, files and symbols'}).fill('cart.py');
  await page.getByRole('option').first().click();await page.getByRole('button',{name:'Read selected source'}).waitFor();await page.getByRole('button',{name:'Read selected source'}).tap();await page.locator('.cm-content').waitFor();
  assert(await page.locator('.cm-content').innerText().then(text=>text.includes('def ')),'native detect → confirm → index → map → read journey did not display committed source');
 }
 await page.goto(`${browserOrigin}/`);await page.getByRole('heading',{name:'Your local repository'}).waitFor();await page.screenshot({path:`../reader-evidence/local-${width}-${colorScheme}.png`,animations:'disabled'});
 const base=`http://127.0.0.1:8988/snapshot/${fixture.snapshotA}?file=${fixture.file}`;
 await page.goto(base);await page.locator('.map-svg').waitFor();await page.getByRole('button',{name:'Read selected source'}).waitFor();await settleCamera(page);
 const before=await cameraBoxes(page);
 const sourceAction=page.getByRole('button',{name:'Read selected source'});
 let phoneControls=[];
 if(narrow&&height>500){
  await page.locator('[data-phone-sheet][data-detent="peek"]').waitFor();phoneControls.push(await assertPhoneActionAndControls(page,width,height));assert.equal(phoneControls.at(-1).controlCount,4,'Peek should leave all four map controls visible');
  await page.locator('[data-sheet-grabber]').click();await page.locator('[data-phone-sheet][data-detent="half"]').waitFor();phoneControls.push(await assertPhoneActionAndControls(page,width,height));
  assert.equal(phoneControls.at(-1).controlCount,width===320?0:4,'map controls should hide only when the short Half sheet covers them');
  await page.goto(base);await page.locator('.map-svg').waitFor();await page.getByRole('button',{name:'Read selected source'}).waitFor();await settleCamera(page);
 }else if(width===844&&height===390){
  const sheet=page.locator('[data-side-sheet]'),toggle=page.locator('[data-side-toggle]');await sheet.waitFor();await toggle.waitFor();
  if(await sheet.getAttribute('data-side-open')==='true'){
   assert(await toggle.isVisible());const toggleBox=await toggle.boundingBox();assert(toggleBox&&await toggle.evaluate(el=>{const r=el.getBoundingClientRect();return el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2));}));
   await toggle.click();
  }
  await page.locator('[data-side-sheet][data-side-open="false"]').waitFor();
  await page.getByRole('button',{name:'Show the side sheet'}).click();await page.locator('[data-side-sheet][data-side-open="true"]').waitFor();
  phoneControls.push(await assertPhoneActionAndControls(page,width,height));assert.equal(phoneControls.at(-1).controlCount,5,'landscape side sheet should expose the four map controls and sheet toggle');
 }
 await page.screenshot({path:`../reader-evidence/map-source-action-${width}-${colorScheme}.png`,animations:'disabled'});
 if(narrow)await sourceAction.tap();else await sourceAction.click();
 await page.locator('.cm-content').waitFor();
 assert.equal(await page.locator('.cm-content').getAttribute('contenteditable'),'false');
 const original=await page.locator('.cm-content').innerText();
 await page.waitForFunction(()=>document.querySelector('.cm-content .cm-line span[class]'));
 const content=page.locator('.cm-content');await content.focus();const rangeContrast=await inspectReaderContrast(page);
 await page.keyboard.press('Control+Home');for(let i=0;i<4;i++)await page.keyboard.press('Shift+ArrowRight');
 const selectionMatch=page.locator('.cm-selectionMatch').first();await selectionMatch.waitFor();
 assert.deepEqual(await inspectDOMSelection(page),{from:0,to:4,text:'from'},'keyboard selection should cover offsets 0..4 and select "from"');
 await page.keyboard.press('Shift+ArrowLeft');
 assert.deepEqual(await inspectDOMSelection(page),{from:0,to:3,text:'fro'},'Shift+ArrowLeft should shrink the keyboard selection to offsets 0..3');
 await page.keyboard.press('Shift+ArrowRight');
 assert.deepEqual(await inspectDOMSelection(page),{from:0,to:4,text:'from'},'keyboard selection should restore the original range');
 await page.keyboard.press('ArrowLeft');
 assert.deepEqual(await inspectDOMSelection(page),{from:0,to:0,text:''},'ArrowLeft should collapse an existing selection to its start');
 for(let i=0;i<4;i++)await page.keyboard.press('Shift+ArrowRight');
 await page.keyboard.press('ArrowRight');
 assert.deepEqual(await inspectDOMSelection(page),{from:4,to:4,text:''},'ArrowRight should collapse an existing selection to its end');
 await page.keyboard.press('Control+Home');for(let i=0;i<4;i++)await page.keyboard.press('Shift+ArrowRight');
 assert.deepEqual(await inspectDOMSelection(page),{from:0,to:4,text:'from'},'keyboard selection should remain available after collapsing either edge');
 const selectionMatchColors=await matchStyle(selectionMatch);
 const selectionMatchContrast=contrast(selectionMatchColors.background,selectionMatchColors.color);assert(selectionMatchContrast>=4.5,JSON.stringify({selectionMatchColors,selectionMatchContrast}));
 await page.keyboard.type('cannot edit');
 assert.equal(await page.locator('.cm-content').innerText(),original);
 await page.keyboard.press('Control+a');await page.keyboard.press('Control+c');
 assert((await page.evaluate(()=>navigator.clipboard.readText())).includes('def '));
 await page.locator('.cm-content').evaluate(el=>{const data=new DataTransfer();data.setData('text/plain','mutated paste');el.dispatchEvent(new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true}));el.dispatchEvent(new DragEvent('drop',{dataTransfer:data,bubbles:true,cancelable:true}));});assert.equal(await page.locator('.cm-content').innerText(),original);
 await page.getByRole('button',{name:'Find in file'}).click();
 const geometryNegativeControl=await assertFindGeometryNegativeControl(page,width,height);
 const findTargets=await inspectFindTargets(page,width,height);
 const search=page.locator('.cm-search input[name=search]');await search.fill('def');await page.locator('.cm-search button[name=next]').click();
 assert(await page.locator('.cm-search').isVisible());const activeMatch=page.locator('.cm-searchMatch-selected').first();await activeMatch.waitFor();
 const activeMatchColors=await matchStyle(activeMatch);
 const otherMatchColors=await matchStyle(page.locator('.cm-searchMatch:not(.cm-searchMatch-selected)').first());
 assert.notDeepEqual(activeMatchColors,otherMatchColors,'current Find match must differ from other matches');
 assert.notDeepEqual(activeMatchColors,selectionMatchColors,'current Find match must differ from selected-word matches');
 assert.equal(activeMatchColors.outlineStyle,'solid');assert(parseFloat(activeMatchColors.outlineWidth)>=2);
 const activeMatchContrast=contrast(activeMatchColors.background,activeMatchColors.color);assert(activeMatchContrast>=4.5,JSON.stringify({activeMatchColors,activeMatchContrast}));
 const findScreenshot=`reader-find-${width}x${height}-${colorScheme}.png`;await page.screenshot({path:`../reader-evidence/${findScreenshot}`,animations:'disabled'});
 await search.fill('no_such_fixture_739');await page.locator('.cm-search button[name=next]').click();
 await page.getByText('No results',{exact:true}).waitFor();await page.getByRole('button',{name:'Find in file'}).focus();
 const afterTyping=await cameraBoxes(page);assert.deepEqual(afterTyping,before,'source typing changed map geometry');
 await page.keyboard.press('Escape');await page.waitForFunction(()=>!document.querySelector('.cm-search'));assert.equal(await page.locator('[data-source-reader]').count(),1,'Escape should close Find before reading');
 assert(await page.getByRole('button',{name:'Find in file'}).evaluate(el=>el===document.activeElement),'closing Find should focus its opener');
 await page.screenshot({path:`../reader-evidence/reader-${width}-${colorScheme}.png`,animations:'disabled'});
 await page.keyboard.press('Escape');await page.waitForFunction(()=>!document.querySelector('[data-source-reader]'));assert.equal(await page.locator('[data-source-reader]').count(),0,'second Escape should close reading');
 assert(await page.getByRole('button',{name:'Read selected source'}).evaluate(el=>el===document.activeElement));
 await settleCamera(page);assert.deepEqual(await cameraBoxes(page),before);
 await page.getByRole('button',{name:'Read selected source'}).click();await page.locator('.cm-content').waitFor();await page.getByRole('button',{name:'Return to map'}).click();assert.equal(await page.locator('[data-source-reader]').count(),0);assert(await page.getByRole('button',{name:'Read selected source'}).evaluate(el=>el===document.activeElement));
 await page.getByRole('button',{name:'Read selected source'}).click();await page.locator('.cm-content').waitFor();
 await page.goBack();await page.waitForFunction(()=>!document.querySelector('[data-source-reader]'));
 assert(new URL(page.url()).searchParams.get('file')===fixture.file);await settleCamera(page);assert.deepEqual(await cameraBoxes(page),before);
 await page.goto(base+'&read=true&line=2.5&end=4.5');await page.locator('.cm-content').waitFor();
 if(narrow)assert.equal(await page.evaluate(()=>history.length-window.readerInitialHistoryLength),2,'selection and reader must create distinct native history entries');
 await page.reload();await page.locator('.cm-content').waitFor();await page.waitForFunction(()=>document.querySelector('.cm-content .cm-line span[class]'));await page.locator('.cm-selectionBackground').first().waitFor();
 await page.locator('.cm-content').focus();const deepLinkedRange=await inspectReaderContrast(page);
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
 assert.deepEqual(errors,[]);assert.deepEqual(outbound,[],'private local journey attempted outbound request');
 results.push({width,height,colorScheme,rangeContrast,selectionMatchContrast,activeMatchContrast,matchStyles:{selection:selectionMatchColors,current:activeMatchColors,other:otherMatchColors},geometryNegativeControl,findTargets,findScreenshot,deepLinkedRange,phoneControls,images:[`local-${width}-${colorScheme}.png`,`map-source-action-${width}-${colorScheme}.png`,`reader-${width}-${colorScheme}.png`,findScreenshot],passed:true,reviewStatus:'unreviewed candidate',device:'Chromium emulation',browser:browser.version()});await context.close();
}
fs.renameSync('../.local-reader/checkout','../.local-reader/moved-checkout');
const movedPage=await browser.newPage();const afterCheckoutMove=await(await movedPage.request.get(`${browserOrigin}/api/maps/snapshot/${fixture.snapshotA}/source?commit=${fixture.a}&path=${encodeURIComponent(fixture.file)}`)).json();assert.equal(createHash('sha256').update(afterCheckoutMove.text).digest('hex'),fixture.sourceSha256,'saved A source changed after checkout move');await movedPage.close();
fs.writeFileSync('../reader-evidence/browser.json',JSON.stringify({sourceSha,baseSha,testedSha,runId:process.env.GITHUB_RUN_ID??null,fixture,results,nativeJourney:{steps:['local page','detect','confirm','index','saved map','search file','read source'],result:'passed on Chromium emulation',storagePathDisclosed:true},afterCheckoutMove:'saved A source remained readable after moving checkout',visualReview:'pending human; CI candidates are not approval',physicalDevice:'not run'},null,2));
}finally{await browser.close()}
