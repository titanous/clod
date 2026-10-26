import http from 'node:http';
import zlib from 'node:zlib';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {capture} from './capture-upstream.mjs';
const fixture=Buffer.from('event: message_start\ndata: {"type":"message_start","message":{"usage":{"input_tokens":3}}}\n\nevent: message_stop\ndata: {"type":"message_stop"}\n\n');
const gzip=zlib.gzipSync(fixture);
const root=fs.mkdtempSync(path.join(os.tmpdir(),'clod-proxy-test-'));
const upstream=http.createServer((req,res)=>{res.writeHead(200,{'content-type':'text/event-stream','content-encoding':'gzip','transfer-encoding':'chunked'});res.write(gzip.subarray(0,13));res.end(gzip.subarray(13));});
await new Promise(r=>upstream.listen(0,'127.0.0.1',r));
let limited=false;
const proxy=capture({destination:path.join(root,'capture.ndjson'),target:`http://127.0.0.1:${upstream.address().port}`,ceiling:1,onLimit:()=>{limited=true;}});
await new Promise(r=>proxy.listen(0,'127.0.0.1',r));
const request=()=>new Promise((resolve,reject)=>{const req=http.request({host:'127.0.0.1',port:proxy.address().port,path:'/__run/abcdef/v1/messages',method:'POST'},res=>{const chunks=[];res.on('data',c=>chunks.push(c));res.on('end',()=>resolve({status:res.statusCode,headers:res.headers,body:Buffer.concat(chunks)}));});req.on('error',reject);req.end('{}');});
try{
 const response=await request();
 assert.equal(response.status,200);assert.equal(response.headers['content-encoding'],'gzip');
 assert.deepEqual(response.body,gzip);assert.deepEqual(zlib.gunzipSync(response.body),fixture);
 const record=JSON.parse(fs.readFileSync(path.join(root,'capture.ndjson'),'utf8').trim());assert.equal(record.cli_run,'abcdef');assert.equal(record.line.path,'/v1/messages');
 assert.equal((await request()).status,503);assert.equal(limited,true);
 console.log('PASS: chunked gzip SSE bytes/encoding preserved; run prefix removed; request ceiling enforced');
}finally{proxy.closeAllConnections();upstream.closeAllConnections();await new Promise(r=>proxy.close(r));await new Promise(r=>upstream.close(r));fs.rmSync(root,{recursive:true});}
