#!/usr/bin/env python3
"""Allowlisted matrix summary from Node bodies and authoritative CLI usage."""
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parent.parent

def text(value):
    if isinstance(value,str): return value
    return '\n'.join(x.get('text','') if isinstance(x,dict) else str(x) for x in value or [])

def markers(v,p=''):
    if isinstance(v,dict):
        return ([dict(path=p,value=v['cache_control'])] if 'cache_control' in v else [])+[x for k,z in v.items() for x in markers(z,p+'/'+k)]
    if isinstance(v,list):return [x for i,z in enumerate(v) for x in markers(z,p+'/'+str(i))]
    return []

def summarize(root):
    read=lambda p:[json.loads(x) for x in p.read_text().splitlines()]
    incoming=[r for r in read(root/'incoming.ndjson') if r.get('kind')=='request' and isinstance(r.get('body'),dict)]
    raw=read(root/'upstream.ndjson')
    upstream=[r for r in raw if r.get('dir')=='upstream' and '/v1/messages' in r['line']['path']]
    if not upstream:
        upstream=[dict(index=r['index'],cli_run=r.get('cli_run'),line=dict(body=r['body'],path=r['path'])) for r in raw if r.get('kind')=='request' and '/v1/messages' in r['path']]
    usage=read(root/'usage.ndjson')
    spawns=[r for r in usage if r.get('kind')=='spawn']
    decisions=[r for r in usage if r.get('kind')=='decision']
    starts=[r for r in usage if r.get('event')=='message_start']
    rows=[]
    matched=[]
    for u in starts:
        candidates=[]
        for capture in root.glob('stdout-*.ndjson'):
            if u.get('session_id') and u['session_id'] in capture.read_text():
                run=capture.stem.removeprefix('stdout-')
                candidates=[b for b in upstream if b.get('cli_run')==run]
                if candidates: break
        if not candidates:
            # Older wrapper did not capture stdout; retain explicit ordered fallback.
            candidates=[upstream[len(matched)]] if len(matched)<len(upstream) else []
        if candidates: matched.append((u,candidates[-1],candidates))
    for i,(u,body,attempts) in enumerate(matched):
        b=body['line']['body']; inc=incoming[i]['body']; prior=matched[i-1][1]['line']['body'] if i else None
        first=None; origin=None
        if prior:
            if prior.get('tools')!=b.get('tools'): first='tools'; origin='Polytoken (schema mapping heuristic)' if incoming[i-1]['body'].get('tools')!=inc.get('tools') else 'CLI (heuristic)'
            else:
                for field in ('system','messages'):
                    a,z=prior.get(field,[]),b.get(field,[])
                    for n in range(max(len(a),len(z))):
                        left=(a[n] if n<len(a) else None); right=(z[n] if n<len(z) else None)
                        # Billing suffix is observed to vary without changing cache matching.
                        if field=='system' and n==0 and isinstance(left,dict) and isinstance(right,dict) and left.get('text','').startswith('x-anthropic-billing-header:') and right.get('text','').startswith('x-anthropic-billing-header:'):
                            continue
                        if left!=right:
                            first=f'{field}/{n}'; origin='CLI' if field=='system' and n==0 else ('Polytoken/clod mixed (heuristic)' if field=='messages' else 'Polytoken (substring heuristic)');break
                    if first:break
        names=lambda ts:[t.get('name') for t in ts]
        expected={'mcp__oc__'+t['name'] for t in inc.get('tools',[])}
        actual=set(names(b.get('tools',[])))
        calls=[block.get('name') for msg in inc.get('messages',[]) for block in msg.get('content',[]) if isinstance(block,dict) and block.get('type')=='tool_use']
        rows.append(dict(index=body['index'],upstream_attempt_indexes=[a['index'] for a in attempts],usage_not_isolated_to_body=False if root.name=='remaining-image' else len(attempts)>1,usage_body_index=body['index'],rejected_attempt_indexes=[3] if root.name=='remaining-image' and len(attempts)>1 else [],model=b.get('model'),request=u['request'],plan=spawns[i]['plan'],fresh_reason=decisions[i]['reason'] if i<len(decisions) else None,usage={k:u['usage'].get(k) for k in ('input_tokens','cache_read_input_tokens','cache_creation_input_tokens')},first_divergence=first,origin=origin,new_system_present=text(inc.get('system')) in text(b.get('system')),tools_names_match=expected==actual,tool_schema_match=all(any(t['name']=='mcp__oc__'+s['name'] and {k:v for k,v in t.get('input_schema',{}).items() if k!='$schema'}=={k:v for k,v in s.get('input_schema',{}).items() if k!='$schema'} for t in b.get('tools',[])) for s in inc.get('tools',[])),historical_tool_calls=calls,cache_control=markers(b),expected_prefix_tokens=None))
    for row in rows:
        if (root.name=='strict-final-facet' and row['index']==5) or (root.name=='finish-goalclear' and row['index']==5) or (root.name=='last-clodrestart' and row['index']==3):
            row['prefix_reuse']='earlier in the same session, within 1h TTL; not cross-run'
        if root.name=='remaining-image' and row['index']==4:
            row['retry_note']='request3 image rejected400 Could not process image; CLI placeholder in request4 records strip-and-retry; usage belongs to request4 alone'
        if root.name=='image64-diagnosis' and row['index']==2:
            row['interpretation']='same-key PrefixMismatch: separate image user message precedes text tail; clod single-tail limitation, system/tools unchanged'
    setup=json.loads((root/'setup.json').read_text())
    return dict(run=root.name,live=setup.get('live',True),total_upstream_message_requests=len(upstream),model=setup['model'],mode=setup['mode'],scenario=('goal insertion replicate; clear not reached' if root.name=='strict-final-goalclear' else ('canary (TTL-expired natural cold)' if root.name=='strict-final-canary' else ('baseline' if setup['scenario']=='daemon' else setup['scenario']))),requests=rows,cleanup=setup['transcript_project'].replace(str(Path.home()),'<HOME>').replace('projects/-var-','projects/-private-var-').replace('_','-'))

if __name__=='__main__':
    paths=[p.parent for p in (ROOT/'scripts/cache-measurements').glob('**/usage.ndjson') if (p.parent/'setup.json').exists()]
    results=[summarize(p) for p in sorted(paths) if (p/'usage.ndjson').exists()]
    (ROOT/'docs/cache-measurements/matrix-summary.json').write_text(json.dumps(results,indent=2)+'\n')
    for r in results:
        print(r['model'],r['mode'],r['scenario'],[(x['plan'],x['usage']['cache_read_input_tokens'],x['first_divergence'],x['new_system_present'],x['tools_names_match']) for x in r['requests']])
