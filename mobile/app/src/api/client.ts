import { Capacitor, CapacitorHttp, type HttpOptions, type HttpResponse } from '@capacitor/core';
export class ApiFailure extends Error {
 constructor(public code:string, public status:number) { super(code); }
}
export function validateAddress(value:string, development=false):string {
 const u=new URL(value.trim());
 if(u.username || u.password || u.search || u.hash || u.pathname!=='/') throw new Error('invalid_address');
 const local=['localhost','127.0.0.1','10.0.2.2','[::1]'].includes(u.hostname);
 if(u.protocol!=='https:' && !(development && local && u.protocol==='http:')) throw new Error('https_required');
 return u.origin;
}
interface Transport {native?:boolean;nativeRequest?:(o:HttpOptions)=>Promise<HttpResponse>;fetch?:typeof fetch}
export class RemoteClient {
 readonly base:string;
 constructor(base:string,private token:string,private transport:Transport={}, development=import.meta.env.VITE_ALLOW_LOOPBACK_HTTP==='true') {
  this.base=validateAddress(base,development);
  if(!token || token.length>512 || !/^[\x21-\x7e]+$/.test(token)) throw new Error('invalid_token');
 }
 clear(){this.token='';}
 async request<T>(method:'GET'|'POST',path:string,data?:unknown):Promise<T> {
  if(!this.token) throw new ApiFailure('disconnected',401);
  if(!/^\/v1\/[a-zA-Z0-9/_-]+$/.test(path)) throw new Error('invalid_route');
  const url=this.base+path;
  const headers={Authorization:`Bearer ${this.token}`,'Content-Type':'application/json'};
  let status:number;let body:unknown;
  if(this.transport.native ?? Capacitor.isNativePlatform()) {
   const response=await (this.transport.nativeRequest ?? CapacitorHttp.request)({url,method,headers,data,disableRedirects:true,responseType:'json',connectTimeout:6000,readTimeout:6000});
   status=response.status;body=response.data;
  } else {
   const response=await (this.transport.fetch ?? globalThis.fetch)(url,{method,headers,body:data===undefined?undefined:JSON.stringify(data),redirect:'error',credentials:'omit',cache:'no-store',signal:AbortSignal.timeout(6000)});
   status=response.status;body=await response.json();
  }
  if(status<200 || status>=300) throw new ApiFailure(typeof body==='object' && body!==null && 'error'in body && typeof (body as {error:{code?:unknown}}).error?.code==='string'?(body as {error:{code:string}}).error.code:'request_failed',status);
  return body as T;
 }
 get<T>(path:string){return this.request<T>('GET',path);}
 post<T>(path:string,data:unknown){return this.request<T>('POST',path,data);}
}
