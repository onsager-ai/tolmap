"""Small CI fixtures for source-offer refusal conditions; no native build."""
import copy
import io
import json
from pathlib import Path
import runpy
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

verifier = runpy.run_path(str(Path(__file__).with_name('npm-source-metadata.py')))
verify = verifier['verify_artifact']
digest_file = verifier['digest_file']
SHA = 'a' * 40
ASSET = f'tolmap-0.1.0-corresponding-source-{SHA}.tar.gz'


def archive(path, files):
    with tarfile.open(path, 'w:gz') as output:
        for name, content in files.items():
            member = tarfile.TarInfo(name)
            member.size = len(content)
            output.addfile(member, io.BytesIO(content))


class SourceOffer(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.TemporaryDirectory()
        self.addCleanup(self.work.cleanup)
        self.root = Path(self.work.name)
        archive(self.root/ASSET, {'corresponding-source/SOURCE_SHA': f'{SHA}\n'.encode(),
                                 'corresponding-source/ubuntu/PACKAGES.txt': b'gmp=1\n'})
        self.metadata = dict(source_commit=SHA, package_version='0.1.0', source_asset=ASSET,
                             url=f'https://github.com/onsager-ai/tolmap/releases/download/v0.1.0/{ASSET}',
                             sha256=digest_file(self.root/ASSET), size_bytes=(self.root/ASSET).stat().st_size,
                             components=dict(native=[dict(name='igraph', version='1.0.0'),
                                                     dict(name='libleidenalg', version='0.12.0')],
                                             ubuntu=[dict(name='gmp', version='1')]))
        self.files = {'package/SOURCE.json': json.dumps(self.metadata).encode(),
                      'package/package.json': b'{"version":"0.1.0"}',
                      'package/SOURCE-BUILD.md': b'build instructions',
                      'package/BUILD_INFO.md': f'- Source commit: {SHA}\n'.encode()}
        self.write_fixture()

    def write_fixture(self):
        root = self.root
        archive(root/'tolmap-0.1.0.tgz', self.files)
        (root/'SOURCE.json').write_text(json.dumps(self.metadata))
        (root/'BUILD_INFO.md').write_bytes(self.files['package/BUILD_INFO.md'])
        source_sum = f'{self.metadata["sha256"]}  {ASSET}\n'
        (root/'SOURCE_SHA256SUMS').write_text(source_sum)
        (root/'SHA256SUMS').write_text(f'{digest_file(root/"tolmap-0.1.0.tgz")}  tolmap-0.1.0.tgz\n' + source_sum)
        sizes = dict(tarball_bytes=(root/'tolmap-0.1.0.tgz').stat().st_size,
                     unpacked_package_bytes=sum(map(len, self.files.values())),
                     source_bundle_bytes=self.metadata['size_bytes'])
        (root/'PACKAGE_SIZES.json').write_text(json.dumps(sizes))

    def test_valid_and_exact_public_asset(self):
        self.assertEqual(verify(self.root, SHA), self.metadata)
        with patch('urllib.request.urlopen', return_value=io.BytesIO((self.root/ASSET).read_bytes())):
            self.assertEqual(verify(self.root, SHA, release=True), self.metadata)

    def test_metadata_refusals(self):
        for field, value in [('source_commit', 'b'*40), ('sha256', '0'*64),
                             ('size_bytes', self.metadata['size_bytes'] + 1),
                             ('url', 'https://example.com/source.tar.gz')]:
            with self.subTest(field=field):
                changed = copy.deepcopy(self.metadata)
                changed[field] = value
                (self.root/'SOURCE.json').write_text(json.dumps(changed))
                with self.assertRaises(AssertionError):
                    verify(self.root, SHA)

    def test_source_directory_refused(self):
        self.files['package/source/old-source.tar.gz'] = b'old source'
        self.write_fixture()
        with self.assertRaisesRegex(AssertionError, 'contains source/'):
            verify(self.root, SHA)

    def test_budget_refused(self):
        # Exercise the size guard with the tiny real fixture above a patched cap.
        with patch.dict(verify.__globals__, BUDGET=1):
            with self.assertRaisesRegex(AssertionError, '64 MiB'):
                verify(self.root, SHA)

    def test_source_manifest_refused(self):
        (self.root/'SOURCE_SHA256SUMS').write_text(f'{"0"*64}  {ASSET}\n')
        with self.assertRaisesRegex(AssertionError, 'source checksum manifest'):
            verify(self.root, SHA)

    def test_changed_ci_source_refused(self):
        source = self.root/ASSET
        data = source.read_bytes()
        source.write_bytes(data[:-1] + bytes([data[-1] ^ 1]))
        with self.assertRaisesRegex(AssertionError, 'CI source hash mismatch'):
            verify(self.root, SHA)

    def test_source_archive_commit_refused(self):
        archive(self.root/ASSET, {'corresponding-source/SOURCE_SHA': b'wrong commit\n',
                                 'corresponding-source/ubuntu/PACKAGES.txt': b'gmp=1\n'})
        self.metadata['sha256'] = digest_file(self.root/ASSET)
        self.metadata['size_bytes'] = (self.root/ASSET).stat().st_size
        self.files['package/SOURCE.json'] = json.dumps(self.metadata).encode()
        self.write_fixture()
        with self.assertRaisesRegex(AssertionError, 'archive source commit mismatch'):
            verify(self.root, SHA)

    def test_public_404_or_changed_bytes_refused(self):
        with patch('urllib.request.urlopen', side_effect=urllib.error.HTTPError(self.metadata['url'], 404, 'missing', {}, None)):
            with self.assertRaises(urllib.error.HTTPError):
                verify(self.root, SHA, release=True)
        for data in (b'', b'x' * self.metadata['size_bytes'], b'x' * (self.metadata['size_bytes'] + 1)):
            with self.subTest(size=len(data)), patch('urllib.request.urlopen', return_value=io.BytesIO(data)):
                with self.assertRaises(AssertionError):
                    verify(self.root, SHA, release=True)


if __name__ == '__main__':
    unittest.main()
