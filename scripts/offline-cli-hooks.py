#!/usr/bin/env python3
"""Bounded SDK PreToolUse experiment, exact local CLI pin and mandatory sandbox."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import selectors
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

spec = importlib.util.spec_from_file_location('confinement', Path(__file__).with_name('offline-cli-confinement.py'))
confinement = importlib.util.module_from_spec(spec)
spec.loader.exec_module(confinement)
PIN = '21d6e7b9a6f07b4cfcb7df13760b3b72aa25e084e13a5354413382987ba832ed'


def sse(names):
    events = [{'type': 'message_start', 'message': {'id': 'msg_offline_hooks', 'type': 'message', 'role': 'assistant', 'model': 'claude-haiku-4-5', 'content': [], 'stop_reason': None, 'stop_sequence': None, 'usage': {'input_tokens': 10, 'output_tokens': 0}}}]
    for i, name in enumerate(names):
        events += [{'type': 'content_block_start', 'index': i, 'content_block': {'type': 'tool_use', 'id': 'toolu_hook_' + str(i), 'name': name, 'input': {}}}, {'type': 'content_block_delta', 'index': i, 'delta': {'type': 'input_json_delta', 'partial_json': '{}'}}, {'type': 'content_block_stop', 'index': i}]
    if not names:
        events += [{'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'text', 'text': ''}}, {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'text_delta', 'text': 'Offline complete.'}}, {'type': 'content_block_stop', 'index': 0}]
    events += [{'type': 'message_delta', 'delta': {'stop_reason': 'tool_use' if names else 'end_turn', 'stop_sequence': None}, 'usage': {'output_tokens': 20}}, {'type': 'message_stop'}]
    return ''.join('event: ' + e['type'] + '\ndata: ' + json.dumps(e) + '\n\n' for e in events).encode()


def run(cli, root, matcher, names):
    root.mkdir()
    for d in ['home', 'config', 'cache', 'data', 'state', 'work', 'tmp']:
        (root / d).mkdir()
    (root / 'network.sb').write_text(confinement.PROFILE)
    payload = sse(names)
    (root / 'provider.sse').write_bytes(payload)
    requests = []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass
        def do_HEAD(self):
            self.send_response(200)
            self.end_headers()
        def do_POST(self):
            body = self.rfile.read(int(self.headers.get('content-length', 0)))
            requests.append({'path': self.path, 'body': body.decode()})
            if len(requests) > 2:
                proc.kill()
                self.send_error(400, 'offline request ceiling exceeded; child killed')
                return
            data = payload if len(requests) == 1 else sse([])
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TMPDIR': str(root / 'tmp'), 'ANTHROPIC_BASE_URL': 'http://127.0.0.1:' + str(server.server_port), 'ANTHROPIC_API_KEY': 'offline-fake-key', 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1', 'CLAUDE_CODE_DISABLE_ATTACHMENTS': '1', 'ENABLE_CLAUDEAI_MCP_SERVERS': 'false', 'ENABLE_TOOL_SEARCH': 'false', 'DISABLE_AUTOUPDATER': '1', 'DISABLE_TELEMETRY': '1'}
    env.update({'XDG_' + k.upper() + '_HOME': str(root / k) for k in ['config', 'cache', 'data', 'state']})
    args = ['/usr/bin/sandbox-exec', '-f', str(root / 'network.sb'), str(cli), '--output-format', 'stream-json', '--verbose', '--input-format', 'stream-json', '--max-turns', '2', '--model', 'claude-haiku-4-5', '--allowedTools', 'mcp__oc__probe', '--tools', '', '--setting-sources=', '--strict-mcp-config', '--permission-mode', 'bypassPermissions', '--allow-dangerously-skip-permissions', '--include-partial-messages']
    (root / 'argv.json').write_text(json.dumps(args))
    proc = subprocess.Popen(args, env=env, cwd=root / 'work', stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
    records, hooks, calls = [], [], []
    pending = []
    with (root / 'stdin.ndjson').open('w') as stdin, (root / 'stdout.ndjson').open('wb') as stdout, (root / 'stderr.txt').open('wb') as stderr:
        def send(record):
            line = json.dumps(record) + '\n'
            stdin.write(line)
            stdin.flush()
            proc.stdin.write(line.encode())
        send({'type': 'control_request', 'request_id': 'init', 'request': {'subtype': 'initialize', 'sdkMcpServers': ['oc'], 'systemPrompt': ['Offline hook probe.'], 'hooks': {'PreToolUse': [{'matcher': matcher, 'hookCallbackIds': ['deny_probe'], 'timeout': 5}]}}})
        # Wait for initialize success before sending the user message.
        selector = selectors.DefaultSelector()
        selector.register(proc.stdout, selectors.EVENT_READ, stdout)
        selector.register(proc.stderr, selectors.EVENT_READ, stderr)
        buffers = {proc.stdout: b'', proc.stderr: b''}
        deadline = time.monotonic() + 35
        while selector.get_map() and time.monotonic() < deadline:
            for due, record in list(pending):
                if time.monotonic() >= due:
                    hooks.append({'pause_seconds': 0.3, 'tool_use_id': record['request'].get('tool_use_id'), 'requests_before_deny': len(requests), 'records_before_deny': len(records)})
                    send({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': record['request_id'], 'response': {'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny', 'permissionDecisionReason': 'Offline hook denied execution'}}}})
                    pending.remove((due, record))
            for key, _ in selector.select(timeout=0.05):
                data = os.read(key.fileobj.fileno(), 65536)
                if not data:
                    selector.unregister(key.fileobj)
                    continue
                key.data.write(data)
                key.data.flush()
                if key.fileobj is proc.stderr:
                    continue
                buffers[key.fileobj] += data
                while b'\n' in buffers[key.fileobj]:
                    line, buffers[key.fileobj] = buffers[key.fileobj].split(b'\n', 1)
                    record = json.loads(line)
                    records.append(record)
                    if record.get('type') == 'control_response' and record.get('response', {}).get('request_id') == 'init':
                        if record['response']['subtype'] != 'success':
                            raise RuntimeError('Hook initialization rejected')
                        send({'type': 'user', 'message': {'role': 'user', 'content': 'Call the probe twice.'}})
                    if record.get('type') == 'control_request':
                        req = record['request']
                        if req['subtype'] == 'hook_callback':
                            hooks.append(req)
                            pending.append((time.monotonic() + 0.3, record))
                        elif req['subtype'] == 'mcp_message':
                            msg = req['message']
                            result = {}
                            if msg.get('method') == 'initialize':
                                result = {'protocolVersion': '2025-11-25', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'oc', 'version': '1.0.0'}}
                            elif msg.get('method') == 'tools/list':
                                result = {'tools': [{'name': 'probe', 'description': 'Offline probe', 'inputSchema': {'type': 'object', 'properties': {}}}]}
                            elif msg.get('method') == 'tools/call':
                                calls.append(msg)
                                result = {'isError': True, 'content': [{'type': 'text', 'text': 'Unexpected execution blocked by offline client'}]}
                            send({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': record['request_id'], 'response': {'mcp_response': {'jsonrpc': '2.0', 'id': msg.get('id', 0), 'result': result}}}})
                    if record.get('type') == 'result' and not proc.stdin.closed:
                        proc.stdin.close()
        timed_out = proc.poll() is None
        if timed_out:
            proc.kill()
        proc.wait(timeout=5)
    server.shutdown()
    server.server_close()
    (root / 'requests.json').write_text(json.dumps(requests, indent=2))
    summary = {'matcher': matcher, 'names': names, 'requests': len(requests), 'exit_code': proc.returncode, 'timed_out': timed_out, 'hooks': hooks, 'tools_call': calls, 'results': [r for r in records if r.get('type') in ['user', 'result']], 'initialize': [r for r in records if r.get('type') == 'control_response']}
    (root / 'summary.json').write_text(json.dumps(summary, indent=2))
    return summary


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    opts = parser.parse_args()
    proof = confinement.preflight()
    cli = Path('/Users/titanous/.local/share/claude/versions/2.1.314').resolve(strict=True)
    digest = hashlib.sha256(cli.read_bytes()).hexdigest()
    if digest != PIN:
        raise RuntimeError('Unreviewed CLI bytes; refusing execution')
    root = opts.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    (root / 'confinement-proof.json').write_text(json.dumps(proof, indent=2))
    (root / 'cli.json').write_text(json.dumps({'path': str(cli), 'sha256': digest, 'size': cli.stat().st_size, 'version_source': 'installed version path; no unrestricted CLI invocation'}))
    cases = [('prefixed-control', 'mcp__oc__.*', ['mcp__oc__probe', 'mcp__oc__probe']), ('bare-prefix-matcher', 'mcp__oc__.*', ['probe', 'probe']), ('bare-catchall', '.*', ['probe', 'probe']), ('bare-explicit', 'probe', ['probe', 'probe'])]
    summaries = [run(cli, root / label, matcher, names) for label, matcher, names in cases]
    (root / 'summary.json').write_text(json.dumps(summaries, indent=2))
    print(json.dumps([{'matcher': s['matcher'], 'names': s['names'], 'requests': s['requests'], 'hook_callbacks': sum('subtype' in h for h in s['hooks']), 'tools_call': len(s['tools_call']), 'exit_code': s['exit_code'], 'timed_out': s['timed_out']} for s in summaries], indent=2))
    if any(s['timed_out'] or s['exit_code'] != 0 or not 1 <= s['requests'] <= 2 for s in summaries):
        raise RuntimeError('Incomplete experiment; inspect artifacts')


if __name__ == '__main__':
    main()
