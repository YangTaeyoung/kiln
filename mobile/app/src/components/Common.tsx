import { ArrowLeft,ChevronRight, Laptop, List, Settings, Plus, Folder, RefreshCw } from 'lucide-react';
import type { Provider } from '../../../protocol/v1';
import type { Translate } from '../locales';
export type Page='p0'|'p1'|'p2'|'p3'|'p4';
export function ProviderIcon({provider}:{provider:Provider}){return <span className={`provider-icon ${provider}`}><img src={`/assets/${provider}.svg`} alt=""/></span>;}
export function PageHeader({title,back,children,t}:{title:string;back?:()=>void;children?:React.ReactNode;t:Translate}){return <header className="page-header">{back&&<button className="icon-button" onClick={back} aria-label={t('back')}><ArrowLeft/></button>}<h1>{title}</h1>{children}</header>;}
export function BottomNav({page,navigate,t}:{page:Page;navigate:(p:Page)=>void;t:Translate}){return <nav className="bottom-nav" aria-label="Kiln Remote">{([['p0','tasks',List],['p1','computer',Laptop],['p3','settings',Settings]] as const).map(([p,key,Icon])=><button key={p} aria-current={page===p?'page':undefined} onClick={()=>navigate(p)}><Icon/><span>{t(key)}</span></button>)}</nav>;}
export function ComputerCard({name,onClick,t,verified}:{name:string;onClick:()=>void;t:Translate;verified:boolean}){return <button className="card computer-card" onClick={onClick}><span className="computer-art"><Laptop size={38}/>{verified&&<i/>}</span><span className="grow"><strong>{name}</strong><span className={verified?'connected':'hint'}>{verified&&<i/>}{t(verified?'connected':'unverified')}</span></span><span className="subtle-action">{t('computer')}<ChevronRight size={16}/></span></button>;}
export function ProjectCard({name,children}:{name:string;children?:React.ReactNode}){return <div className="card project-card"><span className="folder-icon"><img src="/assets/folder.png" alt=""/></span><span><small>{name}</small>{children}</span></div>;}
export { Plus, RefreshCw };
