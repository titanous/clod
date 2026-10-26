#!/usr/bin/env python3
"""Analyze paired measurement captures. Origin labels describe bytes, not causality."""
import argparse
import difflib
import json
from pathlib import Path


def markers(value, path=''):
    result = []
    if isinstance(value, dict):
        if 'cache_control' in value:
            result.append({'path': path, 'value': value['cache_control']})
        for key, child in value.items():
            result += markers(child, path + '/' + key)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            result += markers(child, path + '/' + str(index))
    return result


def rows(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def text(value):
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return '\n'.join(b.get('text', '') if isinstance(b, dict) else b for b in value)
    return json.dumps(value, sort_keys=True)


def at(body, path):
    value = body
    try:
        for part in path.strip('/').split('/'):
            value = value[int(part)] if isinstance(value, list) else value[part]
        return value
    except (KeyError, IndexError, TypeError, ValueError):
        return None


def origin(section, old, new, incoming_old, incoming_new):
    a, b = at(old, section), at(new, section)
    if section == '/tools':
        changed = incoming_old.get('tools') != incoming_new.get('tools')
        return dict(origin='Polytoken' if changed else 'CLI', confidence='inferred mapping: CLI renames and decorates MCP schemas')
    old_text, new_text = text(a), text(b)
    if 'clod-note' in old_text or 'clod-note' in new_text or 'conversation_history' in old_text or 'conversation_history' in new_text:
        return dict(origin='clod', confidence='measured marker match; may also include Polytoken changes')
    if section.startswith('/system'):
        polya, polyb = text(incoming_old.get('system', '')), text(incoming_new.get('system', ''))
        if polyb and polyb in new_text and polya != polyb:
            return dict(origin='Polytoken', confidence='measured supplied-system substring match')
        return dict(origin='CLI', confidence='inferred: outside supplied system; inspect diff')
    if any(s in old_text + new_text for s in ('# Environment', '# userEmail', 'system-reminder')):
        return dict(origin='CLI', confidence='inferred injected marker match; reminders may also originate in Polytoken')
    return dict(origin='Polytoken', confidence='inferred forwarded conversation content; inspect paired input')


def analyze(root):
    upstream = rows(root / 'upstream.ndjson')
    inputs = [r for r in rows(root / 'incoming.ndjson') if r['kind'] == 'request' and isinstance(r.get('body'), dict)]
    responses = {r['index']: r for r in upstream if r['kind'] == 'response'}
    requests = [r for r in upstream if r['kind'] == 'request' and isinstance(r.get('body'), dict) and 'messages' in r['body']]
    mapped = {}
    for request in requests:
        run = request['cli_run']
        log = rows(root / ('cli-' + run + '.ndjson'))
        init = next(r['value']['request']['systemPrompt'] for r in log if r['kind'] == 'initialize')
        candidates = [r for r in inputs if text(r['body'].get('system', '')) in text(init)]
        # Requests are serialized in these controlled scenarios. Choose latest input before upstream timestamp.
        candidates = [r for r in candidates if r['timestamp'] <= request['timestamp']]
        if not candidates:
            raise ValueError('No incoming request matches CLI initialization')
        mapped[request['index']] = candidates[-1]
    result = []
    for previous, current in zip(requests, requests[1:]):
        a, b = previous['body'], current['body']
        sections = ['/tools']
        for field in ('system', 'messages'):
            x, y = a.get(field, []), b.get(field, [])
            sections += ['/' + field + '/' + str(i) for i in range(max(len(x), len(y)))] if isinstance(x, list) and isinstance(y, list) else ['/' + field]
        equal = {s: previous['wire_sha256'].get(s) == current['wire_sha256'].get(s) for s in sections}
        first = next((s for s in sections if not equal[s]), None)
        usage = {}
        for event in responses.get(current['index'], {}).get('usage_events', []):
            usage.update(event.get('usage') or {})
        changes = []
        ia, ib = mapped[previous['index']]['body'], mapped[current['index']]['body']
        for section in sections:
            if equal[section]:
                continue
            changes.append(dict(section=section, **origin(section, a, b, ia, ib), diff=list(difflib.unified_diff(text(at(a, section)).splitlines(), text(at(b, section)).splitlines()))[:16]))
        argv = next(r['value'] for r in rows(root / ('cli-' + current['cli_run'] + '.ndjson')) if r['kind'] == 'argv')
        expected = None
        result.append(dict(before=previous['index'], after=current['index'], cli_run=current['cli_run'], incoming_request=mapped[current['index']]['index'], resumed='--resume' in argv, same_model=a.get('model') == b.get('model'), section_wire_identical=equal, first_divergence=first, changes=changes, cache_control=markers(b), usage=usage, expected_prefix_cache_read_tokens=expected, expected_prefix_basis='Exact zero common tool prefix' if expected == 0 else 'Unknown: no exact prefix tokenizer/checkpoint accounting; byte length is not a token count.', new_system_present=text(ib.get('system', '')) in text(b.get('system', ''))))
    return dict(requests=[dict(index=r['index'], cli_run=r['cli_run'], incoming_request=mapped[r['index']]['index'], usage=responses.get(r['index'], {}).get('usage_events', [])) for r in requests], comparisons=result)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('recording_directory', type=Path)
    print(json.dumps(analyze(parser.parse_args().recording_directory), indent=2))
