#!/usr/bin/env python3
"""Verify captured offline CLI experiment without executing the CLI or networking."""
import json
from pathlib import Path
import sys


def verify(root):
    for name in ['probe', 'mcp__oc__probe']:
        for strings in [False, True]:
            case = root / (name + ('-strings' if strings else '-typed'))
            records = [json.loads(line) for line in (case / 'stdout.ndjson').read_text().splitlines()]
            requests = json.loads((case / 'requests.json').read_text())
            assert len(requests) == 1, (case, 'unexpected provider request count')
            body = json.loads(requests[0]['body'])
            tools = body['tools']
            assert len(tools) == 1 and tools[0]['name'] == 'mcp__oc__probe'
            properties = tools[0]['input_schema']['properties']
            assert {k: v['type'] for k, v in properties.items()} == {'items': 'array', 'count': 'number', 'limit': 'integer'}
            events = [json.loads(part.split('\ndata: ', 1)[1]) for part in (case / 'provider.sse').read_text().strip().split('\n\n')]
            raw = ''.join(e['delta']['partial_json'] for e in events if e['type'] == 'content_block_delta')
            streamed = ''.join(r['event']['delta']['partial_json'] for r in records if r.get('type') == 'stream_event' and r['event']['type'] == 'content_block_delta')
            assert streamed == raw, (case, 'CLI changed raw stream')
            assistant = [b for r in records if r.get('type') == 'assistant' for b in r['message']['content'] if b['type'] == 'tool_use']
            assert len(assistant) == 1 and assistant[0]['name'] == name
            typed = {'items': ['a', 'b'], 'count': 2.5, 'limit': 3}
            expected = typed if name.startswith('mcp__') or not strings else {'items': '["a","b"]', 'count': '2.5', 'limit': '3'}
            assert assistant[0]['input'] == expected, (case, assistant)
            users = [str(r) for r in records if r.get('type') == 'user']
            missing = any('No such tool available: probe' in u for u in users)
            assert missing == (name == 'probe'), (case, users)
            calls = [r for r in records if r.get('type') == 'control_request' and r['request'].get('message', {}).get('method') == 'tools/call']
            assert bool(calls) == (name == 'mcp__oc__probe'), (case, calls)
            if calls:
                assert calls[0]['request']['message']['params']['arguments'] == typed
            result = next(r for r in records if r.get('type') == 'result')
            assert result['subtype'] == 'error_max_turns', result
            print(case.name + ': schema, exact raw SSE/delta, assistant normalization, name/error, execution arguments verified')


if __name__ == '__main__':
    verify(Path(sys.argv[1]))
