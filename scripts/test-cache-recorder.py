import importlib.util
import json
from pathlib import Path
import tempfile
import threading
import unittest
import urllib.request


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).parent / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


recorder = load('recorder', 'cache-recorder.py')
mock = load('mock', 'polytoken-harness/mock.py')


analyzer = load('analyzer', 'analyze-cache.py')


class Tests(unittest.TestCase):
    def test_origins_and_markers(self):
        self.assertEqual(analyzer.origin('/tools', {}, {}, {'tools': []}, {'tools': [1]})['origin'], 'Polytoken')
        self.assertEqual(analyzer.origin('/system/0', {'system': ['old']}, {'system': ['billing new']}, {'system': 'same'}, {'system': 'same'})['origin'], 'CLI')
        self.assertEqual(analyzer.origin('/messages/0', {'messages': ['x']}, {'messages': ['conversation_history']}, {}, {})['origin'], 'clod')
        self.assertEqual(analyzer.markers({'system': [{'cache_control': {'type': 'ephemeral'}}]})[0]['path'], '/system/0')

    def test_offline_chain_analysis(self):
        root = Path(__file__).parent / 'cache-measurements/offline-chain-1'
        if not root.exists():
            self.skipTest('run cache-chain.py offline first')
        result = analyzer.analyze(root)
        self.assertEqual(len(result['requests']), 2)
        self.assertTrue(all(r['cli_run'] and r['incoming_request'] for r in result['requests']))

    def test_sanitizer_nested(self):
        value = {'accountId': 'private-id', 'nested': {'access_token': 'secret'}, 'text': '/Users/person/project person@example.com sk-ant-secret'}
        saved = json.dumps(recorder.sanitize(value))
        for secret in ('private-id', 'secret', '/Users/', 'person@example.com'):
            self.assertNotIn(secret, saved)

    def test_redaction(self):
        result = recorder.sanitize({'text': 'person@example.com sk-ant-secret', 'account_uuid': 'secret-id'})
        self.assertNotIn('person@example.com', json.dumps(result))
        self.assertNotIn('secret-id', json.dumps(result))

    def test_streaming(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'capture.ndjson'
            upstream, _ = mock.serve([[dict(type='text', text='ok')]], Path(directory) / 'mock.ndjson')
            proxy = recorder.serve(f'http://127.0.0.1:{upstream.server_port}', output)
            for server in (upstream, proxy):
                threading.Thread(target=server.serve_forever, daemon=True).start()
            try:
                raw = b'{"model":"test", "system": [1, 2], "tools":[], "messages":[]}'
                req = urllib.request.Request(f'http://127.0.0.1:{proxy.server_port}/v1/messages', data=raw, headers={'Authorization': 'do-not-log'})
                with urllib.request.urlopen(req) as response:
                    self.assertIn(b'message_stop', response.read())
                records = [json.loads(line) for line in output.read_text().splitlines()]
                self.assertEqual(records[0]['wire_sha256']['/system'], mock.field_hashes(raw)['system'])
                self.assertEqual(records[1]['usage_events'][0]['usage']['input_tokens'], 10)
                self.assertEqual(records[1]['usage_events'][-1]['usage']['output_tokens'], 20)
                self.assertNotIn('do-not-log', output.read_text())
            finally:
                for server in (proxy, upstream):
                    server.shutdown()
                    server.server_close()


if __name__ == '__main__':
    unittest.main()
