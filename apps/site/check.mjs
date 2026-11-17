import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { resolve, relative, extname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const output = resolve(root, 'dist/site');
const release = JSON.parse(await readFile(resolve(root, 'apps/site/release.json'), 'utf8'));
const html = await readFile(resolve(output, 'index.html'), 'utf8');
const readme = await readFile(resolve(root, 'README.md'), 'utf8');
const install = await readFile(resolve(root, 'docs/install.md'), 'utf8');
const releaseBase = 'https://github.com/JackEngineer/LanLink/releases/';
assert.match(
  release.tag,
  /^v\d+\.\d+\.\d+-preview\.\d+$/,
  'Use a named development preview release',
);
assert.equal(release.releaseUrl, `${releaseBase}tag/${release.tag}`);
assert.equal(release.checksumsUrl, `${releaseBase}download/${release.tag}/SHA256SUMS`);
assert.ok(
  Array.isArray(release.downloads) && release.downloads.length > 0,
  'Release must contain platforms',
);
const platforms = new Set();
const artifactUrls = [];
for (const download of release.downloads) {
  assert.match(download.platform, /^[a-z0-9-]+$/);
  assert.ok(!platforms.has(download.platform), `Duplicate platform: ${download.platform}`);
  platforms.add(download.platform);
  assert.ok(typeof download.label === 'string' && download.label.trim().length > 0);
  assert.ok(Array.isArray(download.artifacts) && download.artifacts.length > 0);
  for (const artifact of download.artifacts) {
    assert.ok(
      ['PKG', 'ZIP'].includes(artifact.format),
      `Unexpected artifact format: ${artifact.format}`,
    );
    assert.ok(
      artifact.url.startsWith(`${releaseBase}download/${release.tag}/`),
      `Artifact outside release: ${artifact.url}`,
    );
    assert.ok(
      new URL(artifact.url).pathname.endsWith(`.${artifact.format.toLowerCase()}`),
      'Format must match filename',
    );
    artifactUrls.push(artifact.url);
  }
}
assert.equal(new Set(artifactUrls).size, artifactUrls.length, 'Duplicate download artifact');
const releaseAssets = (text) =>
  new Set(
    text.match(/https:\/\/github\.com\/JackEngineer\/LanLink\/releases\/download\/[^\s)"<>]+/g) ??
      [],
  );
const expected = new Set([...artifactUrls, release.checksumsUrl]);
assert.deepEqual(
  releaseAssets(html),
  expected,
  'Built download links differ from release manifest',
);
assert.deepEqual(
  releaseAssets(readme),
  expected,
  'README download assets differ from release manifest',
);
assert.deepEqual(
  releaseAssets(install),
  expected,
  'Install guide download assets differ from release manifest',
);
assert.ok(
  html.includes(release.tag) && html.includes(release.releaseUrl),
  'Built page must show the current release',
);
assert.equal((html.match(/class="download-card"/g) ?? []).length, release.downloads.length);
assert.ok(!/{{[^}]+}}/.test(html), 'Unresolved build slots');
assert.ok(
  !/<script\b|\son[a-z]+\s*=|<iframe\b|<object\b|<embed\b|<form\b/i.test(html),
  'Site must not include executable scripts, embeds, or forms',
);
assert.ok(
  !/localhost|127\.0\.0\.1|\/api\//i.test(html),
  'Website must not call or link a local Agent',
);
const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]);
assert.equal(ids.length, new Set(ids).size, 'Duplicate anchor ID');
const localResources = new Set();
const officialLinks = new Set(['https://beian.miit.gov.cn/']);
for (const match of html.matchAll(/<(\w+)\b[^>]*?\b(href|src)="([^"]+)"[^>]*>/g)) {
  const [, tag, attribute, url] = match;
  if (url.startsWith('#')) {
    assert.ok(ids.includes(url.slice(1)), `Broken anchor: ${url}`);
  } else if (url.startsWith('https://')) {
    assert.equal(tag, 'a', `External runtime resource: ${url}`);
    const destination = new URL(url);
    assert.ok(
      (destination.origin === 'https://github.com' &&
        /^\/JackEngineer\/LanLink(?:\/|$)/.test(destination.pathname)) ||
        officialLinks.has(url),
      `Unexpected external destination: ${url}`,
    );
  } else {
    assert.ok(url.startsWith('./'), `Local resources must use relative paths: ${url}`);
    const path = resolve(output, url);
    assert.ok(!relative(output, path).startsWith('..'), `Resource escapes site: ${url}`);
    assert.ok((await stat(path)).isFile(), `Missing resource: ${url}`);
    if (attribute === 'src' || tag === 'link') localResources.add(path);
  }
}
async function filesIn(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    assert.ok(!entry.isSymbolicLink(), `Symlink in static output: ${path}`);
    if (entry.isDirectory()) files.push(...(await filesIn(path)));
    else files.push(path);
  }
  return files;
}
const files = await filesIn(output);
for (const path of files) {
  assert.ok(['.html', '.css', '.svg'].includes(extname(path)), `Unexpected static output: ${path}`);
  if (path.endsWith('.html')) continue;
  assert.ok(localResources.has(path), `Unreferenced output resource: ${path}`);
  const content = await readFile(path, 'utf8');
  assert.ok(
    !/@import\b|url\s*\(|<script\b|\son[a-z]+\s*=|<foreignObject\b/i.test(content),
    `Unexpected runtime dependency in ${path}`,
  );
}
console.log(
  `Site checks passed: ${files.length} static files, ${platforms.size} platforms, ${artifactUrls.length} download artifacts; README and install guide agree.`,
);
