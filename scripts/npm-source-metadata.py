"""CI-only verification of the npm package and its separate source offer."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tarfile
import urllib.request

BUDGET = 67_108_864


def digest_file(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def checksums(path):
    entries = [line.split() for line in path.read_text().splitlines()]
    assert all(len(entry) == 2 and re.fullmatch(r'[0-9a-f]{64}', entry[0]) for entry in entries), path
    result = {name: digest for digest, name in entries}
    assert len(result) == len(entries), f'duplicate checksum entries in {path}'
    return result


def verify_artifact(directory, sha, release=False):
    root = Path(directory)
    assert re.fullmatch(r'[0-9a-f]{40}', sha), 'invalid expected source commit'
    metadata = json.loads((root/'SOURCE.json').read_text())
    assert metadata['source_commit'] == sha, 'source commit mismatch'
    version = metadata['package_version']
    assert re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', version), 'invalid version'
    asset = f'tolmap-{version}-corresponding-source-{sha}.tar.gz'
    assert metadata['source_asset'] == asset, 'source asset name mismatch'
    url = f'https://github.com/onsager-ai/tolmap/releases/download/v{version}/{asset}'
    assert metadata['url'] == url, 'source URL must be the designated durable release asset'
    assert re.fullmatch(r'[0-9a-f]{64}', metadata['sha256']), 'invalid source SHA-256'
    assert type(metadata['size_bytes']) is int and metadata['size_bytes'] > 0, 'invalid source size'
    assert metadata['components']['native'] == [dict(name='igraph', version='1.0.0'),
                                                dict(name='libleidenalg', version='0.12.0')]
    ubuntu = metadata['components']['ubuntu']
    specs = [f'{package["name"]}={package["version"]}' for package in ubuntu]
    assert specs == sorted(set(specs)), 'Ubuntu source specs must be unique and sorted'
    tarball = root/f'tolmap-{version}.tgz'
    assert list(root.glob('tolmap-*.tgz')) == [tarball], 'expected exactly one npm tarball'
    assert tarball.stat().st_size <= BUDGET, 'npm tarball exceeds 64 MiB budget'
    with tarfile.open(tarball, 'r:gz') as package:
        members = package.getmembers()
        assert not any(m.name == 'package/source' or m.name.startswith('package/source/') for m in members), 'npm tarball contains source/'
        packaged = json.load(package.extractfile('package/SOURCE.json'))
        assert packaged == metadata, 'tarball SOURCE.json differs from CI metadata'
        assert json.load(package.extractfile('package/package.json'))['version'] == version
        assert package.extractfile('package/SOURCE-BUILD.md'), 'missing build instructions'
        build_info = package.extractfile('package/BUILD_INFO.md').read()
        assert build_info == (root/'BUILD_INFO.md').read_bytes(), 'BUILD_INFO mismatch'
        assert f'- Source commit: {sha}\n'.encode() in build_info
        unpacked = sum(member.size for member in members if member.isfile())
    source = root/asset
    assert source.stat().st_size == metadata['size_bytes'], 'CI source size mismatch'
    assert digest_file(source) == metadata['sha256'], 'CI source hash mismatch'
    assert checksums(root/'SOURCE_SHA256SUMS') == {asset: metadata['sha256']}, 'source checksum manifest mismatch'
    assert checksums(root/'SHA256SUMS') == {tarball.name: digest_file(tarball), asset: metadata['sha256']}, 'artifact checksum manifest mismatch'
    assert json.loads((root/'PACKAGE_SIZES.json').read_text()) == dict(
        tarball_bytes=tarball.stat().st_size, unpacked_package_bytes=unpacked,
        source_bundle_bytes=metadata['size_bytes']), 'package size record mismatch'
    # Read only in CI; source contents and acquisition remain unchanged.
    with tarfile.open(source, 'r:gz') as archive:
        assert archive.extractfile('corresponding-source/SOURCE_SHA').read().decode().strip() == sha, 'archive source commit mismatch'
        if specs:
            recorded = archive.extractfile('corresponding-source/ubuntu/PACKAGES.txt')
            assert sorted(set(recorded.read().decode().splitlines())) == specs, 'Ubuntu source component mismatch'
    if release:
        digest = hashlib.sha256()
        size = 0
        # HTTP errors (including 404) propagate. Never upload, replace or bless an asset.
        with urllib.request.urlopen(url, timeout=120) as response:
            for chunk in iter(lambda: response.read(1024 * 1024), b''):
                size += len(chunk)
                assert size <= metadata['size_bytes'], 'release source exceeds pinned size'
                digest.update(chunk)
        assert size == metadata['size_bytes'], 'release source size mismatch'
        assert digest.hexdigest() == metadata['sha256'], 'release source hash mismatch'
    return metadata


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory')
    parser.add_argument('sha')
    parser.add_argument('--release', action='store_true', help='also download and verify the durable public asset')
    args = parser.parse_args()
    print(json.dumps(verify_artifact(args.directory, args.sha, args.release), indent=2))
