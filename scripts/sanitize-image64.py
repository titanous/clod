"""Offline image64 regression export; no network or CLI calls."""
import json
from pathlib import Path

source = Path('scripts/cache-measurements/image64-diagnosis/incoming.ndjson')
rows = [json.loads(line)['body'] for line in source.read_text().splitlines() if '"body"' in line]
# Preserve real role/block ordering and image bytes, not private orientation/tool text.
for body in rows[:2]:
    body['system'] = 'sanitized image64 regression'
    body['tools'] = []
    body['model'] = 'test'
    for message in body['messages']:
        content = message['content']
        if isinstance(content, str):
            message['content'] = 'sanitized text'
        else:
            for block in content:
                if block.get('type') == 'text':
                    block['text'] = 'sanitized text'
Path('crates/clod-core/tests/fixtures/live/image64-requests.json').write_text(json.dumps(rows[:2], indent=2) + '\n')
