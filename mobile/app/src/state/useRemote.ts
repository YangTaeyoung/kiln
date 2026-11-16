import { useCallback, useEffect, useRef, useState } from 'react';
import type { Bootstrap, Command, CommandReceipt, ConversationList, CreateConversation, LaunchOptions, Operation, PendingReceipt, Provider, Snapshot, Terminal, TerminalList, UploadReceipt } from '../../../protocol/v1';
import { RemoteClient, ApiFailure } from '../api/client';
import { Drafts, canSubmit } from './native';
import { MutationLedger } from './mutations';
export function useRemote(){
 const [client,setClient]=useState<RemoteClient|null>(null),[bootstrap,setBootstrap]=useState<Bootstrap|null>(null),[list,setList]=useState<ConversationList|null>(null),[selected,setSelected]=useState<string|null>(null),[snapshot,setSnapshot]=useState<Snapshot|null>(null),[terminals,setTerminals]=useState<TerminalList|null>(null),[terminal,setTerminal]=useState<Terminal|null>(null),[error,setError]=useState<string|null>(null),[loading,setLoading]=useState(false),[revision,changed]=useState(0),[createdId,setCreatedId]=useState<string|null>(null),[connectivity,setConnectivity]=useState<'disconnected'|'verified'|'unverified'>('disconnected'),[verifiedAt,setVerifiedAt]=useState<number|null>(null),[snapshotFresh,setSnapshotFresh]=useState(false);
 const drafts=useRef(new Drafts()).current;
 const ledgerRef=useRef<MutationLedger|null>(null);if(!ledgerRef.current)ledgerRef.current=new MutationLedger(()=>changed(n=>n+1));const ledger=ledgerRef.current;
 const createIntents=useRef(new Map<string,{draft:ReturnType<Drafts['snapshot']>;epoch:number}>());
 const epoch=useRef(0),pairing=useRef(false),currentSnapshot=useRef(snapshot);currentSnapshot.current=snapshot;const selectedRef=useRef(selected);selectedRef.current=selected;const terminalRef=useRef(terminal);terminalRef.current=terminal;
 const refresh=useCallback(async()=>{
  if(!client)return;
  const e=epoch.current;
  try{
   const [b,l,ts]=await Promise.all([client.get<Bootstrap>('/v1/bootstrap'),client.get<ConversationList>('/v1/conversations'),client.get<TerminalList>('/v1/terminals')]);
   if(b.api_version!==1||b.host_version!=='0.1.0')throw new Error('unsupported_host');
   if(e===epoch.current){setBootstrap(b);setList(l);setTerminals(ts);setConnectivity('verified');setVerifiedAt(Date.now());setError(null);}
  }catch(err){if(e===epoch.current){setConnectivity('unverified');setSnapshotFresh(false);setError(err instanceof ApiFailure?err.code:'connection_lost');}}
 },[client]);
 const readSnapshot=useCallback(async()=>{
  if(!client||!selected)return;
  const e=epoch.current;
  try{
   const s=await client.get<Snapshot>(`/v1/conversations/${encodeURIComponent(selected)}`);
   if(e===epoch.current&&selectedRef.current===s.descriptor.id){setSnapshot(s);setSnapshotFresh(true);setError(null);}
  }catch(err){if(e===epoch.current&&selectedRef.current===selected){setConnectivity('unverified');setSnapshotFresh(false);setError(err instanceof ApiFailure?err.code:'connection_lost');}}
 },[client,selected]);
 useEffect(()=>{void refresh();const id=setInterval(()=>void refresh(),4000);return()=>clearInterval(id);},[refresh]);
 useEffect(()=>{setSnapshot(null);void readSnapshot();const id=setInterval(()=>void readSnapshot(),1500);return()=>clearInterval(id);},[readSnapshot]);
 async function pair(address:string,token:string){if(pairing.current)return;pairing.current=true;setLoading(true);setError(null);setConnectivity('unverified');setSnapshotFresh(false);const e=++epoch.current;let next:RemoteClient|undefined;try{next=new RemoteClient(address,token);const b=await next.get<Bootstrap>('/v1/bootstrap');if(b.api_version!==1 || b.host_version!=='0.1.0')throw new Error('unsupported_host');const l=await next.get<ConversationList>('/v1/conversations');if(e===epoch.current){client?.clear();setClient(next);setBootstrap(b);setList(l);setConnectivity('verified');setVerifiedAt(Date.now());setSnapshot(null);setSelected(null);setTerminal(null);setTerminals(null);}}catch(err){next?.clear();if(e===epoch.current)setError(err instanceof ApiFailure?err.code:'connection_failed');}finally{pairing.current=false;setLoading(false);}}
 function disconnect(){++epoch.current;setConnectivity('disconnected');setVerifiedAt(null);setSnapshotFresh(false);client?.clear();setClient(null);setBootstrap(null);setList(null);setSelected(null);setSnapshot(null);setTerminal(null);setTerminals(null);setError(null);setCreatedId(null);}
 function select(id:string){setSnapshotFresh(false);setSnapshot(null);setSelected(id);}
 async function command(command:Command,key:string){const s=currentSnapshot.current;if(!client||!s||connectivity!=='verified'||!snapshotFresh)return;const generation=s.generation;const e=epoch.current;
  const settlementRequired=typeof command==='object'&&('Submit'in command||'SubmitWithSkills'in command||'Configure'in command||'SetMode'in command);
  if(command!=='Interrupt' && (!s.replay_ready || (settlementRequired&&!canSubmit(s,generation)))){setError('native_not_ready');return;}
  const receipt=await ledger.perform(key,id=>{if(currentSnapshot.current?.generation!==generation)throw new ApiFailure('stale_generation',409);return client.post<CommandReceipt>(`/v1/conversations/${encodeURIComponent(s.descriptor.id)}/commands`,{request_id:id,generation,command});});
  if(e!==epoch.current)return;void readSnapshot();return receipt;
 }
 async function recover(requestId:string){
  if(!client)return;
  const e=epoch.current;
  try{
   const op=await client.get<Operation>(`/v1/operations/${requestId}`);
   if(e!==epoch.current)return;
   ledger.recover(requestId,op);
   const intent=createIntents.current.get(requestId);
   if(op.state==='succeeded'&&op.result&&'conversation_id'in op.result&&intent?.epoch===e){
    const id=op.result.conversation_id;
    if(drafts.snapshot(id).revision===0){
     drafts.set(id,intent.draft.text);
     drafts.consume('new-task',intent.draft);
    }
    createIntents.current.delete(requestId);
    select(id);setCreatedId(id);void refresh();
   }
   return op;
  }catch(err){if(e!==epoch.current)return;if(err instanceof ApiFailure&&err.status===404)ledger.expire(requestId);else{setConnectivity('unverified');setSnapshotFresh(false);setError('connection_lost');}}
 }
 // Pending task creation is recovered by safe GET only. No POST is repeated.
 useEffect(()=>{if(!client)return;const pending=ledger.records.filter(r=>r.key==='create' && r.state==='pending');if(!pending.length)return;const id=setTimeout(()=>{for(const r of pending)void recover(r.requestId);},1500);return()=>clearTimeout(id);},[client,revision]);
 async function create(workspace_id:string,provider:Provider,launch_options:LaunchOptions){
  if(!client||connectivity!=='verified')return;
  const e=epoch.current,submitted=drafts.snapshot('new-task');
  const receipt=await ledger.perform('create',id=>{
   createIntents.current.set(id,{draft:submitted,epoch:e});
   return client.post<PendingReceipt>('/v1/conversations',{request_id:id,workspace_id,provider,launch_options} satisfies CreateConversation);
  });
  if(receipt&&e===epoch.current)void recover(receipt.request_id);
 }
 async function upload(file:File){if(!client||!selected)return;if(file.size>20*1024*1024 || !['image/png','image/jpeg','image/gif','image/webp'].includes(file.type)){setError('invalid_image');return;}
  const encoded=await new Promise<string>((resolve,reject)=>{const r=new FileReader();r.onload=()=>resolve(String(r.result).split(',')[1]);r.onerror=reject;r.readAsDataURL(file);});
  return ledger.perform(`upload-${selected}`,id=>client.post<UploadReceipt>(`/v1/conversations/${encodeURIComponent(selected)}/attachments`,{request_id:id,filename:file.name,mime:file.type,bytes_base64:encoded}));
 }
 async function readTerminal(id:string){if(!client)return;const e=epoch.current;try{const value=await client.get<Terminal>(`/v1/terminals/${encodeURIComponent(id)}`);if(e===epoch.current)setTerminal(value);}catch{if(e===epoch.current)setError('terminal_unavailable');}}
 async function terminalInput(text:string,enter:boolean){
  if(!client||!terminal||connectivity!=='verified')return;
  const e=epoch.current;
  const receipt=await ledger.perform(`terminal-${terminal.id}`,id=>client.post(`/v1/terminals/${encodeURIComponent(terminal.id)}/input`,{request_id:id,text,enter}));
  return e===epoch.current&&terminalRef.current?.id===terminal.id?receipt:undefined;
 }
 return {sessionId:epoch.current,isCurrentSession:(id:number)=>id===epoch.current,connectivity,verifiedAt,snapshotFresh,createdId,bootstrap,list,selected,snapshot,terminals,terminal,error,loading,ledger,drafts,revision,pair,disconnect,select,refresh,readSnapshot,command,recover,create,upload,readTerminal,terminalInput};
}
export type Remote=ReturnType<typeof useRemote>;
