#!/usr/bin/env python3
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parent.parent
rows=json.loads((ROOT/'docs/cache-measurements/matrix-summary.json').read_text())
lines=['| Run | Model | Mode | Scenario | upstream attempt indexes | plans | cache reads | creation | uncached input | first divergences | new system upstream |','|---|---|---|---|---|---|---|---|---|---|---|']
for r in rows:
    xs=r['requests']
    def seq(fn):return ' → '.join(str(fn(x)) for x in xs)
    lines.append('|'+ '|'.join([r['run'],seq(lambda x:x.get('model',r['model']).replace('claude-','')),r['mode'],r['scenario']+(' (offline synthetic)' if not r.get('live',True) else ''),seq(lambda x:str(x.get('upstream_attempt_indexes',[x['index']]))+('* aggregate/non-isolated' if x.get('usage_not_isolated_to_body') else (' (rejected '+str(x['rejected_attempt_indexes'])+'; usage only '+str(x['usage_body_index'])+')' if x.get('rejected_attempt_indexes') else ''))),seq(lambda x:x['plan']),seq(lambda x:x['usage']['cache_read_input_tokens']),seq(lambda x:x['usage']['cache_creation_input_tokens']),seq(lambda x:x['usage']['input_tokens']),seq(lambda x:x['first_divergence'] or 'initial'),seq(lambda x:x['new_system_present'])])+'|')
(ROOT/'docs/cache-measurements/matrix-table.md').write_text('\n'.join(lines)+'\n')
print('requests',sum(len(r['requests']) for r in rows))
