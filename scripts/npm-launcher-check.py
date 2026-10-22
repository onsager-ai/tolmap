"""CI only: signal the npm entrypoint while its real native worker is paused."""
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time
import urllib.request

binary = os.environ['TOLMAP_BINARY']
native = str(Path(binary).resolve().parents[1] / 'lib/tolmap')
# Ordinary nonzero exit status is preserved too.
args = ['--not-a-real-option']
assert subprocess.run([binary, *args]).returncode == subprocess.run([native, *args]).returncode != 0

def descendants(parent):
    seen=set(); pending=[parent]
    while pending:
        current=pending.pop()
        for children in Path(f'/proc/{current}/task').glob('*/children'):
            try: ids=[int(x) for x in children.read_text().split()]
            except FileNotFoundError: continue
            for pid in ids:
                if pid not in seen: seen.add(pid); pending.append(pid)
    return seen

results=[]
for signum in (signal.SIGTERM, signal.SIGINT):
    port=8990
    origin=f'http://127.0.0.1:{port}'
    def req(path, post=False):
        request=urllib.request.Request(origin+path, data=b'' if post else None, headers={'Origin':origin})
        with urllib.request.urlopen(request, timeout=20) as res: return res.read()
    with open(f'/tmp/launcher-{signum.name}.log', 'wb') as log:
        p=subprocess.Popen([binary, 'local', str(Path('.local-reader/moved-checkout').resolve()),
            '--data', f'/tmp/launcher-store-{signum.name}', '--port', str(port)], stdout=log, stderr=log)
        try:
            for _ in range(200):
                try: req('/api/healthz'); break
                except OSError: time.sleep(.05)
                assert p.poll() is None
            else: raise AssertionError('startup timeout')
            body=req('/local/refresh', True)
            prepared=re.search(rb'action=/local/index\?id=([a-f0-9-]+)', body).group(1).decode()
            worker=None
            for _ in range(5):
                req('/local/index?id='+prepared, True)
                deadline=time.monotonic()+5
                while time.monotonic() < deadline:
                    tree=descendants(p.pid)
                    for pid in tree:
                        try:
                            argv=Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
                            if len(argv)>1 and argv[1]==b'worker':
                                worker=pid; os.killpg(pid, signal.SIGSTOP); break
                        except (FileNotFoundError, ProcessLookupError): pass
                    if worker: break
                    time.sleep(.002)
                if worker: break
            assert worker, 'signal check never observed an actual worker'
            tree=descendants(p.pid)
            assert len(tree)>=2 and worker in tree, tree
            p.send_signal(signum)
            assert p.wait(timeout=15) == -signum, p.returncode
            for _ in range(200):
                alive=[pid for pid in tree if Path(f'/proc/{pid}').exists()]
                if not alive: break
                time.sleep(.05)
            assert not alive, ('surviving descendants', alive)
            results.append(dict(signal=signum.name, launcher_status=p.returncode, worker=worker,
                                observed_descendants=sorted(tree), survivors=[]))
        finally:
            for pid in descendants(p.pid):
                try: os.kill(pid, signal.SIGKILL)
                except ProcessLookupError: pass
            if p.poll() is None: p.kill(); p.wait()
Path('/tmp/signals-result.json').write_text(json.dumps(results))
