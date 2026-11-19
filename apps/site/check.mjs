import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import { readFile, readdir, stat } from 'node:fs/promises';
import { resolve, relative, extname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const output = resolve(root, 'dist/site');
const release = JSON.parse(await readFile(resolve(root, 'apps/site/release.json'), 'utf8'));
const html = await readFile(resolve(output, 'index.html'), 'utf8');
const script = await readFile(resolve(root, 'apps/site/install-copy.js'), 'utf8');
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)];
assert.equal(scripts.length, 1, 'Only the clipboard script is permitted');
assert.equal(scripts[0][1], script, 'Clipboard script must match its reviewed source');
const hash = createHash('sha256').update(script).digest('base64');
const nginx = await readFile(resolve(root, 'deploy/nginx/lanlink.conf'), 'utf8');
assert.ok(
  nginx.includes(`script-src 'sha256-${hash}';`),
  'Nginx CSP must allow only this script hash',
);
assert.doesNotMatch(nginx, /script-src[^;]*(?:'self'|'unsafe-inline'|'unsafe-eval')/);
const promptText = await readFile(resolve(root, 'apps/site/install-prompt.txt'), 'utf8');
assert.equal(await readFile(resolve(output, 'install.txt'), 'utf8'), promptText);
const promptMatch = html.match(/<div class="install-steps">([\s\S]*?)<\/div>/);
const command = html.match(
  /<textarea id="install-prompt" readonly rows="2">([^<]+)<\/textarea>/,
)?.[1];
assert.ok(
  command && !command.includes('\n') && command.includes('https://lanlink.heyyami.com/install.txt'),
);
assert.ok(promptMatch, 'Full installation instructions must be available in the page');
const decodedPrompt = promptMatch[1]
  .replaceAll('&lt;', '<')
  .replaceAll('&gt;', '>')
  .replaceAll('&quot;', '"')
  .replaceAll('&amp;', '&');
assert.equal(decodedPrompt, promptText, 'Displayed steps must match the public instruction file');
assert.match(html, /id="copy-install" type="button" hidden/);
for (const clipboard of ['success', 'denied', 'unavailable']) {
  let click;
  let copied;
  const button = {
    hidden: true,
    addEventListener: (event, listener) => {
      assert.equal(event, 'click');
      click = listener;
    },
  };
  const prompt = {
    value: command,
    focus() {
      this.focused = true;
    },
    select() {
      this.selected = true;
    },
  };
  const status = {};
  const elements = {
    'copy-install': button,
    'install-prompt': prompt,
    'copy-install-status': status,
  };
  runInNewContext(script, {
    document: { getElementById: (id) => elements[id] },
    navigator:
      clipboard === 'unavailable'
        ? {}
        : {
            clipboard: {
              writeText: async (value) => {
                if (clipboard === 'denied') throw new Error('Permission denied');
                copied = value;
              },
            },
          },
  });
  assert.equal(button.hidden, false);
  await click();
  assert.equal(button.disabled, false);
  if (clipboard === 'success') {
    assert.equal(copied, command);
    assert.match(status.textContent, /已复制/);
  } else {
    assert.equal(prompt.focused, true);
    assert.equal(prompt.selected, true);
    assert.match(status.textContent, /手动复制/);
  }
}
const releaseIndex = resolve(output, 'release.json');
assert.deepEqual(JSON.parse(await readFile(releaseIndex, 'utf8')), { tag: release.tag });
const readme = await readFile(resolve(root, 'README.md'), 'utf8');
const install = await readFile(resolve(root, 'docs/install.md'), 'utf8');
const downloadBase = `https://lanlink.heyyami.com/downloads/${release.tag}/`;
assert.match(
  release.tag,
  /^v\d+\.\d+\.\d+(?:-preview\.\d+)?$/,
  'Use a stable or named preview release tag',
);
assert.equal(release.checksumsUrl, `${downloadBase}SHA256SUMS`);
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
    assert.ok(artifact.url.startsWith(downloadBase), `Artifact outside release: ${artifact.url}`);
    assert.ok(
      new RegExp(`^[A-Za-z0-9_-][A-Za-z0-9._-]*[.]${artifact.format.toLowerCase()}$`).test(
        artifact.url.slice(downloadBase.length),
      ),
      'Format must match filename',
    );
    artifactUrls.push(artifact.url);
  }
}
assert.equal(new Set(artifactUrls).size, artifactUrls.length, 'Duplicate download artifact');
const releaseAssets = (text) =>
  new Set(text.match(/https:\/\/lanlink\.heyyami\.com\/downloads\/[^\s)"<>]+/g) ?? []);
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
const releaseStatus = release.tag.includes('-preview.') ? '开发预览' : '正式发布';
assert.ok(
  html.includes(`${releaseStatus} <span class="release-version">${release.tag}</span>`),
  'Built page must show the current release and channel',
);
for (const body of [html, readme, install]) {
  assert.doesNotMatch(body, /https:\/\/github\.com\/JackEngineer\/LanLink\/releases\//i);
}
assert.equal((html.match(/class="download-card"/g) ?? []).length, release.downloads.length);
assert.ok(!/{{[^}]+}}/.test(html), 'Unresolved build slots');
assert.ok(
  !/<script\b|\son[a-z]+\s*=|<iframe\b|<object\b|<embed\b|<form\b/i.test(
    html.replace(scripts[0][0], ''),
  ),
  'Site must not include executable scripts, embeds, or forms',
);
assert.ok(
  !/localhost|127\.0\.0\.1|\/api\//i.test(html),
  'Website must not call or link a local Agent',
);
const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]);
assert.equal(ids.length, new Set(ids).size, 'Duplicate anchor ID');
for (const [, attribute, targets] of html.matchAll(
  /\b(for|popovertarget|aria-controls|aria-labelledby)="([^"]+)"/g,
)) {
  for (const target of targets.split(/\s+/)) {
    assert.ok(ids.includes(target), `Broken ${attribute} target: ${target}`);
  }
}
const localResources = new Set();
const officialLinks = new Set(['https://beian.miit.gov.cn/']);
for (const match of html.matchAll(/<(\w+)\b[^>]*?\b(href|src)="([^"]+)"[^>]*>/g)) {
  const [, tag, attribute, url] = match;
  if (expected.has(url)) {
    assert.equal(tag, 'a', `Download must be a link: ${url}`);
    assert.match(match[0], /\sdownload(?:[\s>])/);
  } else if (url.startsWith('#')) {
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
  if (path === releaseIndex || path === resolve(output, 'install.txt')) continue;
  const extension = extname(path);
  assert.ok(
    ['.html', '.css', '.svg', '.png', '.webp'].includes(extension),
    `Unexpected static output: ${path}`,
  );
  if (extension === '.html') continue;
  assert.ok(localResources.has(path), `Unreferenced output resource: ${path}`);
  if (extension === '.png' || extension === '.webp') {
    const image = await readFile(path);
    if (extension === '.png') {
      assert.equal(
        image.subarray(0, 8).toString('hex'),
        '89504e470d0a1a0a',
        `Invalid PNG: ${path}`,
      );
    } else {
      assert.equal(image.toString('ascii', 0, 4), 'RIFF', `Invalid WebP: ${path}`);
      assert.equal(image.toString('ascii', 8, 12), 'WEBP', `Invalid WebP: ${path}`);
    }
    continue;
  }
  const content = await readFile(path, 'utf8');
  assert.ok(
    !/@import\b|url\s*\(|<script\b|\son[a-z]+\s*=|<foreignObject\b/i.test(content),
    `Unexpected runtime dependency in ${path}`,
  );
}
assert.deepEqual(
  await readFile(resolve(output, 'assets/lanlink.png')),
  await readFile(resolve(root, 'apps/web/public/apple-touch-icon.png')),
  'Website must use the approved application logo',
);
console.log(
  `Site checks passed: ${files.length} static files, ${platforms.size} platforms, ${artifactUrls.length} download artifacts; README and install guide agree.`,
);
