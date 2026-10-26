#!/usr/bin/env python3
"""Body-only loopback measurement proxy. No headers or response text are saved."""
import argparse
import datetime
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import threading
import urllib.parse


def sanitize(value):
    if isinstance(value, dict):
        return {k: ('<REDACTED>' if re.search(r'(?i)account.?id|account.?uuid|organization.?id|organization.?uuid|authorization|api.?key|access.?token|refresh.?token', k) else sanitize(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [sanitize(v) for v in value]
    if not isinstance(value, str):
        return value
    text = value
    text = text.replace(os.path.expanduser('~'), '<HOME>')
    text = re.sub(r'/Users/[^/\\"\s]+', '<HOME>', text)
    text = re.sub(r'/home/[^/\\"\s]+', '<HOME>', text)
    text = re.sub(r'[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}', '<EMAIL>', text, flags=re.I)
    text = re.sub(r'(?:sk-ant-|sk-proj-)[\w-]+', '<TOKEN>', text)
    text = re.sub(r'(?i)(account(?:_?uuid|_?id)|organization(?:_?uuid|_?id))\\?"\s*:\s*\\?"[^"\\]*', r'\1":"<ACCOUNT>', text)
    return text


def slices(raw):
    text = raw.decode()
    dec = json.JSONDecoder()
    result = {}
    def walk(pos, key):
        while text[pos].isspace():
            pos += 1
        start = pos
        if text[pos] == '{':
            pos += 1
            while True:
                while text[pos].isspace() or text[pos] == ',':
                    pos += 1
                if text[pos] == '}':
                    pos += 1
                    break
                name, pos = dec.raw_decode(text, pos)
                while text[pos].isspace() or text[pos] == ':':
                    pos += 1
                pos = walk(pos, key + '/' + name)
        elif text[pos] == '[':
            pos += 1
            i = 0
            while True:
                while text[pos].isspace() or text[pos] == ',':
                    pos += 1
                if text[pos] == ']':
                    pos += 1
                    break
                pos = walk(pos, key + '/' + str(i))
                i += 1
        else:
            _, pos = dec.raw_decode(text, pos)
        result[key] = hashlib.sha256(text[start:pos].encode()).hexdigest()
        return pos
    walk(0, '')
    return result


def serve(target, output, port=0, label='upstream'):
    dest = urllib.parse.urlsplit(target)
    lock = threading.Lock()
    count = 0
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    def save(record):
        with lock, output.open('a') as f:
            f.write(json.dumps(sanitize(record), separators=(',', ':')) + '\n')
    class Handler(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'
        def log_message(self, *_):
            pass
        def do_GET(self):
            self.forward()
        def do_POST(self):
            self.forward()
        def forward(self):
            nonlocal count
            raw = self.rfile.read(int(self.headers.get('Content-Length', 0)))
            with lock:
                count += 1
                index = count
            try:
                body = json.loads(raw) if raw else None
            except ValueError:
                body = '<non-JSON omitted>'
            route = self.path
            run = None
            match = re.match(r'/__run/([a-f0-9]+)(/.*)', route)
            if match:
                run, route = match.groups()
            save(dict(kind='request', label=label, index=index, cli_run=run, timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(), method=self.command, path=route, body=body, wire_sha256=slices(raw) if isinstance(body, dict) else {}))
            cls = http.client.HTTPSConnection if dest.scheme == 'https' else http.client.HTTPConnection
            conn = cls(dest.hostname, dest.port, timeout=180)
            headers = {k: v for k, v in self.headers.items() if k.lower() not in ('host', 'connection', 'transfer-encoding')}
            headers['Host'] = dest.netloc
            # Keep SSE inspectable and avoid forwarding compressed bytes without encoding metadata.
            headers = {k: v for k, v in headers.items() if k.lower() != 'accept-encoding'}
            headers['Accept-Encoding'] = 'identity'
            usages = []
            try:
                conn.request(self.command, dest.path.rstrip('/') + route, raw, headers)
                response = conn.getresponse()
                self.send_response(response.status)
                self.send_header('Content-Type', response.getheader('Content-Type', 'application/json'))
                self.send_header('Connection', 'close')
                self.end_headers()
                pending = b''
                while True:
                    chunk = response.read1(65536)
                    if not chunk:
                        break
                    self.wfile.write(chunk)
                    self.wfile.flush()
                    pending += chunk
                    while b'\n' in pending:
                        line, pending = pending.split(b'\n', 1)
                        if line.startswith(b'data: '):
                            try:
                                event = json.loads(line[6:])
                                if event.get('type') in ('message_start', 'message_delta'):
                                    usages.append(dict(type=event['type'], usage=event.get('message', event).get('usage')))
                            except ValueError:
                                pass
                save(dict(kind='response', label=label, index=index, status=response.status, usage_events=usages))
            except Exception as error:
                save(dict(kind='error', label=label, index=index, error=type(error).__name__))
                self.close_connection = True
            finally:
                conn.close()
                self.close_connection = True
    return ThreadingHTTPServer(('127.0.0.1', port), Handler)


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--target', default='https://api.anthropic.com')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--port', type=int, default=0)
    p.add_argument('--label', default='upstream')
    a = p.parse_args()
    server = serve(a.target, a.output, a.port, a.label)
    print(server.server_port, flush=True)
    server.serve_forever()
