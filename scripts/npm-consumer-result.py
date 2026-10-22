"""CI consumer completion oracle; the host independently validates this record."""
import hashlib
import json
import os
from pathlib import Path
import sys

CHECKS = ['clean-runtime', 'artifact-identity', 'package-contents', 'cli',
          'unsupported-platform', 'offline-parity', 'reader-cancellation',
          'real-rs-build', 'viewer-assets-source-loopback', 'launcher-signals']

def tgz_hash(directory):
    files = list(Path(directory).glob('tolmap-*.tgz'))
    assert len(files) == 1, files
    digest = hashlib.file_digest(files[0].open('rb'), 'sha256').hexdigest() if sys.version_info >= (3, 11) else None
    if digest is None:
        h = hashlib.sha256()
        with files[0].open('rb') as f:
            for chunk in iter(lambda: f.read(1024 * 1024), b''):
                h.update(chunk)
        digest = h.hexdigest()
    return digest

if sys.argv[1] == 'write':
    checks = json.loads(Path('/tmp/consumer-checks.json').read_text())
    assert set(checks) == set(CHECKS) and all(v == 'passed' for v in checks.values())
    result = dict(nonce=os.environ['CONSUMER_NONCE'], sha=os.environ['EXPECTED_SHA'],
                  tarball_sha256=tgz_hash('/artifacts'), node=os.environ['NODE_VERSION'], checks=checks,
                  build=json.loads(Path('/tmp/build-counts.json').read_text()),
                  viewer=json.loads(Path('/tmp/viewer-result.json').read_text()),
                  signals=json.loads(Path('/tmp/signals-result.json').read_text()))
    Path('/out/consumer-result.json').write_text(json.dumps(result, indent=2) + '\n')
elif sys.argv[1] == 'verify':
    _, _, file, nonce, sha, artifacts, node = sys.argv
    result = json.loads(Path(file).read_text())
    assert result['nonce'] == nonce
    assert result['sha'] == sha and result['node'] == node
    assert result['tarball_sha256'] == tgz_hash(artifacts)
    assert set(result['checks']) == set(CHECKS)
    assert all(value == 'passed' for value in result['checks'].values())
    assert result['build']['files'] >= 20 and result['build']['districts'] >= 1
    print(json.dumps(result, indent=2))
