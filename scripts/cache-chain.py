#!/usr/bin/env python3
"""Offline full chain; --live explicitly permits subscription generation."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.request

HERE = Path(__file__).resolve().parent
REPO = HERE.parent

def load(name, path):
    spec = importlib.util.spec_from_file_location(name, HERE / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

recorder = load('recorder', 'cache-recorder.py')
mock = load('mock', 'polytoken-harness/mock.py')


def main(args):
    root = REPO / 'scripts/cache-measurements' / args.name
    root.mkdir(parents=True, exist_ok=False)
    servers = []
    child = None
    proxy_process = None
    with tempfile.TemporaryDirectory(prefix='clod-cache-measure-') as directory:
        work = Path(directory) / 'workdir'
        work.mkdir()
        if args.live:
            target = 'https://api.anthropic.com'
        else:
            script = [[dict(type='tool_use', id='offline_read', name='mcp__oc__file_read', input=dict(path='hello.txt'))], [dict(type='text', text='The file says hello from the harness.')], [dict(type='text', text='Second prompt complete.')]]
            if args.extended:
                script += [[dict(type='text',text='READY')], [dict(type='text',text='READY')]]
            server, _ = mock.serve(script, root / 'mock.ndjson')
            servers.append(server)
            target = f'http://127.0.0.1:{server.server_port}'
        proxy_env = dict(os.environ, CLOD_UPSTREAM_LOG=str(root / 'upstream.ndjson'), CLOD_CAPTURE_TARGET=target, CLOD_UPSTREAM_PORT='0', CLOD_CAPTURE_LIMIT='3' if args.scenario == 'image' else ('4' if args.canary else '7'))
        proxy_process = subprocess.Popen(['node', str(HERE / 'capture-upstream.mjs')], env=proxy_env, stdout=subprocess.PIPE, text=True)
        upstream_port = int(proxy_process.stdout.readline().strip())
        import socket
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            clod_port = sock.getsockname()[1]
        incoming = recorder.serve(f'http://127.0.0.1:{clod_port}', root / 'incoming.ndjson', label='polytoken')
        servers.append(incoming)
        for server in servers:
            threading.Thread(target=server.serve_forever, daemon=True).start()
        env = dict(os.environ)
        env.pop('CLAUDE_CONFIG_DIR', None)
        env.update(CLOD_WORKDIR=str(work), CLOD_LISTEN=f'127.0.0.1:{clod_port}', CLOD_CLAUDE_BIN=str(HERE / 'cache-cli.py'), CLOD_MEASURE_LOG=str(root / 'usage.ndjson'), CLOD_UPSTREAM_BASE_URL=f'http://127.0.0.1:{upstream_port}', CACHE_RAW=str(root), CACHE_REAL_CLAUDE=shutil.which('claude'), CACHE_FAKE='0' if args.live else '1')
        metadata = dict(live=args.live, model=args.model, mode=args.mode, scenario=args.scenario, workdir=str(work), transcript_project=str(Path.home() / '.claude/projects' / str(work).replace('/', '-')), real_home_passed=True)
        (root / 'setup.json').write_text(json.dumps(metadata, indent=2))
        try:
            with (root / 'clod.log').open('w') as log:
                child = subprocess.Popen([str(REPO / 'target/debug/clod')], env=env, stdout=log, stderr=log)
                # Bounded readiness attempts, no generation traffic.
                deadline = time.monotonic() + 10
                while True:
                    try:
                        with urllib.request.urlopen(f'http://127.0.0.1:{clod_port}/health', timeout=1):
                            break
                    except OSError:
                        if time.monotonic() > deadline:
                            raise RuntimeError('clod readiness failed')
                command = ['python3', '-I', str(HERE / 'polytoken-harness/run.py'), '--scenario', args.scenario, '--model', args.model, '--timeout', '180', '--provider-url', f'http://127.0.0.1:{incoming.server_port}', '--output', str(root / 'harness')]
                if args.extended:
                    command.append('--extended')
                if args.canary:
                    command.append('--canary')
                if args.scenario == 'clodrestart':
                    signal = root / 'restart.signal'
                    harness_env = dict(os.environ, CACHE_RESTART_SIGNAL=str(signal))
                    runner = subprocess.Popen(command, env=harness_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    deadline = time.monotonic()+180
                    while not signal.exists() and runner.poll() is None:
                        if time.monotonic()>deadline: raise TimeoutError('restart signal missing')
                        time.sleep(.05)
                    if signal.exists():
                        child.terminate(); child.wait(timeout=10)
                        child = subprocess.Popen([str(REPO / 'target/debug/clod')], env=env, stdout=log, stderr=log)
                        deadline=time.monotonic()+10
                        while True:
                            try:
                                with urllib.request.urlopen(f'http://127.0.0.1:{clod_port}/health',timeout=1): break
                            except OSError:
                                if time.monotonic()>deadline: raise
                        signal.unlink()
                    out, err = runner.communicate(timeout=180)
                    result = subprocess.CompletedProcess(command,runner.returncode,out,err)
                else:
                    result = subprocess.run(command, capture_output=True, timeout=650)
                (root / 'runner.stdout').write_bytes(result.stdout)
                (root / 'runner.stderr').write_bytes(result.stderr)
                if result.returncode:
                    raise RuntimeError('harness failed; inspect raw runner log')
                requests = [json.loads(line) for line in (root / 'upstream.ndjson').read_text().splitlines()]
                main = [r for r in requests if r.get('dir') == 'upstream' and '/v1/messages' in r['line']['path']]
                usage = [json.loads(line) for line in (root / 'usage.ndjson').read_text().splitlines()]
                starts = [r for r in usage if r.get('event') == 'message_start']
                assert main and all(r['cli_run'] for r in main)
                assert starts and all(isinstance(r['usage'].get('input_tokens'), int) for r in starts), 'No authoritative CLI input usage'
                if not args.live:
                    assert len(main) == (5 if args.extended else 3), 'Offline response sequence was not completed'
                    assert any(r.get('plan') == 'resume' for r in usage), 'No resume plan'
                    users = [json.loads(line) for p in root.glob('cli-*.ndjson') for line in p.read_text().splitlines() if json.loads(line)['kind'] == 'user']
                    assert any('tool_result' in json.dumps(r) and 'hello from the harness' in json.dumps(r) for r in users), 'No real tool result delivered'
                if args.canary:
                    metadata = json.loads((root / 'harness/daemon.run.json').read_text())
                    assert any('CANARY_OK' in (r.get('latest_assistant_text') or '') for r in metadata['logs']), 'Canary text missing'
                print(json.dumps(dict(name=args.name, upstream_message_requests=len(main), cli_message_starts=len(starts), live=args.live, canary=args.canary)))
        finally:
            if child:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            if proxy_process:
                proxy_process.terminate()
                proxy_process.wait(timeout=10)
            for server in servers:
                server.shutdown()
                server.server_close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live', action='store_true')
    parser.add_argument('--canary', action='store_true')
    parser.add_argument('--name', required=True)
    parser.add_argument('--mode', choices=['strict'], default='strict')
    parser.add_argument('--extended', action='store_true')
    parser.add_argument('--scenario', choices=['daemon', 'facet', 'goal', 'goalclear', 'model', 'effort', 'clodrestart', 'daemonrestart', 'image', 'reveal'], default='daemon')
    parser.add_argument('--model', default='claude-test')
    main(parser.parse_args())
