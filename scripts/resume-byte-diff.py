"""Generate exact compact canonical assistant diff from sanitized captured input."""
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
bodies = json.loads((ROOT/'crates/clod-core/tests/fixtures/live/requests.json').read_text())
echo = bodies[1]['messages'][-2]
prior = json.loads(json.dumps(echo))
prior['content'][0]['caller'] = {'type':'direct'}
canonical = lambda v: json.dumps(v, sort_keys=True, separators=(',', ':'), ensure_ascii=False)
a, b = canonical(prior), canonical(echo)
offset = next(i for i,(x,y) in enumerate(zip(a.encode(),b.encode())) if x != y)
result = dict(evidence='Reconstructed pre-fix assistant from captured streamed block and completed JSON input; caller is captured, not hypothetical.', first_different_byte=offset, prior=a, echoed=b, extra_bytes='"caller":{"type":"direct"},', prior_length=len(a.encode()), echoed_length=len(b.encode()))
(ROOT/'docs/cache-measurements/resume-byte-diff.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(dict(first_different_byte=offset,prior_length=len(a),echoed_length=len(b))))
