"""CI only: real binary/process, local Git objects, production exported JSON.
Leaves the isolated loopback process running for the production browser checks.
"""
import json, os, pathlib, re, shutil, subprocess, time, urllib.request, urllib.error
root=pathlib.Path('.local-reader').resolve();root.mkdir(exist_ok=True)
repo=root/'checkout';shutil.copytree('tests/fixtures/service_identity',repo)
def git(*args):return subprocess.check_output(['git','-C',str(repo),*args],stderr=subprocess.DEVNULL).decode().strip()
git('init');git('config','user.name','Fixture');git('config','user.email','fixture@example.invalid')
(repo/'binary.dat').write_bytes(b'\0\x01');(repo/'large.txt').write_text('x'*(1024*1024+1));(repo/'long.txt').write_text('x'*21000)
(repo/'inert.txt').write_text('<script>window.sourceInjected=true</script>\n')
(repo/'escape').symlink_to('/etc/passwd')
git('add','.');git('commit','-m','A');a=git('rev-parse','HEAD')
binary=pathlib.Path(os.environ.get('TOLMAP_BINARY','target/release/tolmap')).resolve()
origin='http://127.0.0.1:8988'
log=open(root/'service.log','wb')
p=subprocess.Popen([str(binary),'local',str(repo),'--data',str(root/'store'),'--static-dir',str(pathlib.Path('web/dist').resolve()),'--port','8988'],stdout=log,stderr=log,start_new_session=True)
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
    assert paths,'no snapshot published'
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
export=get(base+'/artifact');assert export['F'] and 'text' not in export and original['text'] not in json.dumps(export)
(repo/'shop/cart.py').write_text((repo/'shop/cart.py').read_text()+'\n# committed B\n');git('commit','-am','B');b=git('rev-parse','HEAD')
second=index();bid=next(iter(second-first));assert get('/api/maps/snapshot/'+bid+'/provenance')['commit']==b
assert get(base+'/source?path=shop/cart.py')['text']==original['text']
# Even removal of the original checkout cannot replace/expire stored A.
shutil.move(repo,root/'moved-checkout')
assert get(base+'/source?path=shop/cart.py')['text']==original['text']
metadata={'a':a,'b':b,'snapshotA':sid,'snapshotB':bid,'file':'shop/cart.py','pid':p.pid,'repository':provenance['repository']}
(root/'fixture.json').write_text(json.dumps(metadata))
pathlib.Path('reader-evidence').mkdir(exist_ok=True)
pathlib.Path('reader-evidence/process.json').write_text(json.dumps({'result':'passed','scope':'real local binary, worker process, Git A/B, private storage, exports, host/origin/path boundaries','fixture':metadata},indent=2))
print('PASS real local index A/B, independent source objects and exports')
