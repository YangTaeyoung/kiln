// @vitest-environment node
import {expect,it} from 'vitest';
import {readFileSync} from 'node:fs';
import {renderToStaticMarkup} from 'react-dom/server';
import {chromium} from 'playwright';
import approval from '../../protocol/fixtures/approval.json';
import type {Snapshot} from '../../protocol/v1';
import {Conversation} from '../src/components/Conversation';
import {Drafts} from '../src/state/native';
import {MutationLedger} from '../src/state/mutations';
import type {Remote} from '../src/state/useRemote';
import {translator} from '../src/locales';
function renderConversation(){
 const s=structuredClone(approval) as Snapshot;
 s.descriptor.id='native-conversation-'+ 'x'.repeat(180);
 s.models=[{id:'model-'+ 'm'.repeat(120),label:'Native model '+ 'm'.repeat(120)}];
 s.selected_model=s.models[0].id;
 const remote={snapshot:s,selected:s.descriptor.id,list:{conversations:[],truncated:false},drafts:new Drafts(),ledger:new MutationLedger(),connectivity:'verified',snapshotFresh:true} as unknown as Remote;
 return renderToStaticMarkup(<div className="app route-p2"><main><Conversation remote={remote} t={translator('en')} back={()=>{}} settings={()=>{}} discovery={()=>{}}/></main></div>);
}
it('keeps long native IDs and focused text fields within 360/402px with at least 16px input text',async()=>{
 const html=renderConversation();
 const css=readFileSync('src/styles/app.css','utf8');
 const browser=await chromium.launch({headless:true,channel:'chrome'});
 try{
  for(const width of [360,402]){
   const page=await browser.newPage({viewport:{width,height:874}});
   await page.setContent(`<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover"><style>${css}</style>${html}`);
   await page.getByRole('textbox',{name:'Message',exact:true}).fill('Unsent input '+ 'long-native-token-'.repeat(25));
   await page.getByRole('textbox',{name:'Message',exact:true}).blur();
   const result=await page.evaluate(()=>({width:innerWidth,scrollWidth:document.documentElement.scrollWidth,mainWidth:document.querySelector('main')!.getBoundingClientRect().width,fonts:Array.from(document.querySelectorAll('input,textarea,select')).map(e=>parseFloat(getComputedStyle(e).fontSize))}));
   expect(result.fonts.every(font=>font>=16),JSON.stringify({width,result})).toBe(true);
   expect(result.scrollWidth,JSON.stringify({width,result})).toBeLessThanOrEqual(width);
   expect(result.mainWidth).toBeLessThanOrEqual(width);
   await page.close();
  }
 }finally{await browser.close();}
},30_000);


it('keeps rendered heading, body and composer text readable in both system themes',async()=>{
 const html=renderConversation();
 const css=readFileSync('src/styles/app.css','utf8');
 const browser=await chromium.launch({headless:true,channel:'chrome'});
 try{
  for(const colorScheme of ['light','dark'] as const){
   const page=await browser.newPage({viewport:{width:402,height:874},colorScheme});
   await page.setContent(`<!doctype html><style>${css}</style>${html}`);
   await page.getByRole('textbox',{name:'Message',exact:true}).fill('Readable draft');
   const samples=await page.evaluate(()=>{
    const rgb=(value:string)=>value.match(/[\d.]+/g)!.map(Number);
    const luminance=(channels:number[])=>channels.slice(0,3).map(c=>{const v=c/255;return v<=0.04045?v/12.92:((v+0.055)/1.055)**2.4;}).reduce((sum,c,i)=>sum+c*[0.2126,0.7152,0.0722][i],0);
    return ['h1','body','.app','html','.message p','.composer textarea','.composer-controls select'].map(selector=>{
     const element=document.querySelector(selector)!;
     const foreground=getComputedStyle(element).color;
     let ancestor:Element|null=element;
     let background='';
     while(ancestor){
      const candidate=getComputedStyle(ancestor).backgroundColor;
      const channels=rgb(candidate);
      if(channels.length===3||channels[3]===1){background=candidate;break;}
      if(channels[3]!==0)throw new Error(`Unexpected translucent background for ${selector}: ${candidate}`);
      ancestor=ancestor.parentElement;
     }
     if(!background)throw new Error(`No opaque background for ${selector}`);
     const a=luminance(rgb(foreground)),b=luminance(rgb(background));
     return {selector,foreground,background,contrast:(Math.max(a,b)+0.05)/(Math.min(a,b)+0.05)};
    });
   });
   for(const sample of samples){
    expect(sample.contrast,JSON.stringify({colorScheme,sample})).toBeGreaterThanOrEqual(4.5);
    expect(sample.foreground,JSON.stringify({colorScheme,sample})).toBe(colorScheme==='dark'?'rgb(228, 238, 235)':'rgb(19, 39, 46)');
   }
   await page.close();
  }
 }finally{await browser.close();}
},30_000);
