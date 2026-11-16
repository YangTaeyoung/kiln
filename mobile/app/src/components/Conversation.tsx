import { useRef, useState } from 'react';
import { Paperclip, Send, Square, SlidersHorizontal, BookOpen } from 'lucide-react';
import type { NativeSkillInput } from '../../../protocol/v1';
import type { Remote } from '../state/useRemote';
import type { Translate } from '../locales';
import { providerNames } from '../locales';
import { canSubmit } from '../state/native';
import { Approval } from './Approvals';
import { PageHeader, ProviderIcon } from './Common';
export function Conversation({remote:r,t,back,settings,discovery}:{remote:Remote;t:Translate;back:()=>void;settings:()=>void;discovery:()=>void}){
 const s=r.snapshot;const [,rerender]=useState(0),[model,setModel]=useState<string|null>(null);const input=useRef<HTMLInputElement>(null);
 if(!s)return <><PageHeader title={t('tasks')} back={back} t={t}/><p>{t('loading')}</p></>;
 const title=r.list?.conversations.find(c=>c.descriptor.id===s.descriptor.id)?.title??s.descriptor.id;
 const attachments=r.drafts.imageSnapshot(s.descriptor.id);
 const unavailableImages=attachments.some(image=>image.hostScope!==r.hostScope);
 const text=r.drafts.get(s.descriptor.id);const ready=canSubmit(s)&&r.connectivity==='verified'&&r.snapshotFresh;const skillKey=`skills-${s.descriptor.id}`;let skills:NativeSkillInput[]=[];try{skills=JSON.parse(r.drafts.get(skillKey)||'[]');}catch{/* no auto execution */}
 const send=async()=>{
  const submitted=r.drafts.snapshot(s.descriptor.id);
  const submittedSkills=r.drafts.snapshot(skillKey);
  const session=r.sessionId;
  const submittedAttachments=r.drafts.imageSnapshot(s.descriptor.id);
  if(submittedAttachments.some(image=>image.hostScope!==r.hostScope))return;
  if(!text.trim()&&!attachments.length)return;
  if(new TextEncoder().encode(text).length>120000)return;
  const payload={text,attachments:attachments.map(a=>a.id),model};
  const receipt=await r.command(skills.length?{SubmitWithSkills:{...payload,skills}}:{Submit:payload},`send-${s.descriptor.id}`);
  if(!receipt||!r.isCurrentSession(session))return;
  r.drafts.consume(s.descriptor.id,submitted);
  r.drafts.consume(skillKey,submittedSkills);
  r.drafts.consumeImages(s.descriptor.id,submittedAttachments);
  rerender(n=>n+1);
 };
 return <div className="conversation"><PageHeader title={title} back={back} t={t}><button className="icon-button" aria-label={t('providerSettings')} onClick={settings}><SlidersHorizontal/></button></PageHeader><div className="conversation-meta"><ProviderIcon provider={s.descriptor.provider}/><strong>{providerNames[s.descriptor.provider]}</strong><span className="badge">{s.lifecycle}</span></div>{!s.replay_ready&&<p className="notice">{t('catchingUp')}</p>}{s.coverage.truncated&&<p className="warning">{t('partial')} · {s.coverage.entries_retained}</p>}{s.errors.map((e,i)=><p key={i} className="warning">{e}</p>)}<section className="transcript" aria-label={t('message')}>{s.entries.map(entry=>'Message'in entry?<article className={`message ${entry.Message.role.toLowerCase()}`} key={entry.Message.id}><small>{entry.Message.role==='Assistant'?providerNames[s.descriptor.provider]:entry.Message.role}</small><p>{entry.Message.text}</p>{entry.Message.attachments.map(a=><p className="attachment" key={a.id}>▧ {a.name} · {a.mime}</p>)}</article>:<article key={entry.Tool.id} className="tool card"><h3>{({Read:t('read'),Write:t('write'),Edit:t('edit'),Bash:t('shell')} as Record<string,string>)[entry.Tool.name]??t('tools')} <span className="badge">{entry.Tool.status}</span></h3><p>{entry.Tool.presentation?.title}</p><details><summary>{t('original')} · {entry.Tool.name}</summary><p>{t('originalTool')}: {entry.Tool.name}</p><h4>{t('input')}</h4><pre>{JSON.stringify(entry.Tool.input,null,2)}</pre>{entry.Tool.input_truncated&&<p className="warning">{t('partial')}</p>}<h4>{t('output')}</h4><pre>{entry.Tool.output}</pre>{entry.Tool.diff&&<><h4>{t('diff')}</h4><pre>{entry.Tool.diff}</pre></>}</details></article>)}{s.requests.map(q=><Approval request={q} remote={r} t={t} key={`${s.generation}-${q.id}`}/>)}</section><div className="composer-dock">{attachments.length>0&&<div className="attachment-list">{attachments.map(a=><button key={a.revision} onClick={()=>{r.drafts.removeImage(s.descriptor.id,a.revision);rerender(n=>n+1);}} aria-label={`${t('remove')} ${a.name}`}>{a.name}{a.hostScope!==r.hostScope?` · ${t('unavailable')}`:''} ×</button>)}</div>}{unavailableImages&&<p className="warning" role="status">{t('imageSessionChanged')}</p>}{skills.length>0&&<p className="hint">{t('selectedSkills')}: {skills.map(s=>s.name).join(', ')} <button onClick={()=>{r.drafts.set(skillKey,'');rerender(n=>n+1);}}>{t('remove')}</button></p>}<form className="composer" onSubmit={e=>{e.preventDefault();void send();}}><input ref={input} type="file" accept="image/png,image/jpeg,image/gif,image/webp" hidden onChange={e=>{const file=e.currentTarget.files?.[0];if(file)void r.upload(file).then(()=>rerender(n=>n+1));e.currentTarget.value='';}}/><button type="button" className="icon-button" aria-label={t('image')} disabled={r.ledger.busy(`upload-${s.descriptor.id}`)} onClick={()=>input.current?.click()}><Paperclip/></button><textarea aria-label={t('message')} value={text} placeholder={t('message')} rows={2} onChange={e=>{r.drafts.set(s.descriptor.id,e.target.value);rerender(n=>n+1);}}/><button className="icon-button send" aria-label={t('send')} disabled={!ready||unavailableImages||r.ledger.busy(`send-${s.descriptor.id}`)||(!text.trim()&&!attachments.length)||new TextEncoder().encode(text).length>120000}><Send/></button></form><div className="composer-controls"><label>{t('model')}<select aria-label={`${t('model')} · ${t('send')}`} value={model??''} onChange={e=>setModel(e.target.value||null)}><option value="">{s.selected_model??t('inherited')}</option>{s.models.map(m=><option key={m.id} value={m.id}>{m.label}</option>)}</select></label><button onClick={settings}>{t('effort')}<span>⌄</span></button><button onClick={settings}>{t('ultra')}<span>⌄</span></button><button onClick={settings}>{t('mode')}<span>{s.selected_mode??'—'}</span></button></div><div className="row"><button className="quiet" onClick={discovery}><BookOpen size={16}/>{t('discover')}</button><button className="quiet" disabled={!s.generation||r.ledger.busy(`interrupt-${s.descriptor.id}`)} onClick={()=>void r.command('Interrupt',`interrupt-${s.descriptor.id}`)}><Square size={14}/>{t('interrupt')}</button></div></div></div>;
}
