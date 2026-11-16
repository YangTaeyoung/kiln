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
it('keeps long native IDs and focused text fields within 360/402px with at least 16px input text',async()=>{
 const s=structuredClone(approval) as Snapshot;
 s.descriptor.id='native-conversation-'+ 'x'.repeat(180);
 s.models=[{id:'model-'+ 'm'.repeat(120),label:'Native model '+ 'm'.repeat(120)}];
 s.selected_model=s.models[0].id;
 const remote={snapshot:s,selected:s.descriptor.id,list:{conversations:[],truncated:false},drafts:new Drafts(),ledger:new MutationLedger(),connectivity:'verified',snapshotFresh:true} as unknown as Remote;
 const html=renderToStaticMarkup(<div className="app route-p2"><main><Conversation remote={remote} t={translator('en')} back={()=>{}} settings={()=>{}} discovery={()=>{}}/></main></div>);
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
});
