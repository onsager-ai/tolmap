"""CI only: real binary/process, local Git objects, production exported JSON.
Leaves the isolated loopback process running for the production browser checks.
"""
import json, os, pathlib, re, shutil, signal, subprocess, time, urllib.request, urllib.error
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
def index():
    code,body,_=req('/local/refresh',True);assert code==200
    prepared=re.search(rb'action=/local/index\?id=([a-f0-9-]+)',body).group(1).decode()
    assert req('/local/index?id='+prepared,True)[0]==200
    for _ in range(900):
        text=req('/')[1]
        if b'Cancel indexing' not in text:break
        time.sleep(.1)
    else:raise AssertionError('index timeout')
    paths=re.findall(rb'href=/snapshot/([a-f0-9-]+)',text)
    assert paths,('no snapshot published', (root/'service.log').read_text()[-3000:],text[-1500:])
    return {x.decode() for x in paths}
first=index();assert len(first)==1;sid=next(iter(first));base='/api/maps/snapshot/'+sid
provenance=get(base+'/provenance');assert provenance['commit']==a
original=get(base+'/source?path=shop/cart.py&commit='+a);assert original['status']=='ready'
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
(repo/'shop/cart.py').write_text((repo/'shop/cart.py').read_text()+'\n# committed B\n');git('commit','-am','B');b=git('rev-parse','HEAD')
second=index();bid=next(iter(second-first));assert get('/api/maps/snapshot/'+bid+'/provenance')['commit']==b
assert get(base+'/source?path=shop/cart.py')['text']==original['text']
# Pause the actual engine child, then require cancellation to kill/reap it
# rather than waiting for stdout to reach EOF. Retry only if observation missed
# the tiny fixture; a missed worker is never a cancellation pass.
def descendants(parent):
    seen=set();pending=[parent]
    while pending:
        current=pending.pop()
        for children in pathlib.Path(f'/proc/{current}/task').glob('*/children'):
            try:ids=[int(x) for x in children.read_text().split()]
            except FileNotFoundError:continue
            for pid in ids:
                if pid not in seen:seen.add(pid);pending.append(pid)
    return seen
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
index()
ordered=[json.loads(x.read_text())['sequence'] for x in (root/'store/snapshots').glob('*/snapshot.json')]
assert sorted(ordered)==list(range(1,len(ordered)+1))
# Even removal of the original checkout cannot replace/expire stored A.
shutil.move(repo,root/'moved-checkout')
assert get(base+'/source?path=shop/cart.py')['text']==original['text']
# Inspect real process/worker outbound connect syscalls, including failures.
network_trace=os.environ.get('TOLMAP_TEST_NETWORK_TRACE')
if network_trace:
    trace=pathlib.Path(network_trace).read_text()
    internet=[line for line in trace.splitlines() if 'sa_family=AF_INET' in line]
    assert not internet, ('unexpected network connect',internet)
    pathlib.Path('reader-evidence').mkdir(exist_ok=True)
    pathlib.Path('reader-evidence/network.json').write_text(json.dumps({'scope':'actual local service and descendants through A/B, read, export, cancellation; connect syscalls only','internetConnects':internet,'traceBytes':len(trace)},indent=2))
# No private map/source was copied to the already-built public web assets.
for output in pathlib.Path('web/dist').rglob('*'):
    if output.is_file():
        data=output.read_bytes()
        assert original['text'].encode() not in data and sid.encode() not in data
metadata={'a':a,'b':b,'snapshotA':sid,'snapshotB':bid,'file':'shop/cart.py','pid':p.pid,'repository':provenance['repository']}
(root/'fixture.json').write_text(json.dumps(metadata))
pathlib.Path('reader-evidence').mkdir(exist_ok=True)
pathlib.Path('reader-evidence/export-map.json').write_text(json.dumps(export,indent=2))
pathlib.Path('reader-evidence/process.json').write_text(json.dumps({'result':'passed','scope':'real local binary, worker process, Git A/B, private storage, exports, host/origin/path boundaries','fixture':metadata},indent=2))
print('PASS real local index A/B, independent source objects and exports')
