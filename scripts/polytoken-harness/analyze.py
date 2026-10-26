#!/usr/bin/env python3
"""Check offline recordings and report adjacent request context differences."""
import argparse
import datetime
import difflib
import json
from pathlib import Path
import re


def pretty(value):
    return ((value if isinstance(value, str) else json.dumps(value, indent=2, ensure_ascii=False)) + '\n').splitlines(keepends=True)


def tool_changes(before, after):
    a = {t['name']: t for t in before}
    b = {t['name']: t for t in after}
    return dict(added=sorted(b.keys() - a.keys()), removed=sorted(a.keys() - b.keys()),
                changed=sorted(n for n in a.keys() & b.keys() if a[n] != b[n]),
                order_only=a == b and before != after)


def diff(a, b, label):
    text = ''.join(difflib.unified_diff(pretty(a), pretty(b), fromfile='before/' + label, tofile='after/' + label))
    print('\n```diff\n' + (text or '# empty diff\n') + '```\n')


def analyze(path):
    rows = [json.loads(line) for line in path.read_text().splitlines()]
    main = [r for r in rows if r['label'] == 'main' and r['path'] == '/v1/messages']
    expected = {'exec': 2, 'daemon': 3, 'facet': 4, 'goal': 3, 'reveal': 4}
    assert len(main) == expected[path.stem], (path, len(main))
    blocks = [b for m in main[1]['body']['messages'] for b in m['content']]
    assert dict(type='thinking', thinking='I will read the harmless fixture.', signature='harness-signature-v1') in blocks
    assert dict(type='tool_use', id='read_1', name='file_read', input={'path': 'hello.txt'}) in blocks
    assert any(b.get('type') == 'tool_result' and b.get('tool_use_id') == 'read_1' and b.get('is_error') is False and 'hello from the harness' in b['content'] for b in blocks)
    print(f'## {path.name}\n')
    print('Endpoints: ' + ', '.join(f"{r['method']} {r['path']} ({r['label']})" for r in rows) + '\n')
    for row in main:
        system = json.dumps(row['body'].get('system'))
        dates = sorted(set(re.findall(r'\b\d{4}-\d{2}-\d{2}\b', system)))
        raw_system = row['body'].get('system', '')
        system_text = raw_system if isinstance(raw_system, str) else '\n'.join(b.get('text', '') for b in raw_system if isinstance(b, dict))
        temporal = [line for line in system_text.splitlines() if re.search(r'\bdate:|current date|today|timezone|current time|daemon pid|session:', line, re.I)]
        print(f"Request {row['index']} system: today's date ({datetime.date.today()}) present={str(datetime.date.today()) in system}; dates={dates}; time-related lines={temporal!r}\n")
    for before, after in zip(main, main[1:]):
        print(f"### Requests {before['index']} → {after['index']}\n")
        for field in ('system', 'tools', 'model'):
            equal = before['wire_field_sha256'][field] == after['wire_field_sha256'][field]
            print(f'- `{field}`: original wire bytes ' + ('identical' if equal else 'DIFFERENT'))
        for field in ('x-polytoken-session', 'x-polytoken-conversation'):
            print(f'- `{field}`: ' + ('identical' if before['headers'].get(field) == after['headers'].get(field) else 'DIFFERENT'))
        print('- Tools: ' + json.dumps(tool_changes(before['body'].get('tools', []), after['body'].get('tools', []))))
        for field in ('system', 'tools', 'model'):
            diff(before['body'].get(field), after['body'].get(field), field)
        diff(before['headers'], after['headers'], 'headers')
    print('Continuation assertions: PASS (thinking, signature, tool_use, successful tool_result).\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('recordings', nargs='+', type=Path)
    args = parser.parse_args()
    for path in args.recordings:
        analyze(path)
