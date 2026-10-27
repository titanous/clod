import http from 'node:http';
import {once} from 'node:events';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {capture} from './capture-upstream.mjs';

const root=fs.mkdtempSync(path.join(os.tmpdir(),'clod-response-test-'));
const destination=path.join(root,'requests.ndjson');
const responseDestination=path.join(root,'responses.ndjson');
const payload=Buffer.alloc(4*1024*1024,0xab);
let backpressured=false;
const upstream=http.createServer(async(req,res)=>{
 res.writeHead(200,{'content-type':'text/event-stream','request-id':'fixture-id','set-cookie':'private-cookie','authorization':'private-token'});
 if(req.url==='/abort'){
  res.write(Buffer.from('partial'));
  setImmediate(()=>res.destroy());
  return;
 }
 for(let offset=0;offset<payload.length;offset+=16384){
  if(!res.write(payload.subarray(offset,offset+16384))){backpressured=true;await once(res,'drain');}
 }
 res.end();
});
await new Promise(r=>upstream.listen(0,'127.0.0.1',r));
const proxy=capture({destination,responseDestination,responseLimitBytes:31,target:`http://127.0.0.1:${upstream.address().port}`});
await new Promise(r=>proxy.listen(0,'127.0.0.1',r));
const request=route=>new Promise((resolve,reject)=>{
 const req=http.get({host:'127.0.0.1',port:proxy.address().port,path:`/__run/abc123${route}`},res=>{
  const chunks=[];let done=false;
  const finish=complete=>{if(!done){done=true;resolve({complete,body:Buffer.concat(chunks)});}};
  // Pause immediately; resume on a later event-loop turn to exercise streaming flow control.
  res.pause();setImmediate(()=>res.resume());
  res.on('data',c=>chunks.push(c));res.on('end',()=>finish(true));res.on('error',()=>finish(false));res.on('aborted',()=>finish(false));
 });req.on('error',reject);
});
try{
 const results=await Promise.all([request('/one'),request('/two')]);
 for(const result of results){assert.equal(result.complete,true);assert.deepEqual(result.body,payload);}
 assert.equal(backpressured,true);
 const aborted=await request('/abort');assert.equal(aborted.complete,false);
 // Pipeline callbacks can follow the client's close event.
 await new Promise(r=>setImmediate(r));
 const requests=fs.readFileSync(destination,'utf8').trim().split('\n').map(JSON.parse);
 const records=fs.readFileSync(responseDestination,'utf8').trim().split('\n').map(JSON.parse);
 assert.equal(new Set(requests.map(r=>r.request_id)).size,3);
 for(const req of requests){
  const response=records.filter(r=>r.request_id===req.request_id);
  assert.ok(response.every(r=>r.cli_run==='abc123'));
  assert.equal(response[0].kind,'response_start');assert.equal(response[0].headers['request-id'],'fixture-id');
  const end=response.at(-1);assert.equal(end.kind,'response_end');
  const bytes=Buffer.concat(response.filter(r=>r.kind==='response_chunk').map(r=>Buffer.from(r.bytes_base64,'base64')));
  if(req.line.path==='/abort'){assert.equal(end.complete,false);assert.ok(end.error_code);}
  else{
   assert.deepEqual(bytes,payload.subarray(0,31));assert.equal(end.captured_bytes,31);assert.equal(end.received_bytes,payload.length);assert.equal(end.truncated,true);assert.equal(end.complete,true);
  }
 }
 assert.ok(!JSON.stringify(records).includes('private-'));
 assert.equal(fs.statSync(responseDestination).mode&0o777,0o600);
 assert.throws(()=>capture({destination,responseLimitBytes:-1}),/responseLimitBytes/);
 const zeroDestination=path.join(root,'zero.ndjson');
 const zero=capture({destination:zeroDestination,responseLimitBytes:0,target:`http://127.0.0.1:${upstream.address().port}`});
 await new Promise(r=>zero.listen(0,'127.0.0.1',r));
 try{
  const bytes=await new Promise((resolve,reject)=>http.get(`http://127.0.0.1:${zero.address().port}/one`,res=>{const chunks=[];res.on('data',c=>chunks.push(c));res.on('end',()=>resolve(Buffer.concat(chunks)));}).on('error',reject));
  assert.deepEqual(bytes,payload);
  const metadata=fs.readFileSync(`${zeroDestination}.responses.ndjson`,'utf8').trim().split('\n').map(JSON.parse);
  assert.deepEqual(metadata.map(r=>r.kind),['response_start','response_end']);assert.equal(metadata[1].captured_bytes,0);assert.equal(metadata[1].truncated,true);
 }finally{zero.closeAllConnections();await new Promise(r=>zero.close(r));}
 console.log('PASS bounded response capture, concurrent request pairing, slow-client bytes, abort handling, private header exclusion');
}finally{
 proxy.closeAllConnections();upstream.closeAllConnections();
 await new Promise(r=>proxy.close(r));await new Promise(r=>upstream.close(r));fs.rmSync(root,{recursive:true});
}
