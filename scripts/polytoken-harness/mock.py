#!/usr/bin/env python3
"""Dependency-free scripted Anthropic recorder. Run with python3 -I."""
import argparse
import hashlib
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def events(blocks, ordinal):
    def event(kind, **fields):
        return f'event: {kind}\ndata: {json.dumps(dict(type=kind, **fields))}\n\n'
    out = event('message_start', message=dict(id=f'msg_harness_{ordinal}', type='message', role='assistant', model='claude-test', content=[], stop_reason=None, stop_sequence=None, usage=dict(input_tokens=10, output_tokens=1)))
    for i, block in enumerate(blocks):
        kind = block['type']
        start = dict(block)
        if kind == 'text':
            start['text'] = ''
            deltas = [dict(type='text_delta', text=block['text'])]
        elif kind == 'thinking':
            start.update(thinking='', signature='')
            deltas = [dict(type='thinking_delta', thinking=block['thinking']), dict(type='signature_delta', signature=block['signature'])]
        elif kind == 'tool_use':
            start['input'] = {}
            deltas = [dict(type='input_json_delta', partial_json=json.dumps(block['input']))]
        else:
            raise ValueError(kind)
        out += event('content_block_start', index=i, content_block=start)
        for delta in deltas:
            out += event('content_block_delta', index=i, delta=delta)
        out += event('content_block_stop', index=i)
    reason = 'tool_use' if any(b['type'] == 'tool_use' for b in blocks) else 'end_turn'
    return (out + event('message_delta', delta=dict(stop_reason=reason, stop_sequence=None), usage=dict(output_tokens=20)) + event('message_stop')).encode()


def field_hashes(raw):
    """Hash original JSON value slices, not reserialized/normalized values."""
    text = raw.decode()
    decoder = json.JSONDecoder()
    pos = text.index('{') + 1
    hashes = {}
    while True:
        while text[pos].isspace() or text[pos] == ',':
            pos += 1
        if text[pos] == '}':
            return hashes
        key, pos = decoder.raw_decode(text, pos)
        while text[pos].isspace() or text[pos] == ':':
            pos += 1
        start = pos
        _, pos = decoder.raw_decode(text, pos)
        if key in ('system', 'tools', 'model'):
            hashes[key] = hashlib.sha256(text[start:pos].encode()).hexdigest()


def serve(script, recording):
    lock = threading.Lock()
    state = {'main': 0, 'total': 0, 'errors': []}
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass
        def do_GET(self):
            self.respond()
        def do_POST(self):
            self.respond()
        def respond(self):
            raw = self.rfile.read(int(self.headers.get('Content-Length', 0)))
            try:
                body = json.loads(raw) if raw else None
            except ValueError:
                body = {'unparsed': raw.decode(errors='replace')}
            names = [t.get('name') for t in (body or {}).get('tools', [])]
            title = '_infer_title' in names
            with lock:
                state['total'] += 1
                record = dict(index=state['total'], label='title_inference' if title else 'main', method=self.command, path=self.path,
                              headers={k.lower(): ('<REDACTED>' if any(s in k.lower() for s in ('authorization', 'api-key', 'cookie', 'token')) else v) for k, v in self.headers.items()}, body=body, wire_field_sha256=field_hashes(raw) if isinstance(body, dict) and 'model' in body else {})
                with Path(recording).open('a') as f:
                    f.write(json.dumps(record, separators=(',', ':')) + '\n')
                if self.command == 'POST' and self.path == '/v1/messages':
                    if title:
                        blocks = [dict(type='tool_use', id='title_1', name='_infer_title', input={'title': 'Harness session'})]
                    elif state['main'] < len(script):
                        blocks = script[state['main']]
                        state['main'] += 1
                    else:
                        state['errors'].append('unexpected message request')
                        blocks = None
                else:
                    blocks = None
            if self.command == 'GET' and self.path == '/v1/models':
                payload = json.dumps({'data': [{'id': 'claude-test', 'type': 'model', 'display_name': 'Harness'}], 'has_more': False}).encode()
                status, mime = 200, 'application/json'
            elif blocks is not None:
                payload, status, mime = events(blocks, record['index']), 200, 'text/event-stream'
            else:
                payload, status, mime = b'{"error":{"type":"invalid_request_error","message":"unscripted request"}}', 400, 'application/json'
            self.send_response(status)
            self.send_header('Content-Type', mime)
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    return server, state


if __name__ == '__main__':
    p = argparse.ArgumentParser()
    p.add_argument('--script', type=Path, required=True)
    p.add_argument('--recording', type=Path, required=True)
    args = p.parse_args()
    server, _ = serve(json.loads(args.script.read_text()), args.recording)
    print(server.server_port, flush=True)
    server.serve_forever()
