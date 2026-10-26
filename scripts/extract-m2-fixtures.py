import json,importlib.util
from pathlib import Path
root=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('rec',root/'scripts/cache-recorder.py');rec=importlib.util.module_from_spec(spec);spec.loader.exec_module(rec)
for label,name in [('empty-input','claude-haiku-5-5-strict-reveal'),('native-search','claude-opus-5-5-conversation-reveal')]:
 p=root/'scripts/cache-measurements'/('operator-approved-matrix-'+name)
 usage=[json.loads(x) for x in (p/'usage.ndjson').read_text().splitlines()]
 sid=next(x['session_id'] for x in usage if x.get('event')=='message_start')
 f=next(f for f in p.glob('stdout-*') if sid in f.read_text())
 records=[json.loads(x) for x in f.read_text().splitlines()]
 records=[x for x in records if x.get('type') in ('stream_event','assistant','result','control_request')]
 out=root/'crates/clod-core/tests/fixtures/live'
 (out/(label+'.ndjson')).write_text('\n'.join(json.dumps(rec.sanitize(x)) for x in records)+'\n')
 print(label,len(records))
