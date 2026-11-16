// Synthetic visual proof only: intercepted v1 fixtures, never imported by src/.
import {chromium} from 'playwright';
import {readFile,mkdir,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
const out=process.env.KILN_VISUAL_OUT;if(!out)throw new Error('KILN_VISUAL_OUT required');await mkdir(out,{recursive:true});
const fixture=async name=>JSON.parse(await readFile(new URL(`../../protocol/fixtures/${name}.json`,import.meta.url),'utf8'));
const bootstrap=await fixture('bootstrap'),list=await fixture('conversations'),snapshot=await fixture('approval');
const option=(id,label,kind,current,choices=[])=>({id,label,kind,current,choices,description:`Synthetic native advertised ${label}`,category:null,scope:'Live',available:true,reason:null,source:null,documentation:null,mobile_supported:true,mobile_reason:null});
snapshot.controls.options=[option('model','모델','Select',{Text:'native-model'},[{value:'native-model',label:'Native model',description:'Synthetic advertised model',group:null}]),option('effort','Effort','Select',{Text:'high'},[{value:'high',label:'High',description:'Synthetic advertised effort',group:null}]),option('ultracode','Ultracode','Boolean',{Bool:true})];
snapshot.models=[{id:'native-model',label:'Native model'}];snapshot.selected_model='native-model';snapshot.modes=[{id:'default',label:'기본',description:'Synthetic mode'}];snapshot.selected_mode='default';
snapshot.requests[0].title='Read';snapshot.requests[0].details={file_path:'README.md'};
const browser=await chromium.launch({headless:true,channel:'chrome'});
try{
 const context=await browser.newContext({viewport:{width:390,height:844},locale:'ko-KR',colorScheme:'light'});const page=await context.newPage();const failures=[];
 page.on('pageerror',e=>failures.push(String(e)));
 await page.route('https://visual.invalid/v1/**',route=>{const path=new URL(route.request().url()).pathname;const body=path==='/v1/bootstrap'?bootstrap:path==='/v1/conversations'?list:path==='/v1/terminals'?{terminals:[],truncated:false}:snapshot;return route.fulfill({status:200,contentType:'application/json',body:JSON.stringify(body),headers:{'Access-Control-Allow-Origin':'http://127.0.0.1:5178','Access-Control-Allow-Headers':'Authorization, Content-Type'}});});
 await page.goto('http://127.0.0.1:5178/#p1');await page.screenshot({path:resolve(out,'p1.png'),fullPage:true});
 await page.getByLabel('컴퓨터 주소',{exact:true}).fill('https://visual.invalid');await page.getByLabel('연결 토큰',{exact:true}).fill('synthetic-inert-token');await page.getByRole('button',{name:'연결 요청',exact:true}).click();await page.getByRole('button',{name:'연결 해제',exact:true}).waitFor();
 await page.evaluate(()=>location.hash='p0');await page.getByText('Review the example changes',{exact:true}).waitFor();await page.screenshot({path:resolve(out,'p0.png'),fullPage:true});
 await page.getByRole('button').filter({hasText:'Review the example changes'}).click();await page.getByText('도구 승인 대기 중',{exact:true}).waitFor();await page.screenshot({path:resolve(out,'p2.png'),fullPage:true});
 await page.getByRole('button',{name:'제공자 설정',exact:true}).click();await page.getByRole('heading',{name:'제공자 설정'}).waitFor();await page.screenshot({path:resolve(out,'p3.png'),fullPage:true});
 await page.getByRole('button',{name:'새 작업 · 시작 설정',exact:true}).click();await page.getByRole('heading',{name:'새 작업',exact:true}).waitFor();await page.screenshot({path:resolve(out,'p4.png'),fullPage:true});
 const measures=[];for(const width of [360,390,430]){await page.setViewportSize({width,height:844});measures.push({width,overflow:await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth)});}
 await page.emulateMedia({colorScheme:'dark'});await page.screenshot({path:resolve(out,'p4-dark-430.png'),fullPage:true});
 await writeFile(resolve(out,'result.json'),JSON.stringify({proof:'synthetic intercepted fixtures, no native execution',viewport:{width:390,height:844},pageErrors:failures,measures},null,2));if(failures.length||measures.some(m=>m.overflow))throw new Error(JSON.stringify({failures,measures}));await context.close();
}finally{await browser.close();}
