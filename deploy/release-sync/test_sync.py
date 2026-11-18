import errno
import hashlib
from io import BytesIO
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
import subprocess
import stat
import sys
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

import sync

TRUSTED_FILE = sync.trusted_file


class Response(BytesIO):
    def __init__(self, data, headers=None, status=200):
        super().__init__(data)
        self.headers = headers or {}
        self.status = status

    def read1(self, size=-1):
        return self.read(size)


class InterruptedResponse(Response):
    def read(self, size=-1):
        if self.tell() > 0:
            raise TimeoutError('asset read interrupted')
        return super().read(min(size, 5))


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
        self.commit = '1' * 40
        self.tag_object = None
        self.wrong_ref = False
        self.ancestry = 'ahead'
        self.workflow = self.root / 'trusted-workflow.yml'
        self.workflow.write_bytes(b'approved release workflow')
        workflow = self.workflow.read_bytes()
        self.workflow_blob = hashlib.sha1(f'blob {len(workflow)}\0'.encode() + workflow).hexdigest()
        patch('sync.TRUSTED_WORKFLOW', self.workflow).start()
        self.trust_check = patch('sync.trusted_file').start()
        self.verifier = patch('sync.subprocess.run', side_effect=self.verify_bundle).start()
        for version in (self.initial, self.live):
            directory = self.old / 'downloads' / version.tag
            directory.mkdir(parents=True)
            for name, data in self.package_data(version).items():
                (directory / name).write_bytes(data)

    def package_data(self, version):
        packages = {name: f'{version.tag}/{name}'.encode() for name in version.packages()}
        packages[sync.MANIFEST] = ''.join(f'{digest(data)}  {name}\n' for name, data in packages.items()).encode()
        return packages

    def release_data(self, version):
        packages = self.package_data(version)
        packages[sync.BUNDLE] = json.dumps({
            'repo': sync.REPO, 'workflow': sync.WORKFLOW,
            'ref': f'refs/tags/{version.tag}', 'commit': self.commit,
            'manifest': digest(packages[sync.MANIFEST]), 'hosted': True,
        }).encode()
        return packages

    def verify_bundle(self, command, **kwargs):
        # The real cryptographic verifier is exercised separately; this fake enforces every CLI constraint.
        self.assertEqual(command[:3], [str(sync.GH), 'attestation', 'verify'])
        self.assertEqual(kwargs['timeout'], 120)
        self.assertTrue(kwargs['check'])
        self.assertNotIn('GH_TOKEN', kwargs['env'])
        self.assertNotIn('GITHUB_TOKEN', kwargs['env'])
        self.assertIn('--deny-self-hosted-runners', command)
        self.assertEqual(command[command.index('--custom-trusted-root') + 1], str(sync.TRUSTED_ROOT))
        try:
            bundle = json.loads(Path(command[command.index('--bundle') + 1]).read_bytes())
        except ValueError as error:
            raise subprocess.CalledProcessError(1, command) from error
        ref = command[command.index('--source-ref') + 1]
        commit = command[command.index('--source-digest') + 1]
        policy = {
            'repo': command[command.index('--repo') + 1], 'workflow': sync.WORKFLOW,
            'ref': ref, 'commit': commit, 'manifest': digest(Path(command[3]).read_bytes()), 'hosted': True,
        }
        self.assertEqual(command[command.index('--cert-identity') + 1],
                         f'https://github.com/{sync.REPO}/{sync.WORKFLOW}@{ref}')
        self.assertEqual(command[command.index('--signer-digest') + 1], commit)
        if bundle != policy:
            raise subprocess.CalledProcessError(1, command)
        return subprocess.CompletedProcess(command, 0)

    def add_release(self, version, *, missing=None, draft=False, corrupt=None, digest_mismatch=False):
        assets = []
        for asset_id, (name, data) in enumerate(self.release_data(version).items(), start=6 * len(self.releases_api) + 1):
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
            self.assertEqual(kwargs.get('headers', {}).get('Accept'), 'application/octet-stream')
            return Response(self.remote[url])
        if url.startswith(f'{sync.API}?'):
            return Response(json.dumps(self.releases_api).encode())
        if url.startswith(f'{sync.REPO_API}/git/ref/tags/'):
            tag = url.rsplit('/', 1)[1]
            ref = f'refs/heads/{tag}' if self.wrong_ref else f'refs/tags/{tag}'
            obj = self.tag_object or {'sha': self.commit, 'type': 'commit'}
            return Response(json.dumps({'ref': ref, 'object': obj}).encode())
        if url.startswith(f'{sync.REPO_API}/git/tags/'):
            return Response(json.dumps({'sha': url.rsplit('/', 1)[1],
                                        'object': {'sha': self.commit, 'type': 'commit'}}).encode())
        if url.startswith(f'{sync.REPO_API}/compare/'):
            return Response(json.dumps({'status': self.ancestry,
                                        'merge_base_commit': {'sha': self.commit},
                                        'base_commit': {'sha': self.commit}}).encode())
        if url.startswith(f'{sync.REPO_API}/contents/'):
            self.assertTrue(url.endswith(f'?ref={self.commit}'))
            return Response(json.dumps({'type': 'file', 'path': sync.WORKFLOW, 'sha': self.workflow_blob}).encode())
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
        self.assertEqual(json.loads((active / 'release.json').read_bytes()), {'tag': self.stable.tag})
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
                with self.assertRaisesRegex(sync.SyncError, 'GitHub digest and SHA256SUMS disagree|GitHub SHA-256 mismatch'):
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

    def test_safely_restarts_when_server_ignores_range(self):
        self.add_release(self.new)
        package = self.new.packages()[-1]
        url = next(asset['url'] for asset in self.releases_api[0]['assets'] if asset['name'] == package)
        attempts = []

        def open_url(address, **kwargs):
            if address == url:
                attempts.append(address)
                if len(attempts) == 1:
                    return InterruptedResponse(b'interrupted data')
            return self.open_url(address, **kwargs)

        with patch('sync.open_url', side_effect=open_url):
            sync.sync(self.root)
        self.assertEqual(attempts, [url, url])
        self.assertEqual((self.current / 'downloads' / self.new.tag / package).read_bytes(),
                         self.package_data(self.new)[package])

    def test_only_retries_missing_package_across_failed_runs(self):
        self.add_release(self.new)
        failed = self.new.packages()[-1]
        self.remote_fail = True
        requests = []

        def open_url(address, **kwargs):
            for asset in self.releases_api[0]['assets']:
                if address in (asset['url'], asset['browser_download_url']):
                    requests.append(asset['name'])
                    if self.remote_fail and asset['name'] == failed:
                        raise TimeoutError('temporarily unavailable')
            return self.open_url(address, **kwargs)

        with patch('sync.open_url', side_effect=open_url):
            with self.assertRaisesRegex(sync.SyncError, 'Transfer failed'):
                sync.sync(self.root)
            self.assertEqual(self.current.resolve(), self.old)
            first_requests = set(requests)
            requests.clear()
            self.remote_fail = False
            sync.sync(self.root)
        self.assertEqual(first_requests, {*self.new.assets(), sync.BUNDLE})
        self.assertEqual(set(requests), {sync.MANIFEST, sync.BUNDLE, failed})
        self.assertFalse((self.root / '.release-sync-cache' / self.new.tag).exists())

    def test_prunes_only_stale_canonical_partials_after_manifest_verification(self):
        self.add_release(self.new)
        assets = sync.release_assets(self.new, self.releases_api[0])
        packages = self.package_data(self.new)
        cache = self.root / '.release-sync-cache' / self.new.tag
        cache.mkdir(parents=True)
        for name in self.new.packages():
            (cache / name).write_bytes(packages[name])
        name = self.new.packages()[0]
        asset = assets[name]
        current = sync.partial_path(cache, self.new, name, asset, digest(packages[name]))
        previous_asset = sync.Asset(asset.url, f'{sync.ASSET_API}/99999', asset.size, asset.digest)
        stale = sync.partial_path(cache, self.new, name, previous_asset, digest(packages[name]))
        stale_manifest = sync.partial_path(cache, self.new, name, asset, '0' * 64)
        for path in (current, current.with_suffix('.lock'), stale, stale.with_suffix('.lock'), stale_manifest):
            path.write_bytes(b'keep current only')
        other = cache / '.partial-unrelated'
        other.write_bytes(b'keep')
        canonical_directory = cache / ('.partial-' + 'f' * 64)
        canonical_directory.mkdir()
        destination = self.root / 'staged'
        destination.mkdir()
        with sync.release_lock(self.root):
            sync.prepare_downloads(self.new, assets, destination, cache, packages[sync.MANIFEST])
        self.assertTrue(current.exists())
        self.assertTrue(current.with_suffix('.lock').exists())
        self.assertFalse(stale.exists())
        self.assertFalse(stale.with_suffix('.lock').exists())
        self.assertFalse(stale_manifest.exists())
        self.assertEqual(other.read_bytes(), b'keep')
        self.assertTrue(canonical_directory.is_dir())

    def test_reuses_verified_downloads_retained_during_website_rollback(self):
        self.add_release(self.new)
        downloads = self.old / 'downloads' / self.new.tag
        downloads.mkdir()
        for name, data in self.package_data(self.new).items():
            (downloads / name).write_bytes(data)
        for asset in self.releases_api[0]['assets']:
            if asset['name'] in self.new.packages():
                del self.remote[asset['url']]
                del self.remote[asset['browser_download_url']]
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

    def test_new_release_requires_bundle_and_preserves_legacy_noop(self):
        self.add_release(self.live, missing=sync.BUNDLE)
        sync.sync(self.root, check=True)
        self.verifier.assert_not_called()
        self.add_release(self.new, missing=sync.BUNDLE)
        with self.assertRaisesRegex(sync.SyncError, 'missing release assets: SHA256SUMS.sigstore.jsonl'):
            sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)

    def test_wrong_provenance_claims_block_activation_and_check_mode(self):
        for field, value in [('repo', 'attacker/LanLink'), ('workflow', '.github/workflows/build.yml'),
                             ('ref', 'refs/tags/v9.9.9'), ('commit', '2' * 40), ('hosted', False),
                             ('manifest', '0' * 64)]:
            with self.subTest(field=field):
                self.releases_api.clear()
                self.add_release(self.new)
                item = next(a for a in self.releases_api[0]['assets'] if a['name'] == sync.BUNDLE)
                bundle = json.loads(self.remote[item['url']])
                bundle[field] = value
                data = json.dumps(bundle).encode()
                item.update(size=len(data), digest=f'sha256:{digest(data)}')
                self.remote[item['url']] = self.remote[item['browser_download_url']] = data
                for check in (False, True):
                    with self.assertRaisesRegex(sync.SyncError, 'build provenance verification failed'):
                        sync.sync(self.root, check=check)
                    self.assertEqual(self.current.resolve(), self.old)

    def test_provenance_cannot_be_bypassed_by_existing_directory_or_cache(self):
        self.add_release(self.new, corrupt=sync.BUNDLE)
        for location in (self.old / 'downloads' / self.new.tag,
                         self.root / '.release-sync-cache' / self.new.tag):
            location.mkdir(parents=True)
            for name, data in self.package_data(self.new).items():
                (location / name).write_bytes(data)
        with self.assertRaisesRegex(sync.SyncError, 'build provenance verification failed'):
            sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)
        self.verifier.assert_called_once()

    def test_wrong_ref_unmerged_commit_and_substituted_workflow_are_rejected(self):
        self.add_release(self.new)
        for attribute, value, message in [('wrong_ref', True, 'exact tag ref'),
                                          ('ancestry', 'diverged', 'ancestor of main'),
                                          ('workflow_blob', '0' * 40, 'trusted workflow')]:
            with self.subTest(attribute=attribute):
                old = getattr(self, attribute)
                setattr(self, attribute, value)
                with self.assertRaisesRegex(sync.SyncError, message):
                    sync.sync(self.root)
                setattr(self, attribute, old)
                self.assertEqual(self.current.resolve(), self.old)
        self.verifier.assert_not_called()

    def test_annotated_tags_resolve_and_tag_move_before_activation_is_rejected(self):
        self.add_release(self.new)
        self.tag_object = {'type': 'tag', 'sha': '3' * 40}
        sync.sync(self.root, check=True)
        self.assertEqual(self.current.resolve(), self.old)
        with patch('sync.resolve_tag', side_effect=[self.commit, '2' * 40]):
            with self.assertRaisesRegex(sync.SyncError, 'moved during provenance verification'):
                sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)

    def test_staged_manifest_must_match_fresh_attestation(self):
        self.add_release(self.new)
        location = self.old / 'downloads' / self.new.tag
        location.mkdir()
        for name, data in self.package_data(self.new).items():
            (location / name).write_bytes(data)
        path = location / sync.MANIFEST
        path.write_bytes(path.read_bytes().replace(b'  ', b' *', 1))
        with self.assertRaisesRegex(sync.SyncError, 'differs from the attested manifest'):
            sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)

    def test_package_hash_is_required_without_optional_github_digest(self):
        self.add_release(self.new)
        item = next(a for a in self.releases_api[0]['assets'] if a['name'] == self.new.packages()[0])
        item['digest'] = None
        data = b'x' * item['size']
        self.remote[item['url']] = self.remote[item['browser_download_url']] = data
        with self.assertRaisesRegex(sync.SyncError, 'SHA256SUMS mismatch'):
            sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)

    def test_missing_trust_configuration_fails_before_download_or_activation(self):
        self.add_release(self.new)
        self.trust_check.side_effect = FileNotFoundError('trusted verifier not provisioned')
        with self.assertRaises(FileNotFoundError):
            sync.sync(self.root)
        self.verifier.assert_not_called()
        self.assertEqual(self.current.resolve(), self.old)

    def test_release_lock_prevents_concurrent_timer_sync(self):
        self.add_release(self.new)
        with sync.release_lock(self.root):
            with self.assertRaisesRegex(sync.SyncError, 'already running'):
                sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)
        self.verifier.assert_not_called()

    def test_bundle_size_and_timeout_fail_closed(self):
        self.add_release(self.new)
        item = next(a for a in self.releases_api[0]['assets'] if a['name'] == sync.BUNDLE)
        original = item['size']
        item['size'] = sync.BUNDLE_LIMIT + 1
        with self.assertRaisesRegex(sync.SyncError, 'invalid size'):
            sync.sync(self.root)
        item['size'] = original
        self.verifier.side_effect = subprocess.TimeoutExpired('gh', 120)
        with self.assertRaisesRegex(sync.SyncError, 'TimeoutExpired'):
            sync.sync(self.root)
        self.assertEqual(self.current.resolve(), self.old)

    def test_trust_files_and_parents_require_root_ownership_and_safe_permissions(self):
        def info(path):
            return SimpleNamespace(st_mode=(stat.S_IFREG | 0o644) if path == sync.GH else (stat.S_IFDIR | 0o755),
                                   st_uid=0)
        with patch('sync.Path.lstat', autospec=True, side_effect=info), patch('sync.os.access', return_value=True):
            TRUSTED_FILE(sync.GH, executable=True)
        for mode, owner in [(stat.S_IFREG | 0o664, 0), (stat.S_IFREG | 0o644, 1000),
                            (stat.S_IFLNK | 0o777, 0)]:
            with patch('sync.Path.lstat', return_value=SimpleNamespace(st_mode=mode, st_uid=owner)):
                with self.assertRaisesRegex(sync.SyncError, 'root-owned'):
                    TRUSTED_FILE(sync.GH)
        with patch('sync.Path.lstat', autospec=True,
                   side_effect=lambda p: info(p) if p == sync.GH else SimpleNamespace(st_mode=stat.S_IFDIR | 0o777, st_uid=0)):
            with self.assertRaisesRegex(sync.SyncError, 'root-owned'):
                TRUSTED_FILE(sync.GH)

    def test_manifest_requires_exact_files_and_disallows_paths(self):
        manifest = self.package_data(self.new)[sync.MANIFEST]
        first = self.new.packages()[0].encode()
        with self.assertRaisesRegex(sync.SyncError, 'Malformed SHA256SUMS entry'):
            sync.read_manifest(manifest.replace(first, b'../' + first), self.new.packages())
        with self.assertRaisesRegex(sync.SyncError, 'Unexpected or duplicate'):
            sync.read_manifest(manifest + manifest, self.new.packages())


class PackageResumeTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.cache = Path(temporary.name)
        self.version = sync.Version.parse('v0.2.0-preview.10')
        self.name = self.version.packages()[0]
        self.data = b'0123456789' * 100
        self.digest = digest(self.data)
        self.asset = sync.Asset(f'https://github.com/{sync.REPO}/releases/download/{self.version.tag}/{self.name}',
                                f'{sync.ASSET_API}/123', len(self.data), self.digest)
        self.partial = sync.partial_path(self.cache, self.version, self.name, self.asset, self.digest)

    def ranged(self, start, **headers):
        return Response(self.data[start:], {
            'Content-Range': f'bytes {start}-{len(self.data) - 1}/{len(self.data)}',
            'Content-Length': str(len(self.data) - start), **headers,
        }, status=206)

    def test_retries_from_received_offset_with_fresh_official_request(self):
        requests = []

        def open_url(url, **kwargs):
            requests.append((url, kwargs['headers']))
            if len(requests) == 1:
                return InterruptedResponse(self.data)
            return self.ranged(5)

        with patch('sync.open_url', side_effect=open_url):
            sync.download_package(self.asset, self.partial, self.digest)
        self.assertEqual(self.partial.read_bytes(), self.data)
        self.assertNotIn('Range', requests[0][1])
        self.assertEqual(requests[1], (self.asset.api_url, {
            'Accept': 'application/octet-stream', 'Cache-Control': 'no-cache', 'Range': 'bytes=5-',
        }))
        self.assertEqual(stat.S_IMODE(self.partial.stat().st_mode), 0o600)

    def test_no_progress_and_ignored_range_timeout_preserve_previous_prefix(self):
        prefix = self.data[:100]
        for response in (InterruptedResponse(self.data), Response(b'')):
            with self.subTest(response=type(response).__name__):
                self.partial.write_bytes(prefix)
                with patch('sync.open_url', return_value=response), patch('sync.PACKAGE_ATTEMPTS', 1):
                    with self.assertRaisesRegex(sync.SyncError, 'partial retained'):
                        sync.download_package(self.asset, self.partial, self.digest)
                self.assertEqual(self.partial.read_bytes(), prefix)

    def test_ignored_range_can_replace_prefix_only_with_more_progress(self):
        self.partial.write_bytes(b'wrong old prefix')
        with patch('sync.open_url', return_value=Response(self.data)) as request:
            sync.download_package(self.asset, self.partial, self.digest)
        self.assertEqual(request.call_args.kwargs['headers']['Range'], 'bytes=16-')
        self.assertEqual(self.partial.read_bytes(), self.data)

    def test_incompatible_ranges_and_encoding_never_append(self):
        prefix = self.data[:5]
        cases = [
            self.ranged(4),
            self.ranged(5, **{'Content-Range': f'bytes 5-999/{len(self.data) + 1}'}),
            self.ranged(5, **{'Content-Range': 'bytes 5-1000/1000'}),
            self.ranged(5, **{'Content-Range': 'bytes 05-999/1000'}),
            self.ranged(5, **{'Content-Length': '1'}),
            self.ranged(5, **{'Content-Encoding': 'gzip'}),
            Response(self.data, {'Content-Range': 'bytes 0-999/1000'}),
            Response(self.data, status=203),
        ]
        for response in cases:
            with self.subTest(headers=response.headers, status=response.status):
                self.partial.write_bytes(prefix)
                with patch('sync.open_url', return_value=response), patch('sync.PACKAGE_ATTEMPTS', 1):
                    with self.assertRaises(sync.SyncError):
                        sync.download_package(self.asset, self.partial, self.digest)
                self.assertEqual(self.partial.read_bytes(), prefix)

    def test_oversized_response_does_not_succeed_with_valid_prefix_hash(self):
        for offset in (0, 5):
            with self.subTest(offset=offset):
                self.partial.write_bytes(self.data[:offset])
                response = Response(self.data[offset:] + b'extra', {
                    'Content-Range': f'bytes {offset}-999/1000',
                }, status=206)
                with patch('sync.open_url', return_value=response), patch('sync.PACKAGE_ATTEMPTS', 1):
                    with self.assertRaisesRegex(sync.SyncError, 'size mismatch'):
                        sync.download_package(self.asset, self.partial, self.digest)
                self.assertEqual(self.partial.read_bytes(), self.data[:offset])

    def test_416_retries_fresh_without_erasing_prefix(self):
        self.partial.write_bytes(self.data[:5])
        error = HTTPError(self.asset.api_url, 416, 'range not satisfiable', {}, BytesIO())
        with patch('sync.open_url', side_effect=[error, Response(self.data)]) as request:
            sync.download_package(self.asset, self.partial, self.digest)
        self.assertEqual(request.call_args_list[0].kwargs['headers']['Range'], 'bytes=5-')
        self.assertNotIn('Range', request.call_args_list[1].kwargs['headers'])
        self.assertEqual(self.partial.read_bytes(), self.data)

    def test_corrupt_completed_partial_and_bad_download_are_reset(self):
        self.partial.write_bytes(b'x' * len(self.data))
        with patch('sync.open_url', side_effect=[Response(b'y' * len(self.data)), Response(self.data)]) as request:
            sync.download_package(self.asset, self.partial, self.digest)
        self.assertEqual(len(request.call_args_list), 2)
        self.assertTrue(all('Range' not in call.kwargs['headers'] for call in request.call_args_list))
        self.assertEqual(self.partial.read_bytes(), self.data)

    def test_partial_identity_changes_with_tag_signed_hash_size_and_asset_id(self):
        changed = [
            sync.partial_path(self.cache, self.version, self.name, self.asset, '0' * 64),
            sync.partial_path(self.cache, sync.Version.parse('v0.2.0'), self.name, self.asset, self.digest),
            sync.partial_path(self.cache, self.version, self.name,
                              sync.Asset(self.asset.url, self.asset.api_url, len(self.data) + 1, self.digest), self.digest),
            sync.partial_path(self.cache, self.version, self.name,
                              sync.Asset(self.asset.url, f'{sync.ASSET_API}/124', len(self.data), self.digest), self.digest),
        ]
        self.assertNotIn(self.partial, changed)
        self.assertEqual(len(set(changed)), len(changed))

    def test_symlink_partial_cannot_overwrite_other_file(self):
        victim = self.cache / 'victim'
        victim.write_bytes(b'keep')
        self.partial.symlink_to(victim)
        with self.assertRaises(OSError):
            sync.download_package(self.asset, self.partial, self.digest)
        self.assertEqual(victim.read_bytes(), b'keep')

    def test_deadline_preserves_partial_without_another_request(self):
        self.partial.write_bytes(self.data[:5])
        with patch('sync.time.monotonic', side_effect=[0, sync.PACKAGE_DEADLINE + 1]), \
                patch('sync.open_url') as request:
            with self.assertRaisesRegex(sync.SyncError, 'partial retained'):
                sync.download_package(self.asset, self.partial, self.digest)
        request.assert_not_called()
        self.assertEqual(self.partial.read_bytes(), self.data[:5])

    def test_actual_http_resume_survives_new_timer_process(self):
        data = self.data
        ranges = []

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                value = self.headers.get('Range')
                ranges.append(value)
                self.send_response(206 if value else 200)
                start = int(value.removeprefix('bytes=').removesuffix('-')) if value else 0
                self.send_header('Content-Length', str(len(data) - start))
                if value:
                    self.send_header('Content-Range', f'bytes {start}-{len(data) - 1}/{len(data)}')
                self.end_headers()
                self.wfile.write(data[start:] if value else data[:5])
                self.wfile.flush()
                self.close_connection = True

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        driver = '''
import pathlib, sys
from urllib.request import Request, urlopen
import sync
sync.PACKAGE_ATTEMPTS = 1
sync.open_url = lambda url, **kw: urlopen(Request(sys.argv[1], headers=kw['headers']), timeout=2)
asset = sync.Asset('https://github.com/official', 'https://api.github.com/official', int(sys.argv[3]), sys.argv[4])
try:
    sync.download_package(asset, pathlib.Path(sys.argv[2]), sys.argv[4])
except sync.SyncError:
    sys.exit(3)
'''
        command = [sys.executable, '-B', '-c', driver, f'http://127.0.0.1:{server.server_port}/package',
                   str(self.partial), str(len(self.data)), self.digest]
        first = subprocess.run(command, cwd=Path(__file__).resolve().parent, capture_output=True, timeout=10)
        self.assertEqual(first.returncode, 3, first.stderr.decode())
        self.assertEqual(self.partial.read_bytes(), data[:5])
        second = subprocess.run(command, cwd=Path(__file__).resolve().parent, capture_output=True, timeout=10)
        self.assertEqual(second.returncode, 0, second.stderr.decode())
        self.assertEqual(ranges, [None, 'bytes=5-'])
        self.assertEqual(self.partial.read_bytes(), data)


if __name__ == '__main__':
    unittest.main()
