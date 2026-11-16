import type { ApprovalRequest, Command, ControlOption, Decision, Outcome, Snapshot } from '../../../protocol/v1';
import type { Language } from '../locales';
export const editable=(o:ControlOption)=>o.available && o.mobile_supported && o.scope==='Live';
export function canSubmit(s:Snapshot|null,generation=s?.generation):boolean {
 return !!s && !!generation && s.generation===generation && s.replay_ready && s.uncertain_deliveries.length===0 && !('unavailable'in s.controls) && !Object.values(s.controls.outcomes).some(o=>!o.settled);
}
export function outcomeLabel(o:Outcome,lang:Language):string {
 const ko:Record<Outcome['state'],string>={Pending:'적용 대기',Applied:'적용됨',Acknowledged:'수신 확인',Rejected:'거부됨',Cancelled:'취소됨',Unconfirmed:'적용 확인 필요'};
 return lang==='ko'?ko[o.state]:o.state;
}
export function responseCommand(r:ApprovalRequest,decision:Decision,answers:Record<string,string[]>):Command {
 if(!r.respondable)throw new Error('incomplete_approval');
 if(typeof decision==='object' && ('Grant'in decision || !r.options.some(o=>'Option'in decision && o.id===decision.Option)))throw new Error('unsupported_mobile_decision');
 const result:Record<string,string[]>={};
 for(const q of r.questions){const values=answers[q.id]??[];if((!q.multi_select && values.length>1)||values.some(v=>v.length>4096||(q.options.length>0&&!q.options.some(o=>o.id===v))))throw new Error('invalid_answer');result[q.id]=values;}
 if(Object.keys(answers).some(id=>!r.questions.some(q=>q.id===id)))throw new Error('invalid_question');
 return {Respond:{request_id:r.id,decision,answers:result}};
}
export class Drafts {
 private values=new Map<string,{text:string;revision:number}>();
 private revision=0;
 get(id:string){return this.values.get(id)?.text??'';}
 set(id:string,text:string){this.values.set(id,{text,revision:++this.revision});}
 snapshot(id:string){return this.values.get(id)??{text:'',revision:0};}
 consume(id:string,submitted:{text:string;revision:number}){
  if(this.snapshot(id).revision!==submitted.revision)return false;
  this.set(id,'');return true;
 }
}
export function displayValue(value:unknown):string {if(value===null || value===undefined)return '—';if(typeof value==='string')return value;if(typeof value==='object'){const v=value as Record<string,unknown>;if('Text'in v)return String(v.Text);if('Bool'in v)return String(v.Bool);if('Integer'in v)return String(v.Integer);if('Strings'in v)return (v.Strings as string[]).join(', ');}return JSON.stringify(value);}
