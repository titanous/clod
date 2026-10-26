"""Standard-library checks; no binary or network required."""
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('mock', Path(__file__).with_name('mock.py'))
mock = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mock)


class HarnessTests(unittest.TestCase):
    def test_hashes_original_slices(self):
        a = mock.field_hashes(b'{"model":"m", "system": [1, 2], "tools":[]}')
        b = mock.field_hashes(b'{"model":"m", "system": [1,2], "tools":[]}')
        self.assertEqual(a['model'], b['model'])
        self.assertNotEqual(a['system'], b['system'])

    def test_tool_changes(self):
        spec = importlib.util.spec_from_file_location('analyze', Path(__file__).with_name('analyze.py'))
        analyze = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(analyze)
        a = [{'name': 'a', 'input_schema': {}}, {'name': 'b'}]
        self.assertTrue(analyze.tool_changes(a, a[::-1])['order_only'])
        self.assertEqual(analyze.tool_changes(a, [{'name': 'a', 'input_schema': {'type': 'object'}}, {'name': 'c'}]),
                         dict(added=['c'], removed=['b'], changed=['a'], order_only=False))

    def test_sse_thinking_and_tool(self):
        wire = mock.events([dict(type='thinking', thinking='thought', signature='sig'), dict(type='tool_use', id='t', name='file_read', input={'path': 'hello.txt'})], 1)
        payloads = [json.loads(line[6:]) for line in wire.decode().splitlines() if line.startswith('data: ')]
        self.assertEqual(payloads[0]['type'], 'message_start')
        self.assertEqual(payloads[-1]['type'], 'message_stop')
        self.assertIn({'type': 'signature_delta', 'signature': 'sig'}, [p.get('delta') for p in payloads])
        self.assertEqual(payloads[-2]['delta']['stop_reason'], 'tool_use')


if __name__ == '__main__':
    unittest.main()
