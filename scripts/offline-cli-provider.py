#!/usr/bin/env python3
"""Bounded real-CLI experiment: fake provider, isolated HOME, mandatory OS sandbox."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

spec = importlib.util.spec_from_file_location('confinement', Path(__file__).with_name('offline-cli-confinement.py'))
confinement = importlib.util.module_from_spec(spec)
spec.loader.exec_module(confinement)
SCHEMA = {'type': 'object', 'properties': {'items': {'type': 'array', 'items': {'type': 'string'}}, 'count': {'type': 'number'}, 'limit': {'type': 'integer'}}, 'required': ['items', 'count', 'limit']}


def sse(name, values):
    events = [
        {'type': 'message_start', 'message': {'id': 'msg_offline', 'type': 'message', 'role': 'assistant', 'model': 'claude-haiku-4-5', 'content': [], 'stop_reason': None, 'stop_sequence': None, 'usage': {'input_tokens': 10, 'output_tokens': 0}}},
        {'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'tool_use', 'id': 'toolu_offline', 'name': name, 'input': {}}},
        {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'input_json_delta', 'partial_json': json.dumps(values)}},
        {'type': 'content_block_stop', 'index': 0},
        {'type': 'message_delta', 'delta': {'stop_reason': 'tool_use', 'stop_sequence': None}, 'usage': {'output_tokens': 20}},
        {'type': 'message_stop'}]
    return ''.join('event: ' + e['type'] + '\ndata: ' + json.dumps(e) + '\n\n' for e in events).encode()


def run_case(cli, root, name, strings):
    root.mkdir()
    for directory in ['home', 'config', 'cache', 'data', 'state', 'work', 'tmp']:
        (root / directory).mkdir()
    (root / 'network.sb').write_text(confinement.PROFILE)
    values = {'items': ['a', 'b'], 'count': 2.5, 'limit': 3}
    if strings:
        values = {'items': '["a","b"]', 'count': '2.5', 'limit': '3'}
    payload = sse(name, values)
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
            if len(requests) > 3:
                self.send_error(429, 'offline request ceiling exceeded')
                return
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TMPDIR': str(root / 'tmp'),
           'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_CACHE_HOME': str(root / 'cache'),
           'XDG_DATA_HOME': str(root / 'data'), 'XDG_STATE_HOME': str(root / 'state'),
           'ANTHROPIC_BASE_URL': 'http://127.0.0.1:' + str(server.server_port),
           'ANTHROPIC_API_KEY': 'offline-fake-key', 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1',
           'CLAUDE_CODE_DISABLE_ATTACHMENTS': '1', 'ENABLE_CLAUDEAI_MCP_SERVERS': 'false',
           'ENABLE_TOOL_SEARCH': 'false', 'DISABLE_AUTOUPDATER': '1', 'DISABLE_TELEMETRY': '1'}
    args = ['/usr/bin/sandbox-exec', '-f', str(root / 'network.sb'), str(cli),
            '--output-format', 'stream-json', '--verbose', '--input-format', 'stream-json',
            '--max-turns', '1', '--model', 'claude-haiku-4-5', '--allowedTools', 'mcp__oc__probe',
            '--tools', '', '--setting-sources=', '--strict-mcp-config', '--permission-mode',
            'bypassPermissions', '--allow-dangerously-skip-permissions', '--include-partial-messages']
    (root / 'argv.json').write_text(json.dumps(args))
    proc = subprocess.Popen(args, env=env, cwd=root / 'work', stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
    records = []
    timed_out = False
    with (root / 'stdin.ndjson').open('w') as stdin, (root / 'stdout.ndjson').open('w') as stdout, (root / 'stderr.txt').open('w') as stderr:
        def send(record):
            line = json.dumps(record) + '\n'
            stdin.write(line)
            stdin.flush()
            proc.stdin.write(line)
            proc.stdin.flush()
        send({'type': 'control_request', 'request_id': 'init', 'request': {'subtype': 'initialize', 'sdkMcpServers': ['oc'], 'systemPrompt': ['Offline probe.']}})
        send({'type': 'user', 'message': {'role': 'user', 'content': 'Call the probe tool.'}})
        selector = selectors.DefaultSelector()
        selector.register(proc.stdout, selectors.EVENT_READ, stdout)
        selector.register(proc.stderr, selectors.EVENT_READ, stderr)
        deadline = time.monotonic() + 35
        while selector.get_map() and time.monotonic() < deadline:
            for key, _ in selector.select(timeout=0.2):
                line = key.fileobj.readline()
                if not line:
                    selector.unregister(key.fileobj)
                    continue
                key.data.write(line)
                key.data.flush()
                if key.fileobj is not proc.stdout:
                    continue
                record = json.loads(line)
                records.append(record)
                if record.get('type') == 'control_request':
                    request = record['request']
                    if request.get('subtype') == 'mcp_message':
                        message = request['message']
                        method = message.get('method')
                        result = {}
                        if method == 'initialize':
                            result = {'protocolVersion': '2025-11-25', 'capabilities': {'tools': {'listChanged': True}}, 'serverInfo': {'name': 'oc', 'version': '1.0.0'}}
                        elif method == 'tools/list':
                            result = {'tools': [{'name': 'probe', 'description': 'Offline type probe', 'inputSchema': dict(SCHEMA, **{'$schema': 'http://json-schema.org/draft-07/schema#'}), 'execution': {'taskSupport': 'forbidden'}}]}
                        elif method == 'tools/call':
                            result = {'isError': True, 'content': [{'type': 'text', 'text': 'Offline client owns tool execution'}]}
                        send({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': record['request_id'], 'response': {'mcp_response': {'jsonrpc': '2.0', 'id': message.get('id', 0), 'result': result}}}})
                if record.get('type') == 'result':
                    proc.stdin.close()
            if proc.poll() is not None:
                # read remaining lines through the registered pipes
                pass
        if proc.poll() is None:
            timed_out = True
            proc.kill()
        proc.wait(timeout=5)
    server.shutdown()
    server.server_close()
    (root / 'requests.json').write_text(json.dumps(requests, indent=2))
    assistant = [r['message']['content'] for r in records if r.get('type') == 'assistant']
    summary = {'name': name, 'strings': strings, 'raw_input': values, 'assistant': assistant,
               'requests': len(requests), 'exit_code': proc.returncode, 'timed_out': timed_out}
    (root / 'summary.json').write_text(json.dumps(summary, indent=2))
    return summary


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--cli', type=Path, default=Path.home() / '.local/share/claude/versions/2.1.308')
    parser.add_argument('--output', type=Path, required=True)
    options = parser.parse_args()
    proof = confinement.preflight()  # Must pass before even opening/executing CLI.
    cli = options.cli.resolve(strict=True)
    if hashlib.sha256(cli.read_bytes()).hexdigest() != 'ca923c86ac84b30574a82331a05b6e066c27477a926ceaffe46a845a33e4dd37':
        raise RuntimeError('Unreviewed CLI bytes; refusing execution')
    root = options.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    (root / 'confinement-proof.json').write_text(json.dumps(proof, indent=2))
    summaries = []
    for name in ['probe', 'mcp__oc__probe']:
        for strings in [False, True]:
            summaries.append(run_case(cli, root / (name + ('-strings' if strings else '-typed')), name, strings))
    (root / 'summary.json').write_text(json.dumps(summaries, indent=2))
    print(json.dumps(summaries, indent=2))
    if any(s['timed_out'] or not 1 <= s['requests'] <= 3 or not any(b.get('type') == 'tool_use' for content in s['assistant'] for b in content) for s in summaries):
        raise RuntimeError('Incomplete experiment; inspect captured artifacts')
    verifier_spec = importlib.util.spec_from_file_location('verifier', Path(__file__).with_name('test-offline-cli-provider.py'))
    verifier = importlib.util.module_from_spec(verifier_spec)
    verifier_spec.loader.exec_module(verifier)
    verifier.verify(root)


if __name__ == '__main__':
    main()
