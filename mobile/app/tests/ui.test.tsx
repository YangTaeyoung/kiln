import { afterEach, describe,it,expect,vi } from 'vitest';
import { render,screen,fireEvent,cleanup,renderHook,act,waitFor } from '@testing-library/react';
import approvalFixture from '../../protocol/fixtures/approval.json';
import bootstrap from '../../protocol/fixtures/bootstrap.json';
import conversations from '../../protocol/fixtures/conversations.json';
import { Conversation } from '../src/components/Conversation';
import { Pairing } from '../src/components/Pairing';
import { Settings } from '../src/components/Settings';
import { translator } from '../src/locales';
import { Drafts } from '../src/state/native';
import { MutationLedger } from '../src/state/mutations';
import { useRemote,type Remote } from '../src/state/useRemote';
import type { Snapshot } from '../../protocol/v1';
const fixture=()=>structuredClone(approvalFixture) as Snapshot;
function remote(s:Snapshot):Remote{return {snapshot:s,selected:s.descriptor.id,list:conversations,ledger:new MutationLedger(),drafts:new Drafts(),command:vi.fn(),upload:vi.fn()} as unknown as Remote;}
afterEach(()=>{cleanup();vi.unstubAllGlobals();});
describe('safe native output and explicit UI intent',()=>{
 it('renders original hostile tool/message/diff strings as inert text',()=>{const s=fixture();const attack='<img src=x onerror="window.pwned=true"><script>alert(1)</script>';s.entries=[{Message:{id:'m',role:'Assistant',text:attack,attachments:[]}},{Tool:{id:'t',name:'Read',input:{file_path:attack},input_truncated:false,output:attack,diff:attack,status:'Completed',presentation:null}}];const {container}=render(<Conversation remote={remote(s)} t={translator('ko')} back={()=>{}} settings={()=>{}} discovery={()=>{}}/>);expect(container.querySelector('script')).toBeNull();expect(container.querySelector('img[src=x]')).toBeNull();expect(container.textContent).toContain(attack);expect(screen.getByText('파일 읽기')).toBeTruthy();expect(container.textContent).toContain('Read');});
 it('all responses including Deny stay disabled for incomplete approval',()=>{const s=fixture();s.requests[0].respondable=false;s.requests[0].reason='partial';render(<Conversation remote={remote(s)} t={translator('en')} back={()=>{}} settings={()=>{}} discovery={()=>{}}/>);expect((screen.getByRole('button',{name:/Allow once/}) as HTMLButtonElement).disabled).toBe(true);expect((screen.getByRole('button',{name:/Deny/}) as HTMLButtonElement).disabled).toBe(true);});
 it('pairing token is masked by default and toggle reflects the real input type; typing makes no request',()=>{const pair=vi.fn();const {container}=render(<Pairing remote={{pair,loading:false,bootstrap:null} as unknown as Remote} t={translator('en')} back={()=>{}}/>);const token=screen.getByLabelText('Connection token') as HTMLInputElement;expect(token.type).toBe('password');fireEvent.change(token,{target:{value:'private-token'}});expect(pair).not.toHaveBeenCalled();fireEvent.click(screen.getByRole('switch'));expect(token.type).toBe('text');expect(container.textContent).not.toContain('private-token');});
 it('unavailable and inherited controls remain visible without fake editable buttons',()=>{const s=fixture();if('unavailable'in s.controls)throw new Error();s.controls.options=[{id:'ultracode',label:'Ultracode',description:'native description',scope:'Inherited',kind:'Boolean',current:{Bool:true},choices:[],available:true,mobile_supported:false,mobile_reason:'native scope',reason:null,source:null,documentation:null,category:null}];render(<Settings remote={remote(s)} t={translator('en')} language="en" setLanguage={()=>{}} back={()=>{}} newTask={()=>{}}/>);expect(screen.getByText('Ultracode')).toBeTruthy();expect(screen.getByText('native scope')).toBeTruthy();expect(screen.queryByRole('switch',{name:'Ultracode'})).toBeNull();expect(screen.queryByText(/Sonnet 4/)).toBeNull();});
});
describe('host session state',()=>{
 it('bootstrap verification creates connectivity, disconnect clears native state and memory credential while retaining an unsent draft',async()=>{
 const fetcher=vi.fn(async(url:RequestInfo|URL,_init?:RequestInit)=>({status:200,json:async()=>String(url).endsWith('bootstrap')?bootstrap:String(url).endsWith('terminals')?{terminals:[],truncated:false}:String(url).endsWith('fixture-conversation')?fixture():conversations}));vi.stubGlobal('fetch',fetcher);
 const {result}=renderHook(()=>useRemote());result.current.drafts.set('fixture-conversation','보존할 초안');
 await act(async()=>{await result.current.pair('https://chosen.example','private-token');});expect(result.current.bootstrap?.host_name).toBe('Kiln computer');
 act(()=>result.current.select('fixture-conversation'));await waitFor(()=>expect(result.current.snapshot?.generation).toBe('fixture-generation'));
 act(()=>result.current.disconnect());expect(result.current.bootstrap).toBeNull();expect(result.current.snapshot).toBeNull();expect(result.current.drafts.get('fixture-conversation')).toBe('보존할 초안');expect(fetcher.mock.calls.some(call=>(call[1] as RequestInit|undefined)?.method==='POST')).toBe(false);
 });
 it('failed authentication never marks the computer connected',async()=>{vi.stubGlobal('fetch',vi.fn(async()=>({status:401,json:async()=>({error:{code:'unauthorized'}})})));const {result}=renderHook(()=>useRemote());await act(async()=>{await result.current.pair('https://chosen.example','bad-token');});expect(result.current.bootstrap).toBeNull();expect(result.current.error).toBe('unauthorized');});
});
