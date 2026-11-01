#!/usr/bin/env python3
"""Verify bounded hook captures and actual transcript ancestry without CLI execution."""
import json
from pathlib import Path
import sys


def verify(root):
    proof = json.loads((root / 'confinement-proof.json').read_text())
    assert proof['loopback'] == 'connected'
    assert all(proof[ip]['errno'] == 1 for ip in ['192.0.2.1', '1.1.1.1'])
    summaries = json.loads((root / 'summary.json').read_text())
    labels = ['prefixed-control', 'bare-prefix-matcher', 'bare-catchall', 'bare-explicit']
    evidence = []
    assert len(labels) == len(summaries)
    for label, summary in zip(labels, summaries):
        case = root / label
        records = [json.loads(line) for line in (case / 'stdout.ndjson').read_text().splitlines()]
        assert summary['exit_code'] == 0 and not summary['timed_out']
        assert summary['requests'] == 2 and not summary['tools_call']
        assert summary['initialize'][0]['response']['subtype'] == 'success'
        requests = json.loads((case / 'requests.json').read_text())
        assert len(requests) == 2
        assert all(r['path'].startswith('/v1/messages') for r in requests)
        callbacks = [r for r in records if r.get('request', {}).get('subtype') == 'hook_callback']
        results = [r for r in records if r.get('type') == 'user' and isinstance(r.get('message', {}).get('content'), list) and any(b.get('type') == 'tool_result' for b in r['message']['content'])]
        assert len(results) == 2
        transcripts = list((case / 'home' / '.claude' / 'projects').rglob('*.jsonl'))
        assert len(transcripts) == 1
        rows = [json.loads(line) for line in transcripts[0].read_text().splitlines()]
        def block(kind, ident):
            return next(r for r in rows if any(b.get('type') == kind and b.get('id', b.get('tool_use_id')) == ident for b in r.get('message', {}).get('content', []) if isinstance(b, dict)))
        a, ar, b = block('tool_use', 'toolu_hook_0'), block('tool_result', 'toolu_hook_0'), block('tool_use', 'toolu_hook_1')
        if label == 'prefixed-control':
            assert len(callbacks) == 2
            assert all('Offline hook denied execution' in r['message']['content'][0]['content'] for r in results)
            pauses = [h for h in summary['hooks'] if 'pause_seconds' in h]
            assert len(pauses) == 2
            for callback, pause in zip(callbacks, pauses):
                assert pause['requests_before_deny'] == 1
                tool_id = callback['request']['tool_use_id']
                assert not any(any(b.get('tool_use_id') == tool_id for b in r.get('message', {}).get('content', []) if isinstance(b, dict)) for r in records[:pause['records_before_deny']] if r.get('type') == 'user')
            assert b['parentUuid'] == a['uuid']
        else:
            assert not callbacks
            assert all('No such tool available: probe' in r['message']['content'][0]['content'] for r in results)
            assert ar['parentUuid'] == a['uuid'] and b['parentUuid'] == ar['uuid']
        evidence.append({'case': label, 'callbacks': len(callbacks), 'requests': 2, 'A': a['uuid'], 'resultA': ar['uuid'], 'B': b['uuid'], 'B_parent': b['parentUuid']})
    return evidence


if __name__ == '__main__':
    print(json.dumps(verify(Path(sys.argv[1])), indent=2))
