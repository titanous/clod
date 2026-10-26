"""Sanitize diagnostic live input/stdout; preserve protocol ordering and provider caller."""
import importlib.util
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('recorder', ROOT/'scripts/cache-recorder.py')
s = importlib.util.module_from_spec(spec)
spec.loader.exec_module(s)
root = ROOT/'scripts/cache-measurements/resume-diagnostic-1'
requests = [json.loads(x)['body'] for x in (root/'incoming.ndjson').read_text().splitlines() if json.loads(x).get('kind') == 'request'][:2]
id = requests[1]['messages'][-2]['content'][0]['id']
stdout = next(p for p in root.glob('stdout-*.ndjson') if id in p.read_text())
records = [json.loads(x) for x in stdout.read_text().splitlines()]
# Initialization control_response includes account metadata, never publish it.
records = [x for x in records if x.get('type') in ('stream_event','assistant','result','control_request')]
output = ROOT/'crates/clod-core/tests/fixtures/live'
output.mkdir(parents=True, exist_ok=True)
(output/'requests.json').write_text(json.dumps(s.sanitize(requests), indent=2)+'\n')
(output/'tool-stdout.ndjson').write_text('\n'.join(json.dumps(s.sanitize(x)) for x in records)+'\n')
print('sanitized',len(records),'stdout records')
