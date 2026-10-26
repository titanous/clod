#!/opt/homebrew/bin/node
import { query, createSdkMcpServer, tool } from '../meridian/node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs';
import { z } from '../meridian/node_modules/zod/index.js';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createHash, randomUUID } from 'node:crypto';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const run = process.env.CLOD_CAPTURE_RUN || new Date().toISOString().replaceAll(':','-');
const base = path.join(root, 'scripts/captures', run);
const cwd = path.join(root,'scripts/captures/workdir');
fs.mkdirSync(base,{recursive:true}); fs.mkdirSync(cwd,{recursive:true});
const fixtureDir = path.join(root,'crates/clod-core/tests/fixtures'); fs.mkdirSync(fixtureDir,{recursive:true});
const sanitize = s => s.replaceAll(os.homedir(),'<HOME>').replace(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi,'<EMAIL>').replace(/sk-ant-[\w-]+/g,'<TOKEN>').replace(/"(?:accountUuid|account_uuid|organizationUuid|organization_id)"\s*:\s*"[^"]*"/g,'"redacted_account":"<ACCOUNT>"');
const hash = p => fs.existsSync(p) ? createHash('sha256').update(fs.readFileSync(p)).digest('hex') : null;
const transcript = id => path.join(os.homedir(),'.claude/projects',cwd.replace(/[^a-zA-Z0-9]/g,'-'),`${id}.jsonl`);
const summary=[];
async function turn(name, content, prior, extra={}) {
  const dir=path.join(base,name);fs.mkdirSync(dir,{recursive:true});
  const started=Date.now(), events=[], messages=[]; let generating=false, pending=[];
  const note=(kind,data)=>events.push({t_ms:Date.now()-started,kind,...data});
  const release=reason=>{generating=false;note('release',{reason,count:pending.length});for(const f of pending.splice(0))f();};
  const server=createSdkMcpServer({name:'oc',version:'1.0.0',tools:[
    tool('lookup','Look up the value of a named item',{key:z.string()},async()=>{throw Error('Tool must be denied, not executed');}),
    tool('combine','Combine named values',{values:z.array(z.string())},async()=>{throw Error('Tool must be denied, not executed');})
  ]});
  const env={...process.env,CLOD_CAPTURE_DIR:dir,CLAUDE_CODE_DISABLE_ATTACHMENTS:'1',ENABLE_CLAUDEAI_MCP_SERVERS:'false',ENABLE_TOOL_SEARCH:'false',CLAUDE_CODE_MAX_OUTPUT_TOKENS:'2048'};
  for(const k of ['CLAUDE_CONFIG_DIR','ANTHROPIC_API_KEY','ANTHROPIC_AUTH_TOKEN','CLAUDECODE'])delete env[k];
  const source=prior?.session_id ? transcript(prior.session_id):null;
  const before=source?hash(source):null;
  const options={cwd,model:'haiku',pathToClaudeCodeExecutable:path.join(root,'scripts/capture-claude.sh'),maxTurns:1,tools:[],strictMcpConfig:true,settingSources:[],plugins:[],permissionMode:'bypassPermissions',allowDangerouslySkipPermissions:true,systemPrompt:'You are a precise test assistant. Follow the user instructions exactly. Use only the provided tools when requested.',includePartialMessages:true,mcpServers:{oc:server},allowedTools:['mcp__oc__lookup','mcp__oc__combine'],env,
    hooks:{PreToolUse:[{matcher:'mcp__oc__.*',hooks:[async input=>{
      note('hook_enter',{tool:input.tool_name,generating});
      if(generating)await new Promise(resolve=>{const timer=setTimeout(()=>{note('hold_timeout',{});resolve();},60000);pending.push(()=>{clearTimeout(timer);resolve();});});
      note('hook_answer',{tool:input.tool_name});return {hookSpecificOutput:{hookEventName:'PreToolUse',permissionDecision:'deny',permissionDecisionReason:'Forwarded to client. End your turn.'}};
    }]}]},...(prior?{resume:prior.session_id,resumeSessionAt:prior.uuid,forkSession:true}:{}),...extra};
  let q;
  try {
    q=query({prompt:(async function*(){yield {type:'user',message:{role:'user',content},parent_tool_use_id:null,session_id:''};})(),options});
    for await(const m of q){messages.push(m);note('sdk',{type:m.type,event:m.event?.type,uuid:m.uuid,session_id:m.session_id});
      if(m.type==='stream_event'){if(m.event.type==='message_start')generating=true;if(['message_delta','message_stop'].includes(m.event.type))release(m.event.type);}
    }
  } catch(e){note('exception',{message:e.message});} finally {release('finally');q?.close();}
  fs.writeFileSync(path.join(dir,'sdk.json'),JSON.stringify(messages,null,2)); fs.writeFileSync(path.join(dir,'timing.json'),JSON.stringify(events,null,2));
  const assistant=messages.filter(m=>m.type==='assistant').at(-1);
  const result=messages.find(m=>m.type==='result');const session_id=result?.session_id||messages.find(m=>m.session_id)?.session_id;
  const calls=messages.filter(m=>m.type==='assistant').flatMap(m=>m.message.content.filter(b=>b.type==='tool_use'));
  const record={name,session_id,uuid:assistant?.uuid,calls,result,events,sourceBefore:before,sourceAfter:source?hash(source):null,sourcePath:source};summary.push(record);
  fs.writeFileSync(path.join(dir,'summary.json'),sanitize(JSON.stringify(record,null,2)));
  const lines=[];
  for(const f of fs.readdirSync(dir).sort())for(const [ext,direction] of [['stdin','in'],['stdout','out'],['stderr','err']])if(f.endsWith('.'+ext)){
    for(const line of fs.readFileSync(path.join(dir,f),'utf8').split('\n').filter(Boolean)){let value;try{value=JSON.parse(line);}catch{value=line;}lines.push({dir:direction,t_ms:0,line:value});}
  }
  fs.writeFileSync(path.join(fixtureDir,`${name}.ndjson`),sanitize(lines.map(l=>JSON.stringify(l)).join('\n')+'\n'));
  fs.writeFileSync(path.join(base,'summary.json'),sanitize(JSON.stringify(summary,null,2)));
  console.log(JSON.stringify({name,session_id,uuid:assistant?.uuid,calls:calls.length,result:result?.subtype,error:events.find(e=>e.kind==='exception'),sourceUnchanged:before!==null?before===record.sourceAfter:null}));
  return record;
}
function results(r, suffix='') {return [...r.calls.map(c=>({type:'tool_result',tool_use_id:c.id,content:`VALUE_${c.input.key||'COMBINED'}`})),...(suffix?[{type:'text',text:suffix}]:[])];}
const selected=process.argv[2]||'all';
if(['all','a'].includes(selected)){const a=await turn('a-text','Remember the word cobalt. Reply only OK.');await turn('a-text-resume','What word did I ask you to remember? Reply only that word.',a);}
if(['all','b','upstream'].includes(selected)){const b=await turn(selected==='upstream'?'upstream-tool':'b-tool','Call lookup once with key alpha. Do not answer without calling it.');if(b.calls.length)await turn(selected==='upstream'?'upstream-tool-resume':'b-tool-resume',results(b,'Report the exact returned value, no more tools.'),b);}
if(['all','c'].includes(selected)){const c=await turn('c-parallel','Call lookup THREE times in parallel, keys alpha, beta, gamma. Do not combine them or answer without tools.');if(c.calls.length){const c2=await turn('c-second-round',results(c,'Now call combine once with all three returned values. Do not answer yet.'),c);if(c2.calls.length)await turn('c-final',results(c2,'Report all original values and the combined result. Do not call more tools.'),c2);}}
if(['all','d'].includes(selected))await turn('d-thinking','What is 17 times 23? Think briefly then give only the number.',null,{model:'sonnet',thinking:{type:'enabled',budgetTokens:1024}});
if(['all','e'].includes(selected))await turn('e-image',[{type:'image',source:{type:'base64',media_type:'image/png',data:'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6nS8AAAAASUVORK5CYII='}},{type:'text',text:'Describe the image in five words or fewer.'}]);
if(['all','f'].includes(selected)){await turn('f-missing-session','Say OK.',{session_id:randomUUID(),uuid:randomUUID()});const seed=await turn('f-seed','Say OK.');await turn('f-missing-uuid','Say OK.',{...seed,uuid:randomUUID()});}
console.log('Capture directory:',base);
