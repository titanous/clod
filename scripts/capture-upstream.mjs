#!/opt/homebrew/bin/node
// Body-only capture. Never logs authentication or response headers.
import http from 'node:http';
import https from 'node:https';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import zlib from 'node:zlib';
import { pathToFileURL } from 'node:url';
export function capture({destination, target='https://api.anthropic.com', ceiling=6, onLimit=()=>process.exit(75)}) {
 fs.mkdirSync(path.dirname(destination),{recursive:true});
 const start=Date.now(); let forwarded=0;
 const clean=s=>s.replaceAll(os.homedir(),'<HOME>').replace(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi,'<EMAIL>').replace(/sk-ant-[\w-]+/g,'<TOKEN>').replace(/"(?:account_uuid|accountUuid|organization_uuid|organizationUuid)"\s*:\s*"[^"]*"/g,'"redacted_account":"<ACCOUNT>"');
 return http.createServer((req,res)=>{
  const chunks=[];req.on('data',c=>chunks.push(c));req.on('end',()=>{
   let route=req.url, run=null;
   const match=route.match(/^\/__run\/([a-f0-9]+)(\/.*)$/);
   if(match){run=match[1];route=match[2];}
   if(route.startsWith('/v1/messages') && ++forwarded>ceiling){
    fs.appendFileSync(destination,JSON.stringify({kind:'ceiling',forwarded:forwarded-1,ceiling})+'\n');
    res.writeHead(503);res.end('measurement request ceiling');onLimit();return;
   }
   const requestIndex=forwarded;
   const body=Buffer.concat(chunks);let value;try{value=JSON.parse(body.toString());}catch{value='<non-JSON omitted>';}
   fs.appendFileSync(destination,clean(JSON.stringify({dir:'upstream',index:forwarded,cli_run:run,t_ms:Date.now()-start,line:{method:req.method,path:route,body:value}}))+'\n');
   const url=new URL(target);const transport=url.protocol==='https:'?https:http;
   const headers={...req.headers,host:url.host};delete headers.connection;
   const upstream=transport.request({hostname:url.hostname,port:url.port||undefined,path:route,method:req.method,headers},r=>{
    // Node decodes HTTP transfer framing, then re-frames the same payload bytes.
    // Content-Encoding is retained, so gzip remains correctly identified.
    if(r.statusCode>=400){
     const errors=[];r.on('data',c=>errors.push(c));r.on('end',()=>{
      let bytes=Buffer.concat(errors);let errorBody;
      try{if(r.headers['content-encoding']==='gzip')bytes=zlib.gunzipSync(bytes);else if(r.headers['content-encoding']==='br')bytes=zlib.brotliDecompressSync(bytes);errorBody=bytes.toString('utf8');}catch{errorBody='<error body decoding failed>';}
      const safeHeaders=Object.fromEntries(Object.entries(r.headers).filter(([k])=>['content-type','content-encoding','content-length','retry-after','request-id','anthropic-request-id'].includes(k)));
      fs.appendFileSync(destination,clean(JSON.stringify({kind:'upstream_error',cli_run:run,index:requestIndex,status:r.statusCode,headers:safeHeaders,body:errorBody}))+'\n');
     });
    }
    res.writeHead(r.statusCode,r.headers);r.pipe(res);
   });
   upstream.on('error',()=>{if(!res.headersSent)res.writeHead(502);res.end('upstream connection failed');});upstream.end(body);
  });
 });
}
if(import.meta.url===pathToFileURL(process.argv[1]).href){
 const server=capture({destination:path.resolve(process.env.CLOD_UPSTREAM_LOG||'scripts/captures/upstream.ndjson'),target:process.env.CLOD_CAPTURE_TARGET||'https://api.anthropic.com',ceiling:Number(process.env.CLOD_CAPTURE_LIMIT||6)});
 server.listen(Number(process.env.CLOD_UPSTREAM_PORT||19876),'127.0.0.1',()=>console.log(server.address().port));
}
