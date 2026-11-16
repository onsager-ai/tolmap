"""CI only: real binary/process, personal library, retained Git objects, production exported JSON.
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
env={**os.environ,'TOLMAP_DATA_DIR':str(root/'store'),'TOLMAP_REFS':'hand'}
if os.environ.get('TOLMAP_TEST_PACKAGED_DEFAULT_STATIC') != '1':
    env['TOLMAP_STATIC_DIR']=str(pathlib.Path('web/dist').resolve())
web_args=[str(binary),'web','--addr','127.0.0.1:8988']
if os.environ.get('TOLMAP_TEST_WRITE_TRACE'):
    web_args=['strace','-f','-qq','-e','trace=write','-s','1024','-o',os.environ['TOLMAP_TEST_WRITE_TRACE'],*web_args]
if os.environ.get('TOLMAP_TEST_NETWORK_TRACE'):
    web_args=['strace','-f','-qq','-e','trace=connect','-s','256','-o',os.environ['TOLMAP_TEST_NETWORK_TRACE'],*web_args]
p=subprocess.Popen(web_args,stdout=log,stderr=log,start_new_session=True,env=env)
pathlib.Path('reader-evidence').mkdir(exist_ok=True)
def cli(*args,success=True):
    result=subprocess.run([str(binary),*args],env=env,capture_output=True,text=True)
    assert (result.returncode==0)==success,(args,result.returncode,result.stderr[-3000:])
    return json.loads(result.stdout) if result.stdout.strip() else None
def req(path,method='GET',body=None,headers=None):
    request=urllib.request.Request(origin+path,method=method,data=json.dumps(body).encode() if body is not None else None,headers={'Content-Type':'application/json','Origin':origin,**(headers or {})})
    try:
        with urllib.request.urlopen(request,timeout=20) as response:return response.status,response.read(),response.headers
    except urllib.error.HTTPError as error:return error.code,error.read(),error.headers
def get(path):
    code,body,_=req(path);assert code==200,(code,body[:500]);return json.loads(body)
def post(path,body=None):
    code,data,_=req(path,'POST',body or {});assert code in (200,202),(code,data[:500]);return json.loads(data)
for _ in range(200):
    try:
        if req('/api/healthz')[0]==200:break
    except OSError:pass
    assert p.poll() is None,'web process exited'
    time.sleep(.1)
else:raise AssertionError('web startup timeout')
def wait_job(job):
    started=time.monotonic()
    for _ in range(1200):
        value=get('/api/jobs/'+job)
        if value['status'] in ('done','failed'):
            assert value['status']=='done',value
            return value
        time.sleep(.1)
    raise AssertionError(('index timeout',time.monotonic()-started,(root/'service.log').read_text()[-3000:]))
# CLI registration/indexing immediately appears in the already running web app.
cli('add',str(repo),'--name','reader')
rows=get('/api/maps');assert len(rows)==1,rows
repository_id=rows[0]['repository_id'];sid=rows[0]['latest_snapshot_id']
history_path='/api/repositories/'+repository_id+'/history'
def base_for(snapshot):return '/api/maps/snapshot/'+snapshot
base=base_for(sid)
original=get(base+'/source?path=shop/cart.py&commit='+a)
assert original['text']==committed_source,'dirty tracked source entered the snapshot'
assert get(base+'/source?path=untracked-local.py&commit='+a)['status']=='missing'
provenance=get(base+'/provenance');assert provenance['commit']==a
export=get(base);assert export['districts']
assert req('/api/maps/snapshot/'+sid+'/source?path=shop/cart.py')[0]==400
assert req('/api/maps',headers={'Host':'attacker.invalid'})[0]==403
assert req(history_path.replace('/history','/refresh'),'POST',{}, {'Content-Type':'text/plain'})[0]==415
# Stored source is independent from dirty files, checkout GC and deletion.
git('reset','--hard',a);(repo/'untracked-local.py').unlink()
(repo/'shop/cart.py').write_text(committed_source+'\n# committed B\n')
git('add','.');git('commit','-m','B');b=git('rev-parse','HEAD')
assert get(base+'/freshness')['state']=='stale'
# Exercise management writes through HTTP and reopen both retained revisions.
job=post('/api/repositories/'+repository_id+'/refresh')['job_id'];wait_job(job)
history=get(history_path);bid=history['snapshots'][0]['id'];assert bid!=sid,history
assert get(base_for(bid)+'/provenance')['commit']==b
assert get(base+'/source?path=shop/cart.py&commit='+a)['text']==committed_source
# A failed CLI refresh retains the latest map and web reads the failed attempt.
cli('refresh','reader','--pkg','missing','--lang','py',success=False)
assert get(history_path)['snapshots'][0]['id']==bid
assert get(base_for(bid)+'/freshness')['state']=='failed'
# Observe the real worker child and cancel it through the personal API.
(repo/'shop/cart.py').write_text(committed_source+'\n# cancellation C\n')
git('add','.');git('commit','-m','C')
job=post('/api/repositories/'+repository_id+'/refresh')['job_id']
def worker_pids():
    result=[]
    for entry in pathlib.Path('/proc').iterdir():
        if not entry.name.isdigit():continue
        try:
            args=(entry/'cmdline').read_bytes().split(b'\0')
            if len(args)>1 and args[1]==b'worker' and b'tolmap' in args[0]:result.append(int(entry.name))
        except (OSError,ProcessLookupError):pass
    return result
for _ in range(1000):
    workers=worker_pids()
    if workers:break
    assert get('/api/jobs/'+job)['status'] not in ('done','failed'),'worker finished before cancellation qualification'
    time.sleep(.01)
else:raise AssertionError('actual indexing worker was never observed')
post('/api/jobs/'+job+'/cancel')
for _ in range(300):
    if not any(pathlib.Path('/proc/'+str(pid)).exists() for pid in workers):break
    time.sleep(.05)
else:raise AssertionError('cancelled worker was not reaped')
assert get(history_path)['snapshots'][0]['id']==bid
for _ in range(300):
    if get('/api/jobs/'+job)['status']=='failed':break
    time.sleep(.05)
else:raise AssertionError('cancelled job did not become terminal')
# Reset to B so the browser can refresh without creating a new revision.
git('reset','--hard',b);git('reflog','expire','--expire=now','--all');git('gc','--prune=now')
assert get(base+'/source?path=shop/cart.py&commit='+a)['text']==committed_source
moved=root/'moved-checkout';repo.rename(moved)
assert get(base+'/source?path=shop/cart.py&commit='+a)['text']==committed_source
moved.rename(repo)
# Removing one retained source must fail explicitly and never substitute B.
import sqlite3
with sqlite3.connect(root/'store/tolmap.sqlite3') as db:
    source_path=pathlib.Path(db.execute('SELECT source_path FROM personal_snapshots WHERE id=?',(sid,)).fetchone()[0])
off=source_path.with_name('source.offline');source_path.rename(off)
try:
    assert get(base+'/source?path=shop/cart.py&commit='+a)['status']=='expired'
    assert get(base_for(bid)+'/source?path=shop/cart.py&commit='+b)['status']=='ready'
finally:off.rename(source_path)
for district in export['districts']:
    symbols=get(base+'/symbols?district='+str(district));assert symbols['district']==int(district)
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
host_paths=(str(pathlib.Path.cwd().resolve()),str(root),str(repo))
for value in (json.dumps(export),json.dumps(provenance)):
    assert all(path not in value for path in host_paths) and committed_source not in value
for output in pathlib.Path('web/dist').rglob('*'):
    if output.is_file():
        data=output.read_bytes();assert committed_source.encode() not in data and sid.encode() not in data
metadata={'a':a,'b':b,'snapshotA':sid,'snapshotB':bid,'file':'shop/cart.py','pid':p.pid,'repository':provenance['repository'],'repositoryId':repository_id,'sourceSha256':hashlib.sha256(committed_source.encode()).hexdigest()}
(root/'fixture.json').write_text(json.dumps(metadata))
pathlib.Path('reader-evidence/export-map.json').write_text(json.dumps(export,indent=2))
pathlib.Path('reader-evidence/process.json').write_text(json.dumps({'result':'passed','scope':'personal CLI and web shared state, Git A/B, dirty/untracked exclusion, failed refresh recovery, real worker cancellation, checkout GC/move, missing source, private exports','fixture':metadata},indent=2))
print('PASS personal library A/B, retained source and cancellation')
