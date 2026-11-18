import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { execFileSync } from 'node:child_process';

const root = fileURLToPath(new URL('../../', import.meta.url));
const source = resolve(root, 'apps/site');
const output = resolve(root, 'dist/site');
const args = process.argv.slice(2);
if (args.length > 1 || (args.length === 1 && args[0] !== '--package')) {
  throw new Error('Usage: node apps/site/build.mjs [--package]');
}
if (args[0] === '--package') {
  execFileSync(
    'tar',
    [
      '--no-xattrs',
      '-czf',
      resolve(root, 'dist/lanlink-website.tar.gz'),
      '-C',
      output,
      'index.html',
      'styles.css',
      'assets',
    ],
    { stdio: 'inherit', env: { ...process.env, COPYFILE_DISABLE: '1' } },
  );
  console.log('Packaged dist/lanlink-website.tar.gz (static files only)');
} else {
  const release = JSON.parse(await readFile(resolve(source, 'release.json'), 'utf8'));
  const escape = (value) =>
    String(value)
      .replaceAll('&', '&amp;')
      .replaceAll('"', '&quot;')
      .replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;');
  const downloadCards = release.downloads
    .map(
      (
        download,
        index,
      ) => `<article class="download-card" data-platform="${escape(download.platform)}">
        <div class="platform-kicker"><span>DESKTOP APP</span><span>0${index + 1}</span></div>
        <div class="platform-icon" aria-hidden="true"></div>
        <h3>${escape(download.label)}</h3>
        <div class="artifact-links">${download.artifacts.map((artifact) => `<a class="artifact-link" href="${escape(artifact.url)}" download aria-label="下载 ${escape(download.label)} ${escape(artifact.format)}">下载 ${escape(artifact.format)} <span aria-hidden="true">↓</span></a>`).join('')}</div>
      </article>`,
    )
    .join('\n      ');
  let html = await readFile(resolve(source, 'index.html'), 'utf8');
  for (const [slot, value] of Object.entries({
    releaseTag: escape(release.tag),
    checksumsUrl: escape(release.checksumsUrl),
    downloadCards,
  })) {
    if (!html.includes(`{{${slot}}}`)) throw new Error(`Missing template slot: ${slot}`);
    html = html.replaceAll(`{{${slot}}}`, value);
  }
  if (/{{[^}]+}}/.test(html)) throw new Error('Unresolved template slot');
  await rm(output, { recursive: true, force: true });
  await mkdir(output, { recursive: true });
  await writeFile(resolve(output, 'index.html'), html);
  await cp(resolve(source, 'styles.css'), resolve(output, 'styles.css'));
  await cp(resolve(source, 'assets'), resolve(output, 'assets'), { recursive: true });
  console.log(`Built dist/site for ${release.tag}`);
}
