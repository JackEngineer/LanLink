import errno
import hashlib
from io import BytesIO
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import sync


class Response(BytesIO):
    def __init__(self, data, headers=None):
        super().__init__(data)
        self.headers = headers or {}


def digest(data):
    return hashlib.sha256(data).hexdigest()


class ReleaseSyncTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.releases = self.root / 'releases'
        self.old = self.releases / 'initial-website'
        self.old.mkdir(parents=True)
        self.current = self.root / 'current'
        self.current.symlink_to(self.old)
        self.live = sync.Version.parse('v0.2.0-preview.2')
        self.initial = sync.Version.parse('v0.2.0-preview.1')
        self.new = sync.Version.parse('v0.2.0-preview.10')
        self.stable = sync.Version.parse('v0.2.0')
        links = '\n'.join(f'<a href="{url}" download>安装包</a>' for url in sync.site_links(self.live))
        self.old_html = (
            f'<span class="release-version">{self.live.tag}</span>\n'
            '<span class="preview-pill">开发预览</span>\n'
            '目前是开发预览，不是 v0.2.0 正式完成版。欢迎在\n'
            f'{links}\n'
        )
        (self.old / 'index.html').write_text(self.old_html, encoding='utf-8')
        (self.old / 'styles.css').write_text('body{color:#123;}', encoding='utf-8')
        self.releases_api = []
        self.remote = {}
        self.impair_public = False
        self.impair_asset_api = False
        self.addCleanup(patch.stopall)
        patch('sync.open_url', side_effect=self.open_url).start()
        for version in (self.initial, self.live):
            directory = self.old / 'downloads' / version.tag
            directory.mkdir(parents=True)
            for name, data in self.package_data(version).items():
                (directory / name).write_bytes(data)

    def package_data(self, version):
        packages = {name: f'{version.tag}/{name}'.encode() for name in version.packages()}
        packages[sync.MANIFEST] = ''.join(f'{digest(data)}  {name}\n' for name, data in packages.items()).encode()
        return packages

    def add_release(self, version, *, missing=None, draft=False, corrupt=None, digest_mismatch=False):
        assets = []
        for asset_id, (name, data) in enumerate(self.package_data(version).items(), start=5 * len(self.releases_api) + 1):
            if name == missing:
                continue
            url = f'https://github.com/{sync.REPO}/releases/download/{version.tag}/{name}'
            api_url = f'{sync.ASSET_API}/{asset_id}'
            asset = {
                'name': name, 'browser_download_url': url, 'size': len(data),
                'id': asset_id, 'url': api_url,
                'state': 'uploaded', 'digest': f'sha256:{digest(data)}',
            }
            if name == corrupt:
                data = b'x' * len(data)
                if not digest_mismatch:
                    asset['digest'] = f'sha256:{digest(data)}'
            self.remote[url] = data
            self.remote[api_url] = data
            assets.append(asset)
        self.releases_api.append({'tag_name': version.tag, 'draft': draft, 'prerelease': version.preview,
                                  'published_at': '2026-11-17T00:00:00Z', 'assets': assets})

    def open_url(self, url, method='GET', **kwargs):
        if url.startswith(f'{sync.ASSET_API}/'):
            if self.impair_asset_api:
                raise TimeoutError('temporary asset API timeout')
            self.assertEqual(kwargs.get('headers'), {'Accept': 'application/octet-stream'})
            return Response(self.remote[url])
        if url.startswith(f'{sync.API}?'):
            return Response(json.dumps(self.releases_api).encode())
        if url.startswith(f'{sync.SITE}/'):
            if self.impair_public:
                raise OSError('temporary public endpoint failure')
            name = url.removeprefix(f'{sync.SITE}/') or 'index.html'
            file = self.current / name
            return Response(file.read_bytes() if method == 'GET' else b'', {'Content-Length': str(file.stat().st_size)})
        return Response(self.remote[url])

    def test_publishes_batch_at_newest_and_preserves_every_version_url(self):
        self.add_release(self.stable)
        self.add_release(self.new)
        self.add_release(sync.Version.parse('v1.0.0'), draft=True)
        sync.sync(self.root)
        active = self.current.resolve()
        self.assertNotEqual(active, self.old)
        html = (active / 'index.html').read_text(encoding='utf-8')
        self.assertIn(self.stable.tag, html)
        self.assertIn('正式发布', html)
        self.assertEqual((active / 'styles.css').read_bytes(), (self.old / 'styles.css').read_bytes())
        sync.verify_links(html, self.stable)
        for version in (self.initial, self.live, self.new, self.stable):
            for name, data in self.package_data(version).items():
                path = self.current / 'downloads' / version.tag / name
                with self.subTest(tag=version.tag, name=name):
                    self.assertEqual(path.read_bytes(), data)
                    if version in (self.initial, self.live):
                        self.assertTrue(path.samefile(self.old / 'downloads' / version.tag / name))
        sync.sync(self.root, check=True)
        self.assertEqual(self.current.resolve(), active)
        self.assertEqual(len(list(self.releases.iterdir())), 2)

    def test_retains_live_site_on_manifest_or_github_digest_failure(self):
        self.add_release(self.new)
        for digest_mismatch in (False, True):
            with self.subTest(digest_mismatch=digest_mismatch):
                self.releases_api.pop()
                self.add_release(self.new, corrupt=self.new.packages()[0], digest_mismatch=digest_mismatch)
                with self.assertRaisesRegex(sync.SyncError, 'SHA256SUMS mismatch|GitHub SHA-256 mismatch'):
                    sync.sync(self.root)
                self.assertEqual(self.current.resolve(), self.old)
                self.assertEqual(list(self.releases.iterdir()), [self.old])
        self.releases_api.pop()
        self.add_release(self.new)
        sync.sync(self.root)
        self.assertNotEqual(self.current.resolve(), self.old)

    def test_incomplete_latest_blocks_older_staged_release(self):
        self.add_release(self.new)
        self.add_release(self.stable, missing=sync.MANIFEST)
        with self.assertRaisesRegex(sync.SyncError, 'missing release assets: SHA256SUMS'):
            sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)
        self.assertEqual(list(self.releases.iterdir()), [self.old])

    def test_stale_release_does_not_downgrade(self):
        self.add_release(self.initial)
        sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)
        self.assertEqual(list(self.releases.iterdir()), [self.old])

    def test_falls_back_to_release_download_on_asset_api_timeout(self):
        self.add_release(self.new)
        self.impair_asset_api = True
        sync.sync(self.root)
        for name, data in self.package_data(self.new).items():
            self.assertEqual((self.current / 'downloads' / self.new.tag / name).read_bytes(), data)

    def test_reuses_verified_downloads_retained_during_website_rollback(self):
        self.add_release(self.new)
        downloads = self.old / 'downloads' / self.new.tag
        downloads.mkdir()
        for name, data in self.package_data(self.new).items():
            (downloads / name).write_bytes(data)
        self.remote.clear()
        sync.sync(self.root)
        self.assertNotEqual(self.current.resolve(), self.old)
        for name, data in self.package_data(self.new).items():
            path = self.current / 'downloads' / self.new.tag / name
            self.assertEqual(path.read_bytes(), data)
            self.assertTrue(path.samefile(downloads / name))

    def test_dry_run_and_protected_hardlinks_preserve_previous_files(self):
        self.add_release(self.new)
        sync.sync(self.root, check=True)
        self.assertEqual(self.current.resolve(), self.old)
        self.assertEqual(list(self.releases.iterdir()), [self.old])
        with patch('sync.os.link', side_effect=PermissionError(errno.EPERM, 'protected hardlink')):
            sync.sync(self.root)
        for version in (self.initial, self.live):
            for name, data in self.package_data(version).items():
                self.assertEqual((self.current / 'downloads' / version.tag / name).read_bytes(), data)

    def test_public_check_failure_keeps_all_urls_and_next_run_rechecks(self):
        self.add_release(self.new)
        self.impair_public = True
        with self.assertRaisesRegex(OSError, 'public endpoint failure'):
            sync.sync(self.root)
        active = self.current.resolve()
        self.assertNotEqual(active, self.old)
        for version in (self.live, self.new):
            for name, data in self.package_data(version).items():
                self.assertEqual((self.current / 'downloads' / version.tag / name).read_bytes(), data)
        self.impair_public = False
        sync.sync(self.root)
        self.assertEqual(self.current.resolve(), active)
        self.assertEqual(len(list(self.releases.iterdir())), 2)

    def test_manifest_requires_exact_files_and_disallows_paths(self):
        manifest = self.package_data(self.new)[sync.MANIFEST]
        first = self.new.packages()[0].encode()
        with self.assertRaisesRegex(sync.SyncError, 'Malformed SHA256SUMS entry'):
            sync.read_manifest(manifest.replace(first, b'../' + first), self.new.packages())
        with self.assertRaisesRegex(sync.SyncError, 'Unexpected or duplicate'):
            sync.read_manifest(manifest + manifest, self.new.packages())


if __name__ == '__main__':
    unittest.main()
