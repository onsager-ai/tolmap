"""CI only: real production snapshot, assets, immutable source and bind checks."""
import json
from html.parser import HTMLParser
from pathlib import Path
import socket
import subprocess
import urllib.error
import urllib.request

origin = 'http://127.0.0.1:8988'
meta = json.loads(Path('.local-reader/fixture.json').read_text())
base = '/api/maps/snapshot/' + meta['snapshotA']
def fetch(path):
    return urllib.request.urlopen(origin + path, timeout=20)

class Assets(HTMLParser):
    def __init__(self):
        super().__init__(); self.assets = []
    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == 'script' and a.get('src'): self.assets.append((a['src'], 'javascript'))
        if tag == 'link' and a.get('rel') == 'stylesheet': self.assets.append((a['href'], 'css'))

with fetch('/snapshot/' + meta['snapshotA']) as res:
    assert res.status == 200 and 'text/html' in res.headers['Content-Type']
    parser = Assets(); parser.feed(res.read().decode())
assert parser.assets and any(t == 'javascript' for _, t in parser.assets)
assert any(t == 'css' for _, t in parser.assets)
import re
for url, kind in parser.assets:
    assert re.fullmatch(r'/assets/[^/]+-[A-Za-z0-9_-]+\.(js|css)', url), url
    with fetch(url) as res:
        assert res.status == 200 and kind in res.headers['Content-Type'], (url, res.headers)
        assert len(res.read()) > 10
with fetch(base + '/artifact') as res: assert json.load(res)['F']
with fetch(base + '/source?path=' + meta['file'] + '&commit=' + meta['a']) as res:
    source = json.load(res)
expected = subprocess.check_output(['git', '-C', '.local-reader/moved-checkout', 'show', meta['a'] + ':' + meta['file']])
assert source['status'] == 'ready' and source['text'].encode() == expected
for path in ['/source?path=%2Fetc%2Fpasswd', '/source?path=' + meta['file'] + '&commit=' + '0'*40]:
    try:
        with fetch(base + path) as res: assert json.load(res)['text'] is None
    except urllib.error.HTTPError as e: assert e.code in (400, 403, 404, 422), e.code
listeners = subprocess.check_output(['ss', '-ltnH', 'sport = :8988']).decode().splitlines()
assert listeners and all(line.split()[3] == '127.0.0.1:8988' for line in listeners), listeners
ips = subprocess.check_output(['hostname', '-I']).decode().split()
assert ips
for ip in ips:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(2)
        assert s.connect_ex((ip, 8988)) != 0, ip
Path('/tmp/viewer-result.json').write_text(json.dumps(dict(snapshot=meta['snapshotA'], assets=parser.assets,
    same_commit_source=True, restriction_negatives=2, listeners=listeners, non_loopback_rejected=ips)))
