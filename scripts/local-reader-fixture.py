"""CI only: real binary/process, local Git objects, production exported JSON.
Leaves the isolated loopback process running for the production browser checks.
"""
import hashlib, ipaddress, json, os, pathlib, re, shutil, signal, subprocess, time, urllib.request, urllib.error
root=pathlib.Path('.local-reader').resolve();root.mkdir(exist_ok=True)
repo=root/'checkout';shutil.copytree('tests/fixtures/service_identity',repo)
def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],stderr=subprocess.DEVNULL).decode().strip()
git('init');git('config','user.name','Fixture');git('config','user.email','fixture@example.invalid')
(repo/'binary.dat').write_bytes(b'\0\x01');(repo/'large.txt').write_text('x'*(1024*1024+1))
(repo/'shop/perf.py').write_text('def measured():\n    return 7\n' + '# performance fixture line 739\n'*30000 + '# PERF_NEEDLE_739\n')
(repo/'long.txt').write_text('x'*21000)
(repo/'shop/perf_long.py').write_text('LONG = \"'+'x'*900000+'PERF_NEEDLE_739\"\n')
(repo/'inert.txt').write_text('<script>window.sourceInjected=true</script>\n')
(repo/'escape').symlink_to('/etc/passwd')
git('add','.');git('commit','-m','A');a=git('rev-parse','HEAD')
committed_source=(repo/'shop/cart.py').read_text()
(repo/'shop/cart.py').write_text(committed_source+'\n# UNCOMMITTED_TRACKED_739\n')
(repo/'untracked-local.py').write_text('UNTRACKED_SOURCE_739 = True\n')
binary=pathlib.Path(os.environ.get('TOLMAP_BINARY','target/release/tolmap')).resolve()
origin='http://127.0.0.1:8988'
log=open(root/'service.log','wb')
local_args=[str(binary),'local',str(repo),'--data',str(root/'store')]
if os.environ.get('TOLMAP_TEST_PACKAGED_DEFAULT_STATIC') != '1':
    local_args += ['--static-dir',str(pathlib.Path('web/dist').resolve())]
local_args += ['--port','8988']
# Consumer CI traces the synthetic fixture's worker writes, including errors
# otherwise consumed by the local service. Native reader CI keeps its usual path.
if os.environ.get('TOLMAP_TEST_WRITE_TRACE'):
    local_args = ['strace','-f','-qq','-e','trace=write','-s','1024','-o',
                  os.environ['TOLMAP_TEST_WRITE_TRACE'],*local_args]
if os.environ.get('TOLMAP_TEST_NETWORK_TRACE'):
    local_args = ['strace','-f','-qq','-e','trace=connect','-s','256','-o',os.environ['TOLMAP_TEST_NETWORK_TRACE'],*local_args]
p=subprocess.Popen(local_args,stdout=log,stderr=log,start_new_session=True)
pathlib.Path('reader-evidence').mkdir(exist_ok=True)
index_timings=[]
def descendants(parent):
    seen=set();pending=[parent]
    while pending:
        current=pending.pop()
        for children in pathlib.Path(f'/proc/{current}/task').glob('*/children'):
            try:ids=[int(x) for x in children.read_text().split()]
            except (FileNotFoundError,ProcessLookupError):continue
            for pid in ids:
                if pid not in seen:seen.add(pid);pending.append(pid)
    return seen
def bounded_tail(path,limit=2048):
    try:
        with path.open('rb') as stream:
            stream.seek(max(0,path.stat().st_size-limit))
            return stream.read(limit).decode(errors='replace')
    except OSError as error:return str(error)
def process_state():
    rows=[]
    for pid in sorted({p.pid,*descendants(p.pid)})[:24]:
        try:
            fields=pathlib.Path(f'/proc/{pid}/stat').read_text().split()
            rows.append({'pid':pid,'parent':int(fields[3]),'state':fields[2],
                         'cpuSeconds':(int(fields[13])+int(fields[14]))/os.sysconf('SC_CLK_TCK'),
                         'argv':pathlib.Path(f'/proc/{pid}/cmdline').read_bytes()[:500].replace(b'\0',b' ').decode(errors='replace')})
        except (OSError,ValueError):pass
    return rows
def index_diagnostics(prepared,text,elapsed,observations):
    logs={'service.log':bounded_tail(root/'service.log')};entries=[]
    for current,dirs,files in os.walk(root/'store'):
        dirs.sort();files.sort();directory=pathlib.Path(current)
        entries.append(str(directory.relative_to(root)))
        for name in files:
            if len(logs)<12 and (name.endswith(('.log','.json')) or 'stderr' in name or 'status' in name):
                path=directory/name;logs[str(path.relative_to(root))]=bounded_tail(path)
        if len(entries)>=60:break
    trace=os.environ.get('TOLMAP_TEST_WRITE_TRACE')
    if trace:logs['worker-write-trace-tail']=bounded_tail(pathlib.Path(trace))
    stage=re.search(rb'<h2>Indexing</h2><p>(.*?)</p>',text)
    evidence={'reason':'index timeout','elapsedSeconds':elapsed,'prepared':prepared,
              'preparedPresent':(root/'store/prepared'/prepared).is_dir(),
              'lastStage':stage.group(1).decode(errors='replace') if stage else None,
              'lastPageTail':text[-1800:].decode(errors='replace'),'processes':process_state(),
              'observations':observations,'straceWriteWrapper':bool(trace),
              'straceNetworkWrapper':bool(os.environ.get('TOLMAP_TEST_NETWORK_TRACE')),
              'storeDirectories':entries,'logs':logs}
    (root/'index-diagnostics.json').write_text(json.dumps(evidence,indent=2))
    pathlib.Path('reader-evidence/index-diagnostics.json').write_text(json.dumps(evidence,indent=2))
def req(path,post=False,headers=None):
    r=urllib.request.Request(origin+path,data=b'' if post else None,headers={'Origin':origin,**(headers or {})})
    try:
        with urllib.request.urlopen(r,timeout=20) as res:return res.status,res.read(),res.headers
    except urllib.error.HTTPError as e:return e.code,e.read(),e.headers
def get(path):
    code,body,_=req(path);assert code==200,(code,body[:200]);return json.loads(body)
for _ in range(200):
    try:
        if req('/api/healthz')[0]==200:break
    except OSError:pass
    assert p.poll() is None,'local process exited'
    time.sleep(.1)
else:raise AssertionError('local startup timeout')
def prepare():
    code,body,_=req('/local/refresh',True);assert code==200
    prepared=re.search(rb'action=/local/index\?id=([a-f0-9-]+)',body).group(1).decode()
    return prepared,body
def index_prepared(prepared):
    started=time.monotonic();observations=[]
    code,body,_=req('/local/index?id='+prepared,True);assert code==200,(code,body[:200])
    for poll in range(900):
        text=req('/')[1]
        if poll%100==0:
            stage=re.search(rb'<h2>Indexing</h2><p>(.*?)</p>',text)
            observations.append({'elapsedSeconds':time.monotonic()-started,
                                 'stage':stage.group(1).decode(errors='replace') if stage else None,
                                 'processes':process_state()})
        if b'Cancel indexing' not in text and b'Snapshot saved.' in text:break
        if b'Indexing failed or was cancelled.' in text:raise AssertionError(('indexing failed',text[-1000:]))
        time.sleep(.1)
    else:
        index_diagnostics(prepared,text,time.monotonic()-started,observations)
        raise AssertionError('index timeout; see reader-evidence/index-diagnostics.json')
    index_timings.append({'prepared':prepared,'elapsedSeconds':time.monotonic()-started,'observations':observations,
                          'scope':'whole local index including extraction/blend/layout/publication; not reader load'})
    pathlib.Path('reader-evidence/index-timings.json').write_text(json.dumps(index_timings,indent=2))
    (root/'index-timings.json').write_text(json.dumps(index_timings,indent=2))
    paths=re.findall(rb'href=/snapshot/([a-f0-9-]+)',text)
    assert paths,('no snapshot published', (root/'service.log').read_text()[-3000:],text[-1500:])
    return {x.decode() for x in paths}
prepared_a,confirm_a=prepare();assert b'Confirm analysis scope' in confirm_a and b'Only committed HEAD' in req('/')[1]
git('rm','-r','-f','shop');git('rm','-f','pyproject.toml');git('commit','-m','no supported source')
code,failed_detection,_=req('/local/refresh',True);assert code==200 and b'Source detection failed' in failed_detection
assert b'action=/local/index?id='+prepared_a.encode() in failed_detection and a.encode() in failed_detection,failed_detection[-1200:]
git('reset','--hard',a)
# Keep both changes present through indexing, after the deliberate detection
# failure changed the checkout. Prepared A must read only immutable Git objects.
(repo/'shop/cart.py').write_text(committed_source+'\n# UNCOMMITTED_TRACKED_739\n')
assert 'shop/cart.py' in git('status','--porcelain') and (repo/'untracked-local.py').exists()
first=index_prepared(prepared_a);assert len(first)==1;sid=next(iter(first));base='/api/maps/snapshot/'+sid
assert 'UNCOMMITTED_TRACKED_739' in (repo/'shop/cart.py').read_text() and (repo/'untracked-local.py').exists()
provenance=get(base+'/provenance');assert provenance['commit']==a
original=get(base+'/source?path=shop/cart.py&commit='+a);assert original['status']=='ready'
assert original['text']==committed_source,'dirty tracked source was included in snapshot A'
assert get(base+'/source?path=untracked-local.py&commit='+a)['status']=='missing','untracked source was included in snapshot A'
assert 'UNCOMMITTED_TRACKED_739' not in original['text'] and 'UNTRACKED_SOURCE_739' not in original['text']
for path,status in [('binary.dat','binary'),('large.txt','large'),('escape','unsupported'),('absent','missing')]:assert get(base+'/source?path='+path)['status']==status
assert get(base+'/source?path=long.txt')['long_line']
for path in ['../secret','%2Fetc%2Fpasswd','.git/config','escape/other']:
    code,body,_=req(base+'/source?path='+path);assert code!=200 or json.loads(body)['text'] is None
assert req(base+'/source?path=shop/cart.py&commit='+'0'*40)[0]!=200
assert req('/local/refresh',True,{'Origin':'https://attacker.invalid'})[0]==403
assert req('/api/healthz',headers={'Host':'attacker.invalid'})[0]==403
assert get('/api/maps')==[] and get('/maps/index.json')==[]
export=get(base+'/artifact');assert export==json.loads((root/'store/snapshots'/sid/'map.json').read_text())
assert export['F'] and 'text' not in export and original['text'] not in json.dumps(export)
(repo/'shop/cart.py').write_text(committed_source+'\n# committed B\n')
(repo/'untracked-local.py').unlink()
(repo/'shop/__init__.py').unlink();(repo/'pyproject.toml').unlink()
git('add','-A');git('commit','-m','B');b=git('rev-parse','HEAD')
prepared_b,confirm_b=prepare();assert b'Python at .' in confirm_b and b'low confidence' in confirm_b,confirm_b[-1200:]
prepared_dir=root/'store'/'prepared'/prepared_b;source_git=prepared_dir/'source.git';backup=prepared_dir/'source.git.offline';source_git.rename(backup)
try:
    code,body,_=req('/local/index?id='+prepared_b,True);assert code==200,(code,body[:200])
    for _ in range(900):
        failed_scope=req('/')[1]
        if b'Indexing failed or was cancelled.' in failed_scope:break
        time.sleep(.1)
    else:raise AssertionError('fixture indexing failure did not reach recovery page')
finally:
    backup.rename(source_git)
assert b'action=/local/index?id='+prepared_b.encode() in failed_scope and b'Python at .' in failed_scope,failed_scope[-1200:]
second=index_prepared(prepared_b);bid=next(iter(second-first));assert get('/api/maps/snapshot/'+bid+'/provenance')['commit']==b
assert get(base+'/source?path=shop/cart.py')['text']==original['text']
# Pause the actual engine child, then require cancellation to kill/reap it
# rather than waiting for stdout to reach EOF. Retry only if observation missed
# the tiny fixture; a missed worker is never a cancellation pass.
for attempt in range(5):
    body=req('/')[1];prepared=re.search(rb'action=/local/index\?id=([a-f0-9-]+)',body).group(1).decode()
    req('/local/index?id='+prepared,True)
    deadline=time.monotonic()+5;worker=None
    while time.monotonic()<deadline and worker is None:
        for pid in descendants(p.pid):
            try:
                argv=pathlib.Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
                if len(argv)>1 and argv[1]==b'worker':
                    worker=pid;os.killpg(worker,signal.SIGSTOP);break
            except (FileNotFoundError,ProcessLookupError):pass
        time.sleep(.005)
    if worker is not None:break
else:raise AssertionError('actual local worker was never observed')
before_cancel=len(list((root/'store/snapshots').glob('*/snapshot.json')))
body=req('/')[1];job=re.search(rb'action=/local/cancel\?id=([a-f0-9-]+)',body).group(1).decode()
req('/local/cancel?id='+job,True)
for _ in range(100):
    if not pathlib.Path(f'/proc/{worker}').exists():break
    time.sleep(.05)
else:raise AssertionError('cancelled local worker was not killed/reaped')
for _ in range(100):
    if b'Cancel indexing' not in req('/')[1]:break
    time.sleep(.05)
assert len(list((root/'store/snapshots').glob('*/snapshot.json')))==before_cancel
# A third completed snapshot must use monotonically persisted history order.
prepared_third,_=prepare();third=index_prepared(prepared_third)
ordered=[json.loads(x.read_text())['sequence'] for x in (root/'store/snapshots').glob('*/snapshot.json')]
assert sorted(ordered)==list(range(1,len(ordered)+1))
# Inspect real process/worker outbound connect syscalls, including failures.
network_trace=os.environ.get('TOLMAP_TEST_NETWORK_TRACE')
if network_trace:
    non_loopback=[];loopback=[]
    for line in pathlib.Path(network_trace).open():
        address=None
        if 'sa_family=AF_INET6' in line:
            match=re.search(r'inet_pton\(AF_INET6,\s*"([^"]+)"\)',line)
            address=match.group(1) if match else None
        elif 'sa_family=AF_INET,' in line:
            match=re.search(r'sin_addr=inet_addr\("([^"]+)"\)',line)
            address=match.group(1) if match else None
        else:continue
        try:is_loopback=address is not None and ipaddress.ip_address(address).is_loopback
        except ValueError:is_loopback=False
        (loopback if is_loopback else non_loopback).append(line.rstrip())
    assert not non_loopback, ('unexpected non-loopback network connect',non_loopback)
    pathlib.Path('reader-evidence').mkdir(exist_ok=True)
    pathlib.Path('reader-evidence/network.json').write_text(json.dumps({'scope':'actual local service and descendants through A/B, read, export, cancellation; connect syscalls only','loopbackConnects':loopback,'nonLoopbackConnects':non_loopback},indent=2))
# No private map/source/path was copied into the public catalogue or built
# assets. This runs after snapshot A exists, so the id and source are scanned.
catalog=get('/api/maps');assert catalog==[]
host_paths=(str(pathlib.Path.cwd().resolve()),str(root),str(repo))
for public_value in (json.dumps(catalog),json.dumps(export)):
    assert all(path not in public_value for path in host_paths) and original['text'] not in public_value
assert export==json.loads((root/'store/snapshots'/sid/'map.json').read_text())
assert export['F'] and 'text' not in export and original['text'] not in json.dumps(export)
for output in pathlib.Path('web/dist').rglob('*'):
    if output.is_file():
        data=output.read_bytes()
        assert original['text'].encode() not in data and sid.encode() not in data
        assert all(path.encode() not in data for path in host_paths)
metadata={'a':a,'b':b,'snapshotA':sid,'snapshotB':bid,'file':'shop/cart.py','pid':p.pid,'repository':provenance['repository'],'sourceSha256':hashlib.sha256(committed_source.encode()).hexdigest()}
(root/'fixture.json').write_text(json.dumps(metadata))
pathlib.Path('reader-evidence').mkdir(exist_ok=True)
pathlib.Path('reader-evidence/export-map.json').write_text(json.dumps(export,indent=2))
pathlib.Path('reader-evidence/process.json').write_text(json.dumps({'result':'passed','scope':'real local binary, worker process, Git A/B, dirty/untracked exclusion, detection and indexing recovery, low-confidence-root confirmation, private storage, exports, host/origin/path boundaries','fixture':metadata,'recovery':{'detectionFailureRetainedPreparedA':prepared_a,'lowConfidenceScopeRetained':prepared_b,'indexingFailureRetainedPreparedB':prepared_b},'publicBoundary':'local owner page may show chosen storage path; exported JSON, catalogue and built web assets contain no absolute host paths or source text'},indent=2))
print('PASS real local index A/B, independent source objects and exports')
