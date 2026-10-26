#!/usr/bin/env python3
"""Explicitly authorized direct-HTTP investigation; raw evidence stays local."""
import base64
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import uuid
import zlib

REPO = Path(__file__).resolve().parent.parent


def wrapper():
    root = Path(os.environ['LIVE_RAW'])
    run = uuid.uuid4().hex
    env = dict(os.environ, HOME=os.environ['LIVE_AUTH_HOME'])
    env.pop('CLAUDE_CONFIG_DIR', None)
    env['ANTHROPIC_BASE_URL'] += '/__run/' + run
    args = sys.argv[1:]
    probe = json.loads(os.environ['LIVE_FORK_PROBE']) if os.environ.get('LIVE_FORK_PROBE') else None
    if probe and '--version' not in args:
        args += ['--resume', probe['session'], '--fork-session', '--resume-session-at', probe['uuid']]
    with (root / ('argv-' + run + '.json')).open('w') as f:
        json.dump(args, f)
    child = subprocess.Popen([os.environ['LIVE_CLAUDE'], *args], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    def output():
        with (root / ('stdout-' + run + '.ndjson')).open('wb') as f:
            for line in child.stdout:
                f.write(line); f.flush()
                sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
    thread = threading.Thread(target=output)
    thread.start()
    try:
        with (root / ('stdin-' + run + '.ndjson')).open('wb') as f:
            for line in sys.stdin.buffer:
                value = json.loads(line)
                if probe and value.get('type') == 'user':
                    value['message']['content'] = probe['content']
                    line = (json.dumps(value) + '\n').encode()
                f.write(line); f.flush()
                child.stdin.write(line); child.stdin.flush()
    except BrokenPipeError:
        pass
    finally:
        child.stdin.close()
    code = child.wait()
    thread.join()
    sys.exit(code)


def png():
    def chunk(kind, value):
        return struct.pack('!I', len(value)) + kind + value + struct.pack('!I', zlib.crc32(kind + value))
    pixels = b''.join(b'\0' + b''.join(bytes((255, 0, 0) if x < 32 else (0, 0, 255)) for x in range(64)) for _ in range(64))
    return base64.b64encode(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!IIBBBBB', 64, 64, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(pixels)) + chunk(b'IEND', b'')).decode()


def main():
    assert os.environ.get('OPERATOR_APPROVED_LIVE_VERIFY') == '1'
    assert os.environ.get('CLOD_LIVE') == '1'
    root = REPO / 'scripts/captures' / ('live-verify-' + time.strftime('%Y%m%d-%H%M%S'))
    root.mkdir(parents=True, exist_ok=False)
    state = Path(tempfile.mkdtemp(prefix='clod-live-verify-')).resolve()
    work = state / 'workdir'
    work.mkdir()
    home = state / 'home'
    home.mkdir()
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / 'config'), XDG_STATE_HOME=str(home / 'state'), LIVE_RAW=str(root), LIVE_AUTH_HOME=os.environ['HOME'], LIVE_CLAUDE=shutil.which('claude'))
    env.pop('CLAUDE_CONFIG_DIR', None)
    env.update(CLOD_UPSTREAM_LOG=str(root / 'upstream.ndjson'), CLOD_UPSTREAM_PORT='0', CLOD_CAPTURE_LIMIT=os.environ.get('LIVE_LIMIT', '30'))
    if os.environ.get('LIVE_PROBE') == '1':
        env['LIVE_FORK_PROBE'] = json.dumps(dict(session='c7987a7f-87d2-42d9-922c-cba4b9d45eb2', uuid='e14cdb3d-9e7d-4691-a75d-de11233106ac', content=[dict(type='tool_result', tool_use_id='toolu_01Qva23KkpYqaUuuEhBRHNgd', content='REAL_BARE_VALUE'), dict(type='text', text='Report the real result. No tools.')]))
    proxy = subprocess.Popen(['node', 'scripts/capture-upstream.mjs'], cwd=REPO, env=env, stdout=subprocess.PIPE, text=True)
    port = int(proxy.stdout.readline())
    env.update(CLOD_LISTEN='127.0.0.1:0', CLOD_WORKDIR=str(work), CLOD_MEASURE_LOG=str(root / 'measure.ndjson'), CLOD_UPSTREAM_BASE_URL='http://127.0.0.1:' + str(port), CLOD_CLAUDE_BIN=str(Path(__file__).resolve()), LIVE_WRAPPER='1', RUST_LOG='info')
    for key in ('CLOD_API_KEY', 'CLOD_SESSION_KEY'):
        env.pop(key, None)
    meta = dict(root=str(root), state=str(state), workdir=str(work), cleanup=str(Path(os.environ['HOME']) / '.claude/projects' / re.sub(r'[^a-zA-Z0-9-]', '-', str(work))))
    (root / 'setup.json').write_text(json.dumps(meta, indent=2))
    print(json.dumps(meta), flush=True)
    err = (root / 'clod.stderr').open('w')
    clod = subprocess.Popen([os.environ.get('LIVE_CLOD_BIN', str(REPO / 'target/debug/clod'))], cwd=REPO, env=env, stdout=subprocess.PIPE, stderr=err, text=True)
    try:
        while True:
            line = clod.stdout.readline()
            if not line:
                raise RuntimeError('clod startup failed')
            if 'address=' in line:
                address = line.split('address=')[1].strip()
                break
        def call(label, body):
            (root / (label + '-request.json')).write_text(json.dumps(body))
            req = urllib.request.Request('http://' + address + '/v1/messages', data=json.dumps(body).encode(), headers={'Content-Type':'application/json', 'X-Polytoken-Conversation':label.split('-')[0]})
            with urllib.request.urlopen(req, timeout=240) as response:
                text = response.read().decode()
            (root / (label + '-response.txt')).write_text(text)
            if body.get('stream'):
                blocks = []
                for line in text.splitlines():
                    if not line.startswith('data: '):
                        continue
                    event = json.loads(line[6:])
                    if event['type'] == 'content_block_start':
                        blocks.append(event['content_block'])
                    if event['type'] == 'content_block_delta':
                        block = blocks[event['index']]
                        delta = event['delta']
                        for field in ('text', 'thinking', 'signature'):
                            if field in delta:
                                block[field] = block.get(field, '') + delta[field]
                        if 'partial_json' in delta:
                            block['_json'] = block.get('_json', '') + delta['partial_json']
                    if event['type'] == 'error':
                        raise RuntimeError(str(event))
                for block in blocks:
                    if '_json' in block:
                        block['input'] = json.loads(block.pop('_json') or '{}')
            else:
                reply = json.loads(text)
                blocks = reply['content']
            body['messages'].append(dict(role='assistant', content=blocks))
            print(label, [(b['type'], len(b.get('thinking', '')), len(b.get('signature', '')), b.get('name'), b.get('text', '')[:120]) for b in blocks], flush=True)
            return blocks
        def body(model='haiku', **extra):
            return dict(model=model, max_tokens=4096, stream=False, system='Follow user instructions precisely. Use tools only when asked.', messages=[], **extra)
        if os.environ.get('LIVE_MIXED') == '1':
            tools = [dict(name=name, description='Look up a test key', input_schema=dict(type='object', properties=dict(key=dict(type='string')), required=['key'])) for name in ('lookup', 'other')]
            b = body(tools=tools)
            b['system'] = 'Follow exact wire tool names requested by the user, without normalizing prefixes.'
            b['messages'] = [dict(role='user', content='In parallel, emit exactly two tool calls: bare name lookup with key bare, and prefixed name mcp__oc__other with key prefixed. Do not prefix lookup. No text.')]
            call('mixed-1', b)
            b = body(tools=tools)
            b['messages'] = [dict(role='user', content='Call mcp__oc__lookup with key image, then wait for its image result.')]
            blocks = call('toolimage-1', b)
            calls = [x for x in blocks if x['type'] == 'tool_use']
            if calls:
                image = dict(type='image', source=dict(type='base64', media_type='image/png', data=png()))
                b['messages'].append(dict(role='user', content=[dict(type='tool_result', tool_use_id=x['id'], content=[image]) for x in calls] + [dict(type='text', text='Name the two colors from left to right. No tools.')]))
                call('toolimage-2', b)
            return
        if os.environ.get('LIVE_THINKING') == '1':
            tools = [dict(name='lookup', description='Look up a test key', input_schema=dict(type='object', properties=dict(key=dict(type='string')), required=['key']))]
            b = body('sonnet', thinking=dict(type='enabled', budget_tokens=4096), tools=tools)
            b['max_tokens'] = 8192
            b['messages'] = [dict(role='user', content='Think carefully: determine the smallest n > 10000 with n mod 17=9, n mod 19=13, n mod 23=4. Explain briefly. No tools.')]
            call('deep-1', b)
            b['thinking'] = dict(type='adaptive'); b['stream'] = True
            b['messages'].append(dict(role='user', content='Now solve a new problem carefully before calling lookup: find the smallest positive integer x with x mod 29=11, x mod 31=17, x mod 37=23. Call lookup with the decimal answer as key, no final answer yet.'))
            blocks = call('deep-2', b)
            calls = [x for x in blocks if x['type'] == 'tool_use']
            if calls:
                b['stream'] = False
                b['messages'].append(dict(role='user', content=[dict(type='tool_result', tool_use_id=x['id'], content='DEEP_REAL_VALUE') for x in calls] + [dict(type='text', text='Report the result; no more tools.')]))
                call('deep-3', b)
            return
        if os.environ.get('LIVE_PROBE') == '1':
            b = body(tools=[dict(name='lookup', description='Look up a test key', input_schema=dict(type='object', properties=dict(key=dict(type='string')), required=['key']))])
            b['system'] = 'Call the tool named exactly lookup when requested. Do not add any prefix.'
            b['messages'] = [dict(role='user', content='Probe input replaced by dedicated wrapper.')]
            call('barefork-1', b)
            return
        image = dict(type='image', source=dict(type='base64', media_type='image/png', data=png()))
        b = body(); b['messages'] = [dict(role='user', content='Remember image-cobalt. Reply READY.')]
        call('image-1', b)
        b['messages'] += [dict(role='user', content=[image]), dict(role='user', content='Name the two colors, left to right, and the marker.')]
        call('image-2', b)
        b['messages'].append(dict(role='user', content='Repeat the colors and marker.'))
        call('image-3', b)
        if os.environ.get('LIVE_IMAGE') == '1':
            return
        tools = [dict(name='lookup', description='Look up a test key', input_schema=dict(type='object', properties=dict(key=dict(type='string')), required=['key']))]
        for i, prompt in enumerate(['Call the tool named exactly lookup, without mcp__oc__ prefix, key alpha. Emit a tool call, not text.', 'The required wire tool name is lookup. Do not use mcp__oc__lookup. Call lookup with key beta.']):
            b = body(tools=tools); b['system'] = 'Call the tool named exactly lookup when requested. Do not add any prefix.'; b['messages'] = [dict(role='user', content=prompt)]
            call('bare' + str(i) + '-1', b)
        b = body('sonnet', thinking=dict(type='enabled', budget_tokens=4096), tools=tools)
        b['max_tokens'] = 8192; b['stream'] = True
        b['messages'] = [dict(role='user', content='Think carefully: find the smallest positive integer n such that n mod 7=3, n mod 11=5, and n mod 13=7. Explain briefly after thinking. No tools yet.')]
        call('thinking-1', b)
        b['stream'] = False; b['messages'].append(dict(role='user', content='Verify the result independently with careful reasoning. No tools.'))
        call('thinking-2', b)
        b['thinking'] = dict(type='adaptive'); b['stream'] = True
        b['messages'].append(dict(role='user', content='Think carefully, then call lookup once with key thinking-check. Do not answer yet.'))
        blocks = call('thinking-3', b)
        calls = [x for x in blocks if x['type'] == 'tool_use']
        if calls:
            b['stream'] = False; b['messages'].append(dict(role='user', content=[dict(type='tool_result', tool_use_id=x['id'], content='THINKING_VALUE') for x in calls] + [dict(type='text', text='Think briefly, then report the returned value and original integer. No more tools.')]))
            call('thinking-4', b)
    finally:
        clod.terminate()
        try:
            clod.wait(timeout=20)
        except subprocess.TimeoutExpired:
            clod.kill(); clod.wait()
        proxy.terminate(); proxy.wait(timeout=10)
        err.close()
        print('Retained evidence and cleanup folders:', json.dumps(meta), flush=True)
        records = [json.loads(line) for line in (root / 'upstream.ndjson').read_text().splitlines()]
        assert not any(r.get('kind') in ('upstream_error', 'ceiling') for r in records), 'upstream error or ceiling reached'
        upstream = [r['line']['body'] for r in records if r.get('dir') == 'upstream' and '/v1/messages' in r['line']['path']]
        measure = [json.loads(line) for line in (root / 'measure.ndjson').read_text().splitlines()]
        plans = [r['plan'] for r in measure if r.get('kind') == 'spawn']
        if os.environ.get('LIVE_IMAGE') == '1':
            assert plans == ['fresh', 'resume', 'resume'], plans
            assert len(upstream) == 3
            last = upstream[1]['messages'][-1]['content']
            assert last[0]['type'] == 'image' and last[0]['source']['data'] == png()
            assert last[1]['text'].rstrip('\n') == 'Name the two colors, left to right, and the marker.'
            assert len(last) == 3 and last[2]['text'].startswith('[Image: source: ')
            assert upstream[2]['messages'][-3]['content'][0]['source']['data'] == png()
        if os.environ.get('LIVE_THINKING') == '1':
            assert plans == ['fresh', 'resume', 'resume'], plans
            assert len(upstream) == 3
            first = json.loads((root / 'deep-1-response.txt').read_text())['content']
            signed = [x for x in first if x['type'] == 'thinking']
            assert signed and signed[0]['thinking'] and signed[0]['signature']
            wire = (root / 'deep-2-response.txt').read_text()
            assert 'thinking_delta' in wire and 'signature_delta' in wire
            events = [json.loads(line[6:]) for line in wire.splitlines() if line.startswith('data: ')]
            starts = [e['content_block']['type'] for e in events if e['type'] == 'content_block_start']
            assert starts.index('thinking') < starts.index('tool_use'), starts
            deltas = [e['delta'] for e in events if e.get('delta', {}).get('type') in ('thinking_delta', 'signature_delta')]
            cli_deltas = []
            for capture in root.glob('stdout-*.ndjson'):
                cli_events = [r['event'] for r in map(json.loads, capture.read_text().splitlines()) if r.get('type') == 'stream_event']
                cli_deltas.append([e['delta'] for e in cli_events if e.get('delta', {}).get('type') in ('thinking_delta', 'signature_delta')])
            assert deltas in cli_deltas, 'thinking/signature deltas changed between CLI and client'
            streamed_thinking = ''.join(e['delta'].get('thinking', '') for e in events if e.get('delta', {}).get('type') == 'thinking_delta')
            streamed_signature = ''.join(e['delta'].get('signature', '') for e in events if e.get('delta', {}).get('type') == 'signature_delta')
            final_history = [x for m in upstream[2]['messages'] if isinstance(m.get('content'), list) for x in m['content'] if x.get('type') == 'thinking']
            assert any(x.get('thinking') == streamed_thinking and x.get('signature') == streamed_signature for x in final_history)
            for body in upstream[1:]:
                history = [x for m in body['messages'] if isinstance(m.get('content'), list) for x in m['content'] if x.get('type') == 'thinking']
                assert any(x.get('thinking') == signed[0]['thinking'] and x.get('signature') == signed[0]['signature'] for x in history)
            assert 'DEEP_REAL_VALUE' in (root / 'deep-3-response.txt').read_text()
        print('Verified upstream attempts:', len(upstream), 'plans:', plans, flush=True)


if __name__ == '__main__':
    if os.environ.get('LIVE_WRAPPER') == '1':
        wrapper()
    else:
        main()
