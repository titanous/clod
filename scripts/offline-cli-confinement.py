#!/usr/bin/env python3
"""Fail-closed macOS network-confinement preflight; never launches Claude."""
import errno
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile

PROFILE = '''(version 1)
(allow default)
(deny network*)
(allow network-inbound (local ip "localhost:*"))
(allow network-outbound (remote ip "localhost:*"))
'''


def child(port):
    result = {}
    with socket.socket() as s:
        s.settimeout(2)
        s.connect(('127.0.0.1', port))
        result['loopback'] = 'connected'
    for address in ['192.0.2.1', '1.1.1.1']:
        with socket.socket() as s:
            s.settimeout(2)
            try:
                s.connect((address, 443))
            except OSError as exc:
                result[address] = {'errno': exc.errno, 'error': str(exc)}
                if exc.errno != errno.EPERM:
                    raise RuntimeError('No positive OS-denial evidence: ' + repr(result))
            else:
                raise RuntimeError('CONFINEMENT FAILED: non-loopback connected')
    print(json.dumps(result))


def preflight():
    if sys.platform != 'darwin' or not Path('/usr/bin/sandbox-exec').is_file():
        raise RuntimeError('Required macOS sandbox-exec is unavailable; refusing CLI execution')
    with tempfile.TemporaryDirectory(prefix='clod-offline-preflight-') as tmp:
        root = Path(tmp)
        for name in ['home', 'config', 'cache', 'data', 'state', 'work', 'tmp']:
            (root / name).mkdir()
        profile = root / 'network.sb'
        profile.write_text(PROFILE)
        env = {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
               'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_CACHE_HOME': str(root / 'cache'),
               'XDG_DATA_HOME': str(root / 'data'), 'XDG_STATE_HOME': str(root / 'state'),
               'TMPDIR': str(root / 'tmp')}
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen(1)
            run = subprocess.run(['/usr/bin/sandbox-exec', '-f', str(profile),
                                  '/usr/bin/python3', '-I', str(Path(__file__).resolve()),
                                  '--child', str(listener.getsockname()[1])],
                                 env=env, cwd=root / 'work', capture_output=True, text=True, timeout=10)
        print(json.dumps({'sandbox_returncode': run.returncode,
                          'stdout': run.stdout, 'stderr': run.stderr}, indent=2))
        if run.returncode != 0:
            raise RuntimeError('Confinement preflight failed; refusing CLI execution')
        evidence = json.loads(run.stdout)
        if evidence.get('loopback') != 'connected' or any(
                evidence.get(ip, {}).get('errno') != errno.EPERM for ip in ['192.0.2.1', '1.1.1.1']):
            raise RuntimeError('Incomplete confinement proof; refusing CLI execution')
        return evidence


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--child':
        child(int(sys.argv[2]))
    else:
        try:
            preflight()
        except Exception as exc:
            print(str(exc), file=sys.stderr)
            sys.exit(1)
