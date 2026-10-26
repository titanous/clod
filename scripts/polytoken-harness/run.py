#!/usr/bin/env python3
"""Run the real Polytoken with an isolated home and scripted local provider."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.request

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('harness_mock', HERE / 'mock.py')
mock = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mock)


def scenario_script(mode):
    if mode == 'image':
        return [[dict(type='text', text='READY')] for _ in range(3)]
    first = [dict(type='thinking', thinking='I will read the harmless fixture.', signature='harness-signature-v1'),
             dict(type='text', text='Reading the fixture.'),
             dict(type='tool_use', id='read_1', name='file_read', input={'path': 'hello.txt'})]
    final = [dict(type='text', text='The fixture says hello from the harness.')]
    if mode == 'exec':
        final.append(dict(type='tool_use', id='complete_1', name='exec_structured_output', input={'output': 'harness complete'}))
        return [first, final]
    script = [first, final, [dict(type='text', text='Second top-level prompt complete.')]]
    if mode == 'reveal':
        script.insert(1, [dict(type='tool_use', id='search_1', name='tool_search', input={'query': 'file_write', 'max_results': 1})])
    if mode == 'facet':
        script.append([dict(type='text', text='Third top-level prompt complete.')])
    return script


def request(base, token, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            payload = r.read()
            return json.loads(payload) if payload else None
    except urllib.error.HTTPError as error:
        raise RuntimeError(f'Control {path} HTTP {error.code}: {error.read().decode(errors="replace")}') from None


def scrub(text, root, port):
    for source in sorted({str(root), str(root.resolve())}, key=len, reverse=True):
        text = text.replace(source, '<RUN>')
    text = text.replace('127.0.0.1:' + str(port), '127.0.0.1:<MOCK_PORT>')
    return text


def run(args, mode):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = shutil.which(args.binary)
    if not binary:
        raise RuntimeError('Polytoken binary not found; install or build it first')
    version = subprocess.check_output([binary, '--version'], text=True).strip()
    with tempfile.TemporaryDirectory(prefix='polytoken-harness-') as temp:
        root = Path(temp)
        work = root / 'work'
        work.mkdir()
        (work / 'hello.txt').write_text('hello from the harness\n')
        env = {k: v for k, v in os.environ.items() if not k.startswith(('POLYTOKEN_', 'ANTHROPIC_', 'OPENAI_', 'AWS_', 'OTEL_', 'SENTRY_')) and k not in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME')}
        for key, folder in [('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'), ('XDG_CACHE_HOME', 'cache')]:
            target = root / folder
            target.mkdir()
            env[key] = str(target)
        env.update(POLYTOKEN_SKIP_UPDATE_CHECK='1', NO_PROXY='127.0.0.1,localhost', no_proxy='127.0.0.1,localhost')
        raw = root / 'requests.ndjson'
        raw.touch()
        server, state = mock.serve(scenario_script(mode), raw)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        provider = args.provider_url or f'http://127.0.0.1:{server.server_port}'
        config = root / 'config' / 'polytoken'
        config.mkdir()
        (config / 'config.toml').write_text(f'''defaults.full = "m1"
[providers.gateway]
kind = {{ type = "custom_anthropic_compatible" }}
url = {json.dumps(provider)}
auth = {{ type = "static_key", key = "test-key", format = "anthropic_x_api_key" }}
[models.m1]
provider_name = {json.dumps(getattr(args, 'model', 'claude-test'))}
variant = "claude"
provider = "gateway"
class = "full"
context_window = 200000
{('supports_vision = true' if mode == 'image' else '')}
{('tool_loading = "native_deferred"' if mode == 'reveal' else '')}
''')
        if mode == 'model':
            with (config / 'config.toml').open('a') as f:
                f.write('\n[models.m2]\nprovider_name = "claude-opus-5-5"\nvariant = "claude"\nprovider = "gateway"\nclass = "full"\ncontext_window = 200000\n')
        base_cmd = [binary, '--config-dir', str(config), '--working-dir', str(work)]
        logs = []
        daemon = None
        token = None
        session_id = None
        try:
            command = base_cmd + (['exec', 'Read hello.txt and report completion.'] if mode == 'exec' else ['new', '--no-attach'])
            result = subprocess.run(command, env=env, cwd=work, text=True, capture_output=True, timeout=args.timeout)
            logs.append({'command': command, 'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
            if result.returncode:
                raise RuntimeError(result.stderr)
            if mode != 'exec':
                match = re.search(r'session_id=(\S+) port=(\d+)', result.stdout)
                if not match:
                    raise RuntimeError('No daemon identity: ' + result.stdout)
                session_id, port = match.groups()
                daemon = 'http://127.0.0.1:' + port
                credentials = list(root.rglob('credential.json'))
                credential = next(p for p in credentials if session_id in str(p))
                token = json.loads(credential.read_text())['token']
                starting_facet = request(daemon, token, '/state').get('active_facet', 'execute')
                if isinstance(starting_facet, dict):
                    starting_facet = starting_facet['name']
                prompts = ['Read hello.txt and report what it says.', 'This is the second top-level prompt. Reply briefly.']
                if mode == 'facet':
                    prompts.append('This is the third top-level prompt. Reply briefly.')
                extended = getattr(args, 'extended', False)
                if extended:
                    prompts = ['Reply briefly with READY. Do not use tools.'] * (3 if mode == 'image' else (6 if mode in ('facet','goalclear') else 4))
                if getattr(args, 'canary', False):
                    prompts = ['Reply with exactly CANARY_OK. Do not use any tools.']
                if mode == 'reveal' and extended:
                    prompts[2] = 'Use mcp__oc__tool_search to reveal file_read with query file_read and max_results 1. Do not read a file. Then say REVEALED.'
                for turn, prompt in enumerate(prompts):
                    if mode == 'facet' and ((extended and turn in (2,4)) or (not extended and turn)):
                        target = ('plan' if starting_facet != 'plan' else 'execute') if turn == (2 if extended else 1) else starting_facet
                        logs.append({'facet_switch': target, 'response': request(daemon, token, '/facet', {'facet': target})})
                    if mode == 'daemonrestart' and turn == 2:
                        request(daemon, token, '/terminate', {})
                        def old_daemon_alive():
                            try: request(daemon, token, '/state'); return True
                            except Exception: return False
                        deadline=time.monotonic()+10
                        while old_daemon_alive():
                            if time.monotonic()>deadline: raise TimeoutError('old daemon did not terminate')
                            time.sleep(.05)
                        restarted=subprocess.run(base_cmd+['continue','--no-attach',session_id],env=env,cwd=work,text=True,capture_output=True,timeout=args.timeout)
                        logs.append({'restart_exit':restarted.returncode})
                        if restarted.returncode: raise RuntimeError('isolated daemon continue failed: '+restarted.stderr)
                        match=re.search(r'port=(\d+)',restarted.stdout)
                        if not match: raise RuntimeError('continued daemon did not report port')
                        daemon='http://127.0.0.1:'+match.group(1)
                        token=json.loads(credential.read_text())['token']
                    if mode == 'clodrestart' and turn == 2:
                        signal = Path(os.environ['CACHE_RESTART_SIGNAL'])
                        signal.write_text('restart')
                        deadline = time.monotonic()+15
                        while signal.exists():
                            if time.monotonic()>deadline: raise TimeoutError('clod restart acknowledgment missing')
                            time.sleep(.05)
                    if mode == 'effort' and turn == 2:
                        logs.append({'effort': request(daemon, token, '/model/options', {'type':'set','key':'reasoning_effort','value':'high'})})
                    if mode == 'model' and turn == 2:
                        logs.append({'model_switch': request(daemon, token, '/model', {'model':'m2'})})
                    if mode == 'goalclear' and turn == 4:
                        logs.append({'goal_clear': request(daemon, token, '/goal/clear', {})})
                    if mode in ('goal','goalclear') and turn == (2 if extended else 1):
                        logs.append({'goal_set': request(daemon, token, '/goal', {'summary': 'Report the fixture contents, then wait for instructions.'})})
                        # Keep goal context but disable automatic goal-driver turns.
                        logs.append({'goal_pause': request(daemon, token, '/goal/pause', {})})
                    images = []
                    if mode == 'image' and turn == 1:
                        import base64, struct, zlib
                        def chunk(kind, data):
                            return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
                        pixels=b''.join(b'\x00'+b''.join(bytes((255,40,40) if (x//8+y//8)%2 else (30,90,255)) for x in range(64)) for y in range(64))
                        png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',64,64,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')
                        images = [{'media_type':'image/png','data':base64.b64encode(png).decode()}]
                    accepted = request(daemon, token, '/prompt', {'content': prompt, 'images': images, 'max_tool_turns': 4})
                    deadline = time.monotonic() + args.timeout
                    while True:
                        snapshot = request(daemon, token, '/state')
                        # 0.9.2 omits this serde-default boolean when idle.
                        if not snapshot.get('turn_in_flight', False):
                            break
                        if time.monotonic() > deadline:
                            raise TimeoutError('Daemon turn did not finish')
                        time.sleep(.05)
                    latest = snapshot.get('most_recent_assistant_text', snapshot.get('latest_assistant_text'))
                    logs.append({'prompt': prompt, 'accepted': accepted, 'latest_assistant_text': latest})
                    if args.provider_url and (not latest or "wasn't able to produce a visible response" in latest):
                        raise RuntimeError('Provider produced no usable visible response; stopping scenario')
            if not args.provider_url and (state['main'] != len(scenario_script(mode)) or state['errors']):
                raise RuntimeError('Script not consumed exactly: ' + repr(state))
        finally:
            if daemon and token:
                try:
                    request(daemon, token, '/terminate', {})
                except Exception as error:
                    logs.append({'termination_error': str(error)})
            server.shutdown()
            server.server_close()
            records = [json.loads(line) for line in raw.read_text().splitlines()]
            identities = {}
            for record in records:
                for header in ('x-polytoken-session', 'x-polytoken-conversation'):
                    value = record['headers'].get(header)
                    if value:
                        identities.setdefault(value, f'<IDENTITY_{len(identities) + 1}>')
                        record['headers'][header] = identities[value]
            text = '\n'.join(json.dumps(r, separators=(',', ':')) for r in records) + ('\n' if records else '')
            metadata = dict(version=version, scenario=mode, provider_url=provider, requests=len(records), logs=logs)
            metadata_text = json.dumps(metadata, indent=2)
            if session_id:
                identities.setdefault(session_id, '<DAEMON_SESSION>')
            for identity, placeholder in identities.items():
                text = text.replace(identity, placeholder)
                metadata_text = metadata_text.replace(identity, placeholder)
            (output / f'{mode}.ndjson').write_text(scrub(text, root, server.server_port))
            (output / f'{mode}.run.json').write_text(scrub(metadata_text, root, server.server_port) + '\n')
    print(f'{mode}: {version}; {len(records)} recorded requests; {output}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='polytoken')
    parser.add_argument('--model', default='claude-test')
    parser.add_argument('--canary', action='store_true')
    parser.add_argument('--extended', action='store_true', help='Measurement-only warm/change/stable prompts')
    parser.add_argument('--scenario', choices=['exec', 'daemon', 'facet', 'goal', 'goalclear', 'model', 'effort', 'clodrestart', 'daemonrestart', 'image', 'reveal', 'both', 'all'], default='both')
    parser.add_argument('--output', type=Path, default=HERE / 'output')
    parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('--provider-url', help='Explicit provider override, e.g. local clod proxy. May make live calls! Local mock is the default.')
    args = parser.parse_args()
    for mode in (['exec', 'daemon', 'facet', 'goal', 'reveal'] if args.scenario == 'all' else ['exec', 'daemon'] if args.scenario == 'both' else [args.scenario]):
        run(args, mode)
