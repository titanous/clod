#!/usr/bin/env python3
"""Measurement CLI wrapper / synthetic offline CLI. Never records environment."""
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid
import urllib.request


def emit(value):
    print(json.dumps(value), flush=True)


def main():
    if sys.argv[1:] == ['--version']:
        print('2.1.308 (measurement wrapper)')
        return
    run = uuid.uuid4().hex
    root = Path(os.environ['CACHE_RAW'])
    root.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    env.pop('CLAUDE_CONFIG_DIR', None)
    env['ANTHROPIC_BASE_URL'] = env['ANTHROPIC_BASE_URL'].rstrip('/') + '/__run/' + run
    # This file stays in the gitignored raw tree. No environment or CLI auth output.
    log = root / ('cli-' + run + '.ndjson')
    def record(kind, value):
        with log.open('a') as f:
            f.write(json.dumps(dict(kind=kind, run=run, value=value)) + '\n')
    record('argv', sys.argv[1:])
    if os.environ.get('CACHE_FAKE') != '1':
        import threading
        process = subprocess.Popen([os.environ['CACHE_REAL_CLAUDE'], *sys.argv[1:]], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        def stdout():
            with (root / ('stdout-' + run + '.ndjson')).open('wb') as capture:
                for line in process.stdout:
                    capture.write(line)
                    capture.flush()
                    sys.stdout.buffer.write(line)
                    sys.stdout.buffer.flush()
        output = threading.Thread(target=stdout)
        output.start()
        try:
            for line in sys.stdin:
                value = json.loads(line)
                if value.get('type') == 'control_request' and value.get('request', {}).get('subtype') == 'initialize':
                    record('initialize', value)
                if value.get('type') == 'user':
                    record('user', value)
                process.stdin.write(line.encode())
                process.stdin.flush()
        except BrokenPipeError:
            pass
        finally:
            process.stdin.close()
        sys.exit(process.wait())
    init = json.loads(sys.stdin.readline())
    user = json.loads(sys.stdin.readline())
    record('initialize', init)
    record('user', user)
    emit(dict(type='control_request', request_id='tools', request=dict(subtype='mcp_message', server_name='oc', message=dict(jsonrpc='2.0', method='tools/list', id=1))))
    response = json.loads(sys.stdin.readline())
    tools = response['response']['response']['mcp_response']['result']['tools']
    tools = [dict(name='mcp__oc__' + t['name'] if not t['name'].startswith('mcp__oc__') else t['name'], description=t.get('description', ''), input_schema=t['inputSchema']) for t in tools]
    model = sys.argv[sys.argv.index('--model') + 1]
    body = dict(model=model, max_tokens=64, stream=True, tools=tools, system=init['request']['systemPrompt'], messages=[dict(role='user', content=user['message']['content'])])
    req = urllib.request.Request(env['ANTHROPIC_BASE_URL'] + '/v1/messages', data=json.dumps(body).encode(), headers={'Content-Type': 'application/json'})
    blocks = []
    with urllib.request.urlopen(req, timeout=20) as response:
        for line in response:
            if line.startswith(b'data: '):
                event = json.loads(line[6:])
                if event['type'] == 'content_block_start':
                    blocks.append(event['content_block'])
                elif event['type'] == 'content_block_delta':
                    delta = event['delta']
                    if delta['type'] == 'text_delta':
                        blocks[event['index']]['text'] += delta['text']
                    elif delta['type'] == 'input_json_delta':
                        block = blocks[event['index']]
                        block['_json'] = block.get('_json', '') + delta['partial_json']
                elif event['type'] == 'content_block_stop':
                    block = blocks[event['index']]
                    if '_json' in block:
                        block['input'] = json.loads(block.pop('_json'))
                emit(dict(type='stream_event', event=event, session_id=run))
    emit(dict(type='assistant', uuid=run, session_id=run, message=dict(role='assistant', content=blocks)))
    tool_turn = any(b['type'] == 'tool_use' for b in blocks)
    emit(dict(type='result', subtype='error_max_turns' if tool_turn else 'success', is_error=tool_turn, session_id=run, usage=dict(input_tokens=10, output_tokens=20)))
    for _ in sys.stdin:
        pass


if __name__ == '__main__':
    main()
