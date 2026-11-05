"""CI-only extraction diagnosis, separate from the production reader benchmark.

One fresh-process untraced/traced pair per shape, same packaged binary and inputs.
Timeouts are observations, never passes. No blend/layout or browser is involved.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

binary = str(Path(sys.argv[1]).resolve())
root = Path('/tmp/reader-extraction-cost')
root.mkdir()
output = Path('/out/reader-extraction-cost.json')
samples = []
scope = ('Fresh-process dump-graph --refs hand; one untraced then one write-traced run '
         'per synthetic source. Extraction plus launcher/Git/serialization, '
         'not reader load or a statistically controlled tracing benchmark; '
         'fixed order and OS cache effects limit comparison. '
         '30 s per probe; timeouts remain open evidence, not successful extraction.')
for name, source in [
    ('normal', 'def measured():\n    return 7\n' + '# performance fixture line 739\n' * 30000 + '# PERF_NEEDLE_739\n'),
    ('long', 'LONG = "' + 'x' * 900000 + 'PERF_NEEDLE_739"\n'),
]:
    repo = root / name
    repo.mkdir()
    (repo / 'perf.py').write_text(source)
    for args in [('init',), ('config', 'user.name', 'Fixture'),
                 ('config', 'user.email', 'fixture@example.invalid'),
                 ('add', '.'), ('commit', '-m', 'Synthetic extraction measurement')]:
        subprocess.run(['git', '-C', str(repo), *args], check=True, capture_output=True)
    for traced in (False, True):
        label = f'{name}-{"traced" if traced else "untraced"}'
        command = [binary, 'dump-graph', str(repo), '--pkg', '.', '--lang', 'py',
                   '--refs', 'hand', '--out', str(root / f'{label}.graph.json')]
        if traced:
            command = ['strace', '-f', '-qq', '-e', 'trace=write', '-s', '1024',
                       '-o', str(root / f'{label}.writes.log'), *command]
        started = time.monotonic()
        with (root / f'{label}.log').open('wb') as log:
            process = subprocess.Popen(command, stdout=log, stderr=log, start_new_session=True)
            timed_out = False
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                timed_out = True
            finally:
                # Bound the traced/launcher group, including orphan descendants.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
        with (root / f'{label}.log').open('rb') as log:
            log.seek(max(0, log.seek(0, 2) - 1200))
            tail = log.read(1200).decode(errors='replace')
        samples.append(dict(shape=name, bytes=len(source.encode()), traced=traced,
                            elapsedSeconds=time.monotonic()-started, timeoutSeconds=30,
                            timedOut=timed_out, exitCode=process.returncode, logTail=tail))
        output.write_text(json.dumps(dict(scope=scope, samples=samples), indent=2) + '\n')
print('Recorded bounded packaged extraction probes; inspect reader-extraction-cost artifact')
