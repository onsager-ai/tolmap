"""CI only: bounded failure evidence from synthetic consumer repositories."""
import json
import os
from pathlib import Path
import stat
import subprocess
import sys

LIMIT = 2 * 1024


def tail(path):
    with path.open('rb') as stream:
        stream.seek(max(0, path.stat().st_size - LIMIT))
        return stream.read(LIMIT).decode('utf-8', errors='replace')


evidence = dict(exit_status=int(sys.argv[1]), uid=os.geteuid(), gid=os.getegid(),
                identity=subprocess.check_output(['id'], text=True).strip(),
                processes=subprocess.check_output(
                    ['ps', '-eo', 'user,uid,gid,pid,ppid,pgid,stat,time,pcpu,args'], text=True)[-8192:],
                logs={}, store_entries=[])
logs = [Path('/tmp/consumer/.local-reader/service.log'),
        Path('/tmp/consumer/.local-reader/index-diagnostics.json'),
        Path('/tmp/consumer/.local-reader/index-timings.json'),
        Path('/tmp/reader-worker-writes.log'),
        *Path('/tmp').glob('launcher-*.log')]
store = Path('/tmp/consumer/.local-reader/store')
# No production repository data: these paths belong to the synthetic fixture.
for current, dirs, files in os.walk(store):
    dirs.sort()
    files.sort()
    path = Path(current)
    info = path.stat()
    evidence['store_entries'].append(dict(path=str(path), uid=info.st_uid,
                                          gid=info.st_gid, mode=oct(stat.S_IMODE(info.st_mode))))
    for name in files:
        if name.endswith(('.log', '.json')) or 'status' in name or 'stderr' in name:
            logs.append(path / name)
    if len(evidence['store_entries']) >= 100:
        break
for path in logs[:12]:
    try:
        if path.is_file():
            evidence['logs'][str(path)] = tail(path)
    except OSError as error:
        evidence['logs'][str(path)] = str(error)
Path('/out/consumer-diagnostics.json').write_text(json.dumps(evidence, indent=2) + '\n')
# Preserve the structured timeout record intact, rather than only its log tail.
timeout_record = Path('/tmp/consumer/.local-reader/index-diagnostics.json')
if timeout_record.is_file():
    Path('/out/reader-index-timeout.json').write_bytes(timeout_record.read_bytes())
print('Captured bounded consumer identity, processes, worker writes and store log/status evidence', file=sys.stderr)
