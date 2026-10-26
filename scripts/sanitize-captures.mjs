#!/opt/homebrew/bin/node
import fs from 'node:fs';import path from 'node:path';import os from 'node:os';
const base=path.resolve(process.argv[2]||'scripts/captures/phase0');const out=path.resolve('crates/clod-core/tests/fixtures');
const clean=s=>s.replaceAll(os.homedir(),'<HOME>').replaceAll(os.homedir().replace(/[^a-zA-Z0-9]/g,'-'),'<HOME_PROJECT>').replace(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi,'<EMAIL>').replace(/sk-ant-[\w-]+/g,'<TOKEN>');
function scrub(v){if(Array.isArray(v))return v.map(scrub);if(v&&typeof v==='object')return Object.fromEntries(Object.entries(v).map(([k,x])=>[k,/^(account|email|organization|accountUuid|account_uuid|organizationUuid|organization_id|userID|user_id|device_id)$/.test(k)?'<REDACTED>':scrub(x)]));return typeof v==='string'?clean(v):v;}
const evidence=[];
for(const name of fs.readdirSync(base)){const dir=path.join(base,name);if(!fs.statSync(dir).isDirectory())continue;const rows=[];let argv=[];
for(const f of fs.readdirSync(dir).sort()){
if(f.endsWith('.argv'))argv=fs.readFileSync(path.join(dir,f),'utf8').split('\0').filter(Boolean);
for(const [suffix,d]of [['stdin','in'],['stdout','out'],['stderr','err']])if(f.endsWith('.'+suffix))for(const line of fs.readFileSync(path.join(dir,f),'utf8').split('\n').filter(Boolean)){let obj;try{obj=JSON.parse(line);}catch{obj=line;}rows.push({dir:d,t_ms:0,line:scrub(obj)});}}
fs.writeFileSync(path.join(out,name+'.ndjson'),rows.map(JSON.stringify).join('\n')+'\n');
const timing=JSON.parse(fs.readFileSync(path.join(dir,'timing.json')));const messages=JSON.parse(fs.readFileSync(path.join(dir,'sdk.json')));const result=messages.find(x=>x.type==='result');
evidence.push({name,argv:scrub(argv),timing,assistant:messages.filter(x=>x.type==='assistant').map(x=>({uuid:x.uuid,session_id:x.session_id,content:x.message.content})),result:result?{subtype:result.subtype,session_id:result.session_id,errors:result.errors,cache_read_input_tokens:result.usage?.cache_read_input_tokens}:null,stderrRows:rows.filter(x=>x.dir==='err').length});}
fs.writeFileSync(path.join(out,'capture-evidence.json'),JSON.stringify(scrub(evidence),null,2));
const upstream='scripts/captures/upstream.ndjson';if(fs.existsSync(upstream)){const rows=fs.readFileSync(upstream,'utf8').trim().split('\n').map(x=>scrub(JSON.parse(x)));fs.writeFileSync(path.join(out,'upstream.ndjson'),rows.map(JSON.stringify).join('\n')+'\n');}
const synthetic=[['g-usage-synthetic','rate limit exceeded'],['g-auth-synthetic','API Error: 401 authentication_error'],['g-overflow-synthetic','Claude Code returned an error result: Prompt is too long']];for(const[name,text]of synthetic)fs.writeFileSync(path.join(out,name+'.ndjson'),JSON.stringify({dir:'err',t_ms:0,synthetic:true,source:'meridian/src/__tests__/errors.test.ts',line:text})+'\n');
console.log('Sanitized',evidence.length,'CLI exchanges; wrote upstream, evidence, and synthetic errors.');
