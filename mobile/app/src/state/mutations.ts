import type { Operation } from '../../../protocol/v1';
import { ApiFailure } from '../api/client';
export interface MutationRecord {requestId:string;key:string;state:'pending'|'queued'|'succeeded'|'failed'|'unknown';commandId?:string;error?:string;created:number}
/** One user intent, one POST. Retains IDs and ambiguity for read-only recovery. */
export class MutationLedger {
 records:MutationRecord[]=[];
 private active=new Map<string,Promise<unknown>>();
 constructor(private changed:()=>void=()=>{}){}
 perform<T>(key:string,post:(id:string)=>Promise<T>):Promise<T|undefined> {
  const existing=this.active.get(key);
  if(existing) return existing as Promise<T>;
  if(this.records.some(r=>r.key===key && (r.state==='unknown'||r.state==='pending'))) return Promise.resolve(undefined);
  const record:MutationRecord={requestId:crypto.randomUUID(),key,state:'pending',created:Date.now()};
  this.records.push(record);
  // Reserve the key synchronously, including if post throws before returning a Promise.
  let finish!:(v:T|undefined)=>void;
  const result=new Promise<T|undefined>(resolve=>{finish=resolve;});
  this.active.set(key,result);this.changed();
  void (async()=>{
   try {
    const value=await post(record.requestId);
    if(value && typeof value==='object' && 'command_id'in value){record.commandId=String(value.command_id);record.state='queued';}
    else if(value && typeof value==='object' && 'state'in value && value.state==='pending')record.state='pending';
    else record.state='succeeded';
    finish(value);
   } catch(error){
    const definitive=error instanceof ApiFailure && error.status!==408 && error.status<500;
    record.state=definitive?'failed':'unknown';record.error=error instanceof ApiFailure?error.code:'delivery_unknown';finish(undefined);
   } finally {this.active.delete(key);this.changed();}
  })();
  return result;
 }
 recover(id:string,operation:Operation){
  const record=this.records.find(r=>r.requestId===id);if(!record)return;
  if(operation.state==='succeeded'){
   record.state=operation.result && 'delivery'in operation.result?'queued':'succeeded';
   if(operation.result && 'command_id'in operation.result)record.commandId=operation.result.command_id;
  }else record.state=operation.state;
  record.error=operation.error?.code;this.changed();
 }
 expire(id:string){const r=this.records.find(r=>r.requestId===id);if(r){r.state='unknown';r.error='operation_expired';this.changed();}}
 busy(key:string){return this.active.has(key)||this.records.some(r=>r.key===key && ['pending','unknown'].includes(r.state));}
}
