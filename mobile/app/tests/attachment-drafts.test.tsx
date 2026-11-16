import {afterEach, expect, it, vi} from 'vitest';
import {act, cleanup, fireEvent, render, screen, waitFor} from '@testing-library/react';
import App from '../src/App';
import approval from '../../protocol/fixtures/approval.json';
import bootstrap from '../../protocol/fixtures/bootstrap.json';
import conversations from '../../protocol/fixtures/conversations.json';

function deferred<T>() { let resolve!: (value:T)=>void; const promise=new Promise<T>(r=>resolve=r); return {promise,resolve}; }
function host() {
 const uploads: Array<{url:string;body:any}> = [], sends: any[] = [];
 let uploadDelay: ReturnType<typeof deferred<any>> | undefined;
 let sendDelay: ReturnType<typeof deferred<any>> | undefined;
 const response=(body:unknown)=>({status:200,json:async()=>body});
 const fetcher=vi.fn(async(url:RequestInfo|URL,init?:RequestInit)=>{
  const route=String(url);
  if(init?.method==='POST') {
   const body=JSON.parse(String(init.body));
   if(route.endsWith('/attachments')) {
    uploads.push({url:route,body});
    const result=response({attachment:{id:body.filename,name:body.filename,mime:body.mime,sha256:'fixture'}});
    return uploadDelay ? uploadDelay.promise : result;
   }
   sends.push(body);
   return sendDelay ? sendDelay.promise : response({command_id:'queued',delivery:'queued'});
  }
  return response(route.endsWith('/bootstrap')?bootstrap:route.endsWith('/terminals')?{terminals:[],truncated:false}:route.endsWith('/fixture-conversation')?structuredClone(approval):route.endsWith('/fixture-unavailable')?{...structuredClone(approval),descriptor:{...approval.descriptor,id:'fixture-unavailable'}}:conversations);
 });
 vi.stubGlobal('fetch',fetcher);
 return {uploads,sends,response,delayUpload:()=>uploadDelay=deferred(),delaySend:()=>sendDelay=deferred()};
}
async function pair(address='https://host.invalid',token='fixture-token') {
 fireEvent.change(screen.getByLabelText('Computer address'),{target:{value:address}});
 fireEvent.change(screen.getByLabelText('Connection token'),{target:{value:token}});
 fireEvent.click(screen.getByRole('button',{name:'Request connection'}));
 await waitFor(()=>expect(screen.getByRole('button',{name:'Disconnect'})).toBeTruthy());
 fireEvent.click(screen.getAllByRole('button',{name:'Tasks'})[0]);
 fireEvent.click(await screen.findByRole('button',{name:/Review the example changes/}));
 await screen.findByRole('textbox',{name:'Message'});
}
async function start() {location.hash='p1'; const view=render(<App/>); await pair(); return view;}
async function attach(container:HTMLElement,name='A.png') {
 fireEvent.change(container.querySelector('input[type=file]')!,{target:{files:[new File(['image'],name,{type:'image/png'})]}});
 await screen.findByRole('button',{name:`Remove ${name}`});
}
async function disconnect() {
 fireEvent.click(screen.getByRole('button',{name:'Back'}));
 fireEvent.click(screen.getByRole('button',{name:'Computer'}));
 fireEvent.click(screen.getByRole('button',{name:'Disconnect'}));
 await screen.findByRole('button',{name:'Request connection'});
}
afterEach(()=>{cleanup();vi.unstubAllGlobals();location.hash='';});

it.each(['settings','discovery','list'])('uploaded image survives real %s navigation and is included once on explicit Send',async route=>{
 const h=host(),{container}=await start();
 fireEvent.change(screen.getByRole('textbox',{name:'Message'}),{target:{value:'preserved text'}});
 await attach(container);
 if(route==='settings') {fireEvent.click(screen.getByRole('button',{name:'Provider settings'}));fireEvent.click(screen.getByRole('button',{name:'Back'}));}
 if(route==='discovery') {fireEvent.click(screen.getByRole('button',{name:'Commands and skills'}));fireEvent.click(screen.getByRole('button',{name:'Back'}));}
 if(route==='list') {fireEvent.click(screen.getByRole('button',{name:'Back'}));fireEvent.click(screen.getByRole('button',{name:/Review the example changes/}));}
 await screen.findByRole('button',{name:'Remove A.png'});
 expect(h.sends).toHaveLength(0);expect(h.uploads).toHaveLength(1);
 fireEvent.click(screen.getByRole('button',{name:'Send'}));
 await waitFor(()=>expect(h.sends).toHaveLength(1));
 expect(h.sends[0].command.Submit).toMatchObject({text:'preserved text',attachments:['A.png']});
 await waitFor(()=>expect(screen.queryByRole('button',{name:'Remove A.png'})).toBeNull());
});
it('same live host re-pair preserves uploaded image without reupload or automatic Send',async()=>{
 const h=host(),{container}=await start();await attach(container);await disconnect();await pair();
 expect(screen.getByRole('button',{name:'Remove A.png'})).toBeTruthy();expect(h.uploads).toHaveLength(1);expect(h.sends).toHaveLength(0);
 fireEvent.click(screen.getByRole('button',{name:'Send'}));await waitFor(()=>expect(h.sends).toHaveLength(1));
 expect(h.sends[0].command.Submit.attachments).toEqual(['A.png']);
});
it.each([['https://other.invalid','fixture-token'],['https://host.invalid','replacement-token']])('changed host/session %s %s shows unavailable image and requires explicit removal/reselection',async(address,token)=>{
 const h=host(),{container}=await start();await attach(container);await disconnect();await pair(address,token);
 expect(screen.getByText(/Image belongs to another host session/)).toBeTruthy();
 expect((screen.getByRole('button',{name:'Send'}) as HTMLButtonElement).disabled).toBe(true);
 expect(h.uploads).toHaveLength(1);expect(h.sends).toHaveLength(0);
 fireEvent.click(screen.getByRole('button',{name:'Remove A.png'}));await attach(container,'B.png');
 fireEvent.click(screen.getByRole('button',{name:'Send'}));await waitFor(()=>expect(h.sends).toHaveLength(1));
 expect(h.sends[0].command.Submit.attachments).toEqual(['B.png']);expect(h.uploads).toHaveLength(2);
});
it('late upload after old route/session is retained only in its origin and cannot send an old opaque ID to the new host',async()=>{
 const h=host(),{container}=await start(),pending=h.delayUpload();
 fireEvent.change(container.querySelector('input[type=file]')!,{target:{files:[new File(['a'],'late.png',{type:'image/png'})]}});
 await waitFor(()=>expect(h.uploads).toHaveLength(1));await disconnect();await pair('https://other.invalid','other-token');
 await act(async()=>pending.resolve(h.response({attachment:{id:'old-opaque',name:'late.png',mime:'image/png',sha256:'fixture'}})));
 expect(screen.getByRole('button',{name:'Remove late.png'})).toBeTruthy();
 expect(screen.getByText(/Image belongs to another host session/)).toBeTruthy();
 expect((screen.getByRole('button',{name:'Send'}) as HTMLButtonElement).disabled).toBe(true);
 expect(h.uploads).toHaveLength(1);expect(h.sends).toHaveLength(0);
});
it('late Send A across navigation consumes exact revisions while newer B text and same-ID reupload survive',async()=>{
 const h=host(),{container}=await start();await attach(container);
 fireEvent.change(screen.getByRole('textbox',{name:'Message'}),{target:{value:'A'}});
 const pending=h.delaySend();fireEvent.click(screen.getByRole('button',{name:'Send'}));
 await waitFor(()=>expect(h.sends).toHaveLength(1));
 fireEvent.click(screen.getByRole('button',{name:'Provider settings'}));fireEvent.click(screen.getByRole('button',{name:'Back'}));
 fireEvent.change(screen.getByRole('textbox',{name:'Message'}),{target:{value:'B'}});
 fireEvent.click(screen.getByRole('button',{name:'Remove A.png'}));await attach(container);
 await act(async()=>pending.resolve(h.response({command_id:'queued',delivery:'queued'})));
 expect((screen.getByRole('textbox',{name:'Message'}) as HTMLTextAreaElement).value).toBe('B');
 expect(screen.getByRole('button',{name:'Remove A.png'})).toBeTruthy();expect(h.sends).toHaveLength(1);expect(h.uploads).toHaveLength(2);
});

it('late upload to conversation A never appears in B and survives returning to A',async()=>{
 const h=host(),{container}=await start(),pending=h.delayUpload();
 fireEvent.change(container.querySelector('input[type=file]')!,{target:{files:[new File(['a'],'late.png',{type:'image/png'})]}});
 await waitFor(()=>expect(h.uploads).toHaveLength(1));
 fireEvent.click(screen.getByRole('button',{name:'Back'}));
 fireEvent.click(screen.getByRole('button',{name:/fixture-unavailable/}));
 await screen.findByRole('textbox',{name:'Message'});
 await act(async()=>pending.resolve(h.response({attachment:{id:'A-only',name:'late.png',mime:'image/png',sha256:'fixture'}})));
 expect(screen.queryByRole('button',{name:'Remove late.png'})).toBeNull();
 fireEvent.click(screen.getByRole('button',{name:'Back'}));
 fireEvent.click(screen.getByRole('button',{name:/Review the example changes/}));
 await screen.findByRole('button',{name:'Remove late.png'});
 expect(h.uploads).toHaveLength(1);expect(h.sends).toHaveLength(0);
});
it('missing secure digest support fails pairing closed without a mutation',async()=>{
 const h=host();vi.stubGlobal('crypto',{randomUUID:()=> 'fixture-id'});location.hash='p1';render(<App/>);
 fireEvent.change(screen.getByLabelText('Computer address'),{target:{value:'https://host.invalid'}});
 fireEvent.change(screen.getByLabelText('Connection token'),{target:{value:'fixture-token'}});
 fireEvent.click(screen.getByRole('button',{name:'Request connection'}));
 await screen.findByText('connection_failed');
 expect(screen.queryByRole('button',{name:'Disconnect'})).toBeNull();expect(h.sends).toHaveLength(0);expect(h.uploads).toHaveLength(0);
});
