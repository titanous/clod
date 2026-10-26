#!/usr/bin/env python3
"""Export allowlisted numeric/hash evidence, never raw prompt or auth data."""
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
runs = ['haiku-canary-node-1', 'haiku-node-strict-daemon']
result = []
for name in runs:
    root = ROOT / 'scripts/cache-measurements' / name
    read = lambda p: [json.loads(x) for x in p.read_text().splitlines()]
    usage = read(root / 'usage.ndjson')
    bodies = [x for x in read(root / 'upstream.ndjson') if x.get('dir') == 'upstream' and x['line']['path'].startswith('/v1/messages')]
    incoming = [x for x in read(root / 'incoming.ndjson') if x.get('kind') == 'request' and isinstance(x.get('body'), dict)]
    records = []
    for row in usage:
        if row['kind'] == 'spawn':
            records.append({k: row[k] for k in ('kind', 'request', 'attempt', 'plan', 'key_mode')})
        else:
            u = row['usage']
            records.append(dict(kind='usage', request=row['request'], attempt=row['attempt'], event=row['event'], usage={k: u[k] for k in ('input_tokens', 'cache_creation_input_tokens', 'cache_read_input_tokens', 'output_tokens', 'cache_creation') if k in u}))
    result.append(dict(run=name, upstream_message_requests=len(bodies), usage=records, incoming=[dict(index=x['index'], tool_count=len(x['body'].get('tools', [])), tool_names=[t['name'] for t in x['body'].get('tools', [])] if len(x['body'].get('tools', [])) == 1 else None, context_hashes={k:v for k,v in x['wire_sha256'].items() if k in ('/system','/tools')}) for x in incoming], cleanup_project=json.loads((root/'setup.json').read_text())['transcript_project'].replace(str(Path.home()), '<HOME>')))
output = ROOT / 'docs/cache-measurements/cli-usage-summary.json'
output.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps([dict(run=x['run'],requests=x['upstream_message_requests']) for x in result]))
