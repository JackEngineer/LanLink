#!/usr/bin/env python3
"""Publish verified LANLink release assets to the existing static website."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import sys
import tempfile
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


REPO = 'JackEngineer/LanLink'
API = f'https://api.github.com/repos/{REPO}/releases'
ASSET_API = f'{API}/assets'
SITE = 'https://lanlink.heyyami.com'
TAG = r'v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})(?:-preview\.(0|[1-9][0-9]{0,8}))?'
VERSION_HTML = re.compile(rf'<span class="release-version">({TAG})</span>')
HASH = r'[0-9a-fA-F]{64}'
MANIFEST = 'SHA256SUMS'
MANIFEST_LIMIT = 8192
ASSET_LIMIT = 256 * 1024 * 1024
PAGE_LIMIT = 4 * 1024 * 1024
ASSET_CDNS = {'release-assets.githubusercontent.com', 'objects.githubusercontent.com'}


class SyncError(Exception):
    pass


@dataclass(frozen=True)
class Version:
    tag: str
    base: str
    preview: bool
    order: tuple

    @classmethod
    def parse(cls, tag):
        match = re.fullmatch(TAG, tag) if isinstance(tag, str) else None
        if not match:
            return None
        major, minor, patch, preview = match.groups()
        return cls(tag, f'{major}.{minor}.{patch}', preview is not None,
                   (int(major), int(minor), int(patch), preview is None, int(preview or 0)))

    def packages(self):
        prefix = f'LANLink-{self.base}'
        return (
            f'{prefix}-aarch64-apple-darwin.pkg',
            f'{prefix}-aarch64-apple-darwin.zip',
            f'{prefix}-x86_64-apple-darwin.zip',
            f'{prefix}-x86_64-pc-windows-msvc.zip',
        )

    def assets(self):
        return (*self.packages(), MANIFEST)


class ReleaseRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        source = urlsplit(req.full_url)
        target = urlsplit(newurl)
        is_asset_api = req.full_url.startswith(f'{ASSET_API}/')
        if (not (source.netloc in {'github.com', *ASSET_CDNS} or is_asset_api)
                or target.scheme != 'https' or target.netloc not in ASSET_CDNS):
            raise SyncError('Unexpected redirect while reading release or website')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def open_url(url, **kwargs):
    headers = {'User-Agent': 'LANLink-release-sync', 'Accept-Encoding': 'identity'}
    headers.update(kwargs.pop('headers', {}))
    return build_opener(ReleaseRedirect).open(Request(url, headers=headers, **kwargs), timeout=45)


def read_response(response, limit):
    data = response.read(limit + 1)
    if len(data) > limit:
        raise SyncError('Response exceeded its size limit')
    return data


def published_releases(current):
    candidates = {}
    for page in range(1, 11):
        with open_url(f'{API}?per_page=100&page={page}', headers={'Accept': 'application/vnd.github+json'}) as response:
            releases = json.loads(read_response(response, PAGE_LIMIT))
            has_next = 'rel="next"' in response.headers.get('Link', '')
        if not isinstance(releases, list) or len(releases) > 100:
            raise SyncError('Unexpected GitHub Releases API response')
        for release in releases:
            if not isinstance(release, dict):
                raise SyncError('Invalid GitHub release metadata')
            version = Version.parse(release.get('tag_name'))
            if (not version or version.order < current.order or release.get('draft') is not False
                    or not release.get('published_at')):
                continue
            if release.get('prerelease') is not version.preview:
                raise SyncError(f'{version.tag} tag and GitHub prerelease flag disagree')
            if version in candidates and candidates[version] != release:
                raise SyncError(f'{version.tag} occurs more than once in the GitHub release listing')
            candidates[version] = release
        if not has_next:
            return sorted(candidates.items(), key=lambda item: item[0].order)
    raise SyncError('Release listing exceeds the ten-page scan limit')


@dataclass(frozen=True)
class Asset:
    url: str
    api_url: str
    size: int
    digest: str | None


def release_assets(version, release):
    if not isinstance(release.get('assets'), list):
        raise SyncError(f'{version.tag} has no asset list')
    expected = set(version.assets())
    result = {}
    for item in release['assets']:
        if not isinstance(item, dict):
            raise SyncError(f'{version.tag} has invalid asset metadata')
        name = item.get('name')
        if not isinstance(name, str) or name not in expected:
            continue
        url = f'https://github.com/{REPO}/releases/download/{version.tag}/{name}'
        if name in result or item.get('browser_download_url') != url or item.get('state') != 'uploaded':
            raise SyncError(f'{version.tag} has invalid metadata for {name}')
        asset_id = item.get('id')
        api_url = f'{ASSET_API}/{asset_id}'
        if type(asset_id) is not int or asset_id <= 0 or item.get('url') != api_url:
            raise SyncError(f'{version.tag} has an invalid asset ID or API URL for {name}')
        size = item.get('size')
        limit = MANIFEST_LIMIT if name == MANIFEST else ASSET_LIMIT
        if type(size) is not int or not 0 < size <= limit:
            raise SyncError(f'{version.tag} has an invalid size for {name}')
        digest = item.get('digest')
        if digest is not None and (not isinstance(digest, str) or not re.fullmatch(f'sha256:{HASH}', digest)):
            raise SyncError(f'{version.tag} has an invalid digest for {name}')
        result[name] = Asset(url, api_url, size, digest[7:].lower() if digest else None)
    missing = expected - result.keys()
    if missing:
        raise SyncError(f'{version.tag} is missing release assets: {", ".join(sorted(missing))}')
    return result


def download(asset, destination):
    try:
        response = open_url(asset.api_url, headers={'Accept': 'application/octet-stream'})
    except (URLError, TimeoutError, ConnectionError):
        response = open_url(asset.url)
    digest = hashlib.sha256()
    remaining = asset.size
    with response, destination.open('xb') as output:
        while remaining:
            chunk = response.read(min(remaining, 1024 * 1024))
            if not chunk:
                break
            output.write(chunk)
            digest.update(chunk)
            remaining -= len(chunk)
        if remaining or response.read(1):
            raise SyncError(f'Download size mismatch for {destination.name}')
    actual = digest.hexdigest()
    if asset.digest and actual != asset.digest:
        raise SyncError(f'GitHub SHA-256 mismatch for {destination.name}')
    return actual


def read_manifest(data, packages):
    result = {}
    for line in data.decode('ascii').splitlines():
        match = re.fullmatch(rf'({HASH}) [ *]([A-Za-z0-9._-]+)', line)
        if not match:
            raise SyncError('Malformed SHA256SUMS entry')
        digest, name = match.groups()
        if name not in packages or name in result:
            raise SyncError(f'Unexpected or duplicate SHA256SUMS entry: {name}')
        result[name] = digest.lower()
    if set(packages) != result.keys():
        raise SyncError('SHA256SUMS does not cover every required package')
    return result


def prepare_downloads(version, assets, directory):
    checksums = directory / MANIFEST
    download(assets[MANIFEST], checksums)
    expected = read_manifest(checksums.read_bytes(), version.packages())
    for name, digest in expected.items():
        if download(assets[name], directory / name) != digest:
            raise SyncError(f'SHA256SUMS mismatch for {name}')


def verify_release_files(version, assets, directory):
    manifest_path = directory / MANIFEST
    if manifest_path.stat().st_size != assets[MANIFEST].size:
        raise SyncError(f'{version.tag} has an unexpected SHA256SUMS size')
    manifest = manifest_path.read_bytes()
    if assets[MANIFEST].digest and hashlib.sha256(manifest).hexdigest() != assets[MANIFEST].digest:
        raise SyncError(f'{version.tag} has an unexpected SHA256SUMS digest')
    expected = read_manifest(manifest, version.packages())
    for name, digest in expected.items():
        path = directory / name
        asset = assets[name]
        if path.stat().st_size != asset.size:
            raise SyncError(f'Staged download size mismatch for {name}')
        with path.open('rb') as source:
            actual = hashlib.sha256()
            for chunk in iter(lambda: source.read(1024 * 1024), b''):
                actual.update(chunk)
        if actual.hexdigest() != digest or (asset.digest and actual.hexdigest() != asset.digest):
            raise SyncError(f'Staged download SHA-256 mismatch for {name}')


def site_links(version):
    return [f'{SITE}/downloads/{version.tag}/{name}' for name in version.assets()]


def verify_links(html, version):
    links = re.findall(r'href="(https://lanlink\.heyyami\.com/downloads/[^"<> ]+)"', html)
    if (sorted(links) != sorted(site_links(version))
            or re.search(r'href="https://github\.com/JackEngineer/LanLink/releases/', html, re.I)):
        raise SyncError(f'Website download links do not match {version.tag}')


def render(html, current, version):
    verify_links(html, current)
    for old, new in zip(site_links(current), site_links(version)):
        html = html.replace(f'href="{old}"', f'href="{new}"')
    html, replaced = VERSION_HTML.subn(f'<span class="release-version">{version.tag}</span>', html)
    if replaced != 1:
        raise SyncError('Website must have exactly one release version')

    preview_faq = re.compile(r'目前是开发预览，不是 v\d+\.\d+\.\d+ 正式完成版。欢迎在')
    html = preview_faq.sub('当前下载版本见上方。欢迎在', html)
    html = html.replace('下载预览版', '下载安装包')
    html = html.replace('安装前，请先了解预览版的边界。', '安装前，请先阅读安装指南。')
    if current.preview != version.preview:
        labels = {
            '现已提供开发预览。': '现已提供正式版本。',
            '开发预览': '正式发布',
            '适合愿意尝试并反馈的用户。': '当前提供的正式版本。',
        }
        for preview, stable in labels.items():
            old, new = (stable, preview) if version.preview else (preview, stable)
            html = html.replace(old, new)
    verify_links(html, version)
    return html


def active_site(root, releases):
    current = root / 'current'
    if not current.is_symlink():
        raise SyncError('Website current path must be a symbolic link')
    active = current.resolve(strict=True)
    if active.parent != releases or not active.is_dir():
        raise SyncError('Website current link must point to a child of releases')
    index = active / 'index.html'
    if index.is_symlink() or not index.is_file() or index.stat().st_size > PAGE_LIMIT:
        raise SyncError('Active website needs a regular index.html')
    html = index.read_bytes().decode('utf-8')
    versions = list(VERSION_HTML.finditer(html))
    if len(versions) != 1:
        raise SyncError('Active website must contain exactly one supported version')
    version = Version.parse(versions[0][1])
    verify_links(html, version)
    return active, html, version


def verify_downloads(site, version):
    directory = site / 'downloads' / version.tag
    if any(path.is_symlink() or not path.is_dir() for path in (directory.parent, directory)):
        raise SyncError(f'Active website has an invalid download directory for {version.tag}')
    for name in version.assets():
        path = directory / name
        if path.is_symlink() or not path.is_file() or not path.stat().st_size:
            raise SyncError(f'Website package missing or invalid: {version.tag}/{name}')
    return directory


def copy_site(active, stage, archive=False):
    for source in active.iterdir():
        destination = stage / source.name
        mode = source.lstat().st_mode
        if stat.S_ISDIR(mode):
            destination.mkdir(mode=0o755)
            copy_site(source, destination, archive=archive or source.name == 'downloads')
        elif stat.S_ISREG(mode):
            if archive:
                try:
                    os.link(source, destination, follow_symlinks=False)
                    if not source.samefile(destination):
                        raise SyncError(f'Archived download changed while staging: {source}')
                    continue
                except OSError as error:
                    if error.errno not in {errno.EXDEV, errno.EPERM, errno.EACCES, errno.EMLINK}:
                        raise
            shutil.copyfile(source, destination)
            if destination.stat().st_size != source.stat().st_size:
                raise SyncError(f'Archived file size changed while staging: {source}')
        else:
            raise SyncError(f'Active website contains an unsupported path: {source}')


def replace_current(root, target):
    with tempfile.TemporaryDirectory(prefix='.activate-', dir=root) as work:
        link = Path(work) / 'current'
        link.symlink_to(target)
        os.replace(link, root / 'current')


def verify_public(version, html, directory, assets):
    headers = {'Cache-Control': 'no-cache'}
    with open_url(f'{SITE}/', headers=headers) as response:
        if read_response(response, PAGE_LIMIT) != html.encode('utf-8'):
            raise SyncError('Public homepage differs from the activated page')
    with open_url(site_links(version)[-1], headers=headers) as response:
        if read_response(response, MANIFEST_LIMIT) != (directory / MANIFEST).read_bytes():
            raise SyncError('Public SHA256SUMS differs from the verified file')
    for name, url in zip(version.packages(), site_links(version)):
        with open_url(url, method='HEAD', headers=headers) as response:
            if response.headers.get('Content-Length') != str(assets[name].size):
                raise SyncError(f'Public package size mismatch for {name}')


@contextmanager
def release_lock(root):
    with (root / '.release-sync.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise SyncError('Another release sync is already running') from error
        yield


def sync(root, check=False):
    releases = root / 'releases'
    if releases.is_symlink() or not releases.is_dir():
        raise SyncError('Website needs a real releases directory')
    with release_lock(root):
        active, old_html, current = active_site(root, releases)
        directory = verify_downloads(active, current)
        releases_to_sync = published_releases(current)
        if releases_to_sync and releases_to_sync[0][0] == current:
            _, active_release = releases_to_sync.pop(0)
            if not releases_to_sync:
                assets = release_assets(current, active_release)
                if check:
                    verify_release_files(current, assets, directory)
                verify_public(current, old_html, directory, assets)
                print(f'{current.tag} is already active and publicly accessible')
                return
        if not releases_to_sync:
            print(f'{current.tag} is already current; no newer supported release')
            return
        assets_by_version = {version: release_assets(version, release) for version, release in releases_to_sync}
        newest = releases_to_sync[-1][0]
        html = render(old_html, current, newest)
        stage = Path(tempfile.mkdtemp(prefix=f'.sync-{newest.tag}-', dir=releases))
        try:
            copy_site(active, stage)
            for version, assets in assets_by_version.items():
                downloads = stage / 'downloads' / version.tag
                if not downloads.exists():
                    downloads.mkdir(parents=True)
                    prepare_downloads(version, assets, downloads)
                verify_downloads(stage, version)
                verify_release_files(version, assets, downloads)
            (stage / 'index.html').write_bytes(html.encode('utf-8'))
            verify_downloads(stage, current)
            if check:
                print(f'Verified {len(assets_by_version)} release(s) through {newest.tag}; {current.tag} is still active')
                return
            if (root / 'current').resolve(strict=True) != active:
                raise SyncError('Another deployment changed the active website during the sync')
            published = stage.with_name(stage.name.removeprefix('.sync-'))
            stage.chmod(0o755)
            stage.rename(published)
            try:
                replace_current(root, published)
            except OSError:
                if (root / 'current').resolve() != published:
                    shutil.rmtree(published)
                raise
            print(f'Activated {newest.tag}; previous website retained at {active}', flush=True)
            for version, assets in assets_by_version.items():
                verify_public(version, html, published / 'downloads' / version.tag, assets)
            print(f'Verified public homepage, SHA256SUMS and packages through {newest.tag}')
        finally:
            if stage.exists():
                shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path('/var/www/lanlink'))
    parser.add_argument('--check', action='store_true', help='Verify a new release without activating it')
    args = parser.parse_args()
    os.umask(0o022)
    try:
        sync(args.root.resolve(strict=True), check=args.check)
    except HTTPError as error:
        message = f'HTTP status {error.code}'
    except URLError as error:
        message = f'network error ({type(error.reason).__name__})'
    except (SyncError, OSError, ValueError) as error:
        message = str(error)
    else:
        return 0
    print(f'LANLink release sync failed: {message}', file=sys.stderr)
    return 1


if __name__ == '__main__':
    sys.exit(main())
