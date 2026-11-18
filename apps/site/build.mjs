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
      'release.json',
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
  const screenshots = {
    request: {
      title: 'Mac · 等待接收',
      width: 1016,
      height: 422,
      alt: 'Mac 验收时发出的文件显示等待接收',
    },
    receive: {
      title: 'Windows · 接收完成',
      width: 680,
      height: 340,
      alt: 'Windows 传输列表中已接收的文件显示下载按钮',
    },
    chat: {
      title: 'Mac · 双向聊天',
      width: 1320,
      height: 660,
      alt: 'Mac 的 LANLink 聊天界面显示双机验收消息',
    },
  };
  const renderScreenshot = ([id, { title, width, height, alt }]) => {
    const src = `./assets/demo-${id}.webp`;
    const shot = `<img src="${src}" width="${width}" height="${height}" alt="${escape(alt)}" />`;
    const frame = `<span class="screen-bar"><strong>${escape(title)}</strong><span>放大</span></span>${shot}`;
    return `<div class="screen">
      <button class="screen-open" type="button" popovertarget="zoom-${id}" aria-label="放大 ${escape(title)} 截图">${frame}</button>
      <a class="screen-fallback" href="${src}" aria-label="打开 ${escape(title)} 截图">${frame}</a>
      <section class="screen-lightbox" id="zoom-${id}" popover aria-labelledby="zoom-title-${id}">
        <div class="lightbox-bar">
          <h3 id="zoom-title-${id}">${escape(title)}</h3>
          <button type="button" popovertarget="zoom-${id}" popovertargetaction="hide" autofocus>关闭</button>
        </div>
        ${shot}
      </section>
    </div>`;
  };
  const screenshotSlots = Object.fromEntries(
    Object.entries(screenshots).map((entry) => [`${entry[0]}Screen`, renderScreenshot(entry)]),
  );
  const platformDetails = {
    'macos-apple-silicon': { icon: 'laptop', description: '适用于 Apple 芯片的 Mac（M 系列）' },
    'macos-intel': { icon: 'laptop', description: '适用于 Intel 芯片的 Mac' },
    'windows-x64': { icon: 'grid-2x2', description: '适用于 x64 架构的 Windows' },
  };
  const downloadCards = release.downloads
    .map((download) => {
      const details = Object.hasOwn(platformDetails, download.platform)
        ? platformDetails[download.platform]
        : undefined;
      const icon = details
        ? `<img src="./assets/${details.icon}.svg" alt="" width="40" height="40" />`
        : '';
      const description = details ? `<p>${escape(details.description)}</p>` : '';
      const links = download.artifacts
        .map(
          (
            artifact,
          ) => `<a class="artifact-link" href="${escape(artifact.url)}" download aria-label="下载 ${escape(download.label)} ${escape(artifact.format)}">
          <img src="./assets/download.svg" alt="" width="18" height="18" />
          ${escape(artifact.format)}</a>`,
        )
        .join('\n          ');
      return `<article class="download-card" data-platform="${escape(download.platform)}">
        <div class="platform">
          ${icon}
          <div><h3>${escape(download.label)}</h3>${description}</div>
        </div>
        <div class="artifact-links">${links}</div>
      </article>`;
    })
    .join('\n      ');
  let html = await readFile(resolve(source, 'index.html'), 'utf8');
  for (const [slot, value] of Object.entries({
    releaseTag: escape(release.tag),
    checksumsUrl: escape(release.checksumsUrl),
    downloadCards,
    ...screenshotSlots,
  })) {
    if (!html.includes(`{{${slot}}}`)) throw new Error(`Missing template slot: ${slot}`);
    html = html.replaceAll(`{{${slot}}}`, value);
  }
  if (/{{[^}]+}}/.test(html)) throw new Error('Unresolved template slot');
  await rm(output, { recursive: true, force: true });
  await mkdir(output, { recursive: true });
  await writeFile(resolve(output, 'index.html'), html);
  await writeFile(resolve(output, 'release.json'), JSON.stringify({ tag: release.tag }) + '\n');
  await cp(resolve(source, 'styles.css'), resolve(output, 'styles.css'));
  await cp(resolve(source, 'assets'), resolve(output, 'assets'), { recursive: true });
  await cp(
    resolve(root, 'apps/web/public/apple-touch-icon.png'),
    resolve(output, 'assets/lanlink.png'),
  );
  console.log(`Built dist/site for ${release.tag}`);
}
