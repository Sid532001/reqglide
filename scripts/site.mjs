import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { cp, mkdir, readFile, realpath, rm, stat, lstat, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = path.join(root, 'apps/site');
const output = path.join(source, 'dist');
const config = JSON.parse(await readFile(path.join(source, 'site.config.json'), 'utf8'));
const desktop = JSON.parse(await readFile(path.join(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'));
if (!/^\d+\.\d+\.\d+$/.test(config.releaseVersion) || config.releaseVersion !== desktop.version) {
  throw new Error('Website and desktop release versions must match.');
}
const repository = new URL(config.repositoryUrl);
if (repository.protocol !== 'https:' || repository.hostname !== 'github.com' || repository.username || repository.password || !/^\/[\w.-]+\/[\w.-]+\/?$/.test(repository.pathname)) {
  throw new Error('Configure a public HTTPS GitHub repository URL.');
}
const origin = new URL(config.siteUrl);
if (origin.protocol !== 'https:' || origin.username || origin.password || origin.search || origin.hash) throw new Error('Configure an HTTPS website URL.');
const releaseDirectory = path.join(root, '.release-artifacts/windows', config.releaseVersion);
const committedRelease = path.join(source, 'releases', `${config.releaseVersion}.json`);
const releaseBase = `${config.repositoryUrl.replace(/\/$/, '')}/releases/download/v${config.releaseVersion}/`;

function validateRelease(manifest) {
  if (manifest.product !== 'Reqglide' || manifest.version !== config.releaseVersion || manifest.platform !== 'windows-x64' || !Array.isArray(manifest.assets) || manifest.assets.length !== 2) {
    throw new Error('Release metadata must describe both Reqglide Windows installers.');
  }
  const kinds = new Set();
  for (const asset of manifest.assets) {
    const extension = asset.kind === 'nsis' ? '.exe' : asset.kind === 'msi' ? '.msi' : null;
    if (!extension || kinds.has(asset.kind) || asset.file !== path.basename(asset.file) || asset.file.includes('\\') || !asset.file.startsWith(`reqglide_${config.releaseVersion}_windows-x64`) || !asset.file.endsWith(extension) || !/^[a-f0-9]{64}$/.test(asset.sha256) || !Number.isSafeInteger(asset.bytes) || asset.bytes < 1 || asset.bytes > 1024 * 1024 * 1024) {
      throw new Error('Invalid or unsafe installer metadata.');
    }
    kinds.add(asset.kind);
  }
  return {
    schemaVersion: 1,
    product: 'Reqglide', version: manifest.version, platform: manifest.platform,
    channel: 'preview',
    assets: manifest.assets.map(({ file, kind, bytes, sha256, signatureStatus }) => ({ file, kind, bytes, sha256, signatureStatus }))
  };
}

async function localRelease() {
  const manifest = validateRelease(JSON.parse(await readFile(path.join(releaseDirectory, 'manifest.json'), 'utf8')));
  const base = await realpath(releaseDirectory);
  for (const asset of manifest.assets) {
    const file = path.join(releaseDirectory, asset.file);
    const resolved = await realpath(file);
    if (path.dirname(resolved) !== base) throw new Error('Installer path escapes the release directory.');
    const details = await stat(file);
    if (!details.isFile() || details.size !== asset.bytes) throw new Error(`Unexpected installer size: ${asset.file}`);
    const hash = createHash('sha256').update(await readFile(file)).digest('hex');
    if (hash !== asset.sha256) throw new Error(`Installer checksum mismatch: ${asset.file}`);
  }
  return manifest;
}

async function build() {
  const remote = process.argv.includes('--remote-downloads');
  let manifest;
  let includeDownloads = !remote;
  if (!remote) {
    try { manifest = await localRelease(); }
    catch (error) { if (error.code !== 'ENOENT') throw error; includeDownloads = false; }
  }
  if (!manifest) manifest = validateRelease(JSON.parse(await readFile(committedRelease, 'utf8')));

  // Only this generated directory may be replaced. Refuse symlinks rather than
  // following a dist path that could point to user files outside this project.
  if (path.relative(source, output) !== 'dist') throw new Error('Unsafe website output directory.');
  try {
    const details = await lstat(output);
    if (details.isSymbolicLink() || await realpath(output) !== output) throw new Error('Refusing a redirected website output directory.');
    await rm(output, { recursive: true });
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
  await mkdir(output, { recursive: true });
  for (const name of ['index.html', 'styles.css', 'site.js', 'assets', 'docs']) {
    await cp(path.join(source, name), path.join(output, name), { recursive: true, dereference: false });
  }
  await cp(path.join(root, 'LICENSE'), path.join(output, 'LICENSE.txt'));
  const release = {
    ...manifest,
    assets: manifest.assets.map(asset => ({ ...asset, url: includeDownloads ? `downloads/${asset.file}` : `${releaseBase}${asset.file}` })),
    checksumsUrl: includeDownloads ? 'downloads/SHA256SUMS.txt' : `${releaseBase}SHA256SUMS.txt`
  };
  await writeFile(path.join(output, 'site-config.js'), `window.REQGLIDE_SITE = ${JSON.stringify({ repositoryUrl: config.repositoryUrl, release }, null, 2).replaceAll('<', '\\u003c')};\n`);
  if (includeDownloads) {
    const downloads = path.join(output, 'downloads');
    await mkdir(downloads);
    for (const asset of manifest.assets) await cp(path.join(releaseDirectory, asset.file), path.join(downloads, asset.file));
    await writeFile(path.join(downloads, 'SHA256SUMS.txt'), manifest.assets.map(asset => `${asset.sha256}  ${asset.file}`).join('\n') + '\n');
    await writeFile(path.join(downloads, 'release.json'), JSON.stringify(manifest, null, 2) + '\n');
  }
  await writeFile(path.join(output, 'robots.txt'), `User-agent: *\nAllow: /\nSitemap: ${new URL('sitemap.xml', origin).href}\n`);
  await writeFile(path.join(output, 'sitemap.xml'), `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>${origin.href}</loc></url><url><loc>${new URL('docs/', origin).href}</loc></url></urlset>\n`);
  console.log(`Built Reqglide website: ${output}`);
  console.log(includeDownloads ? 'Verified Windows preview downloads included.' : 'Downloads link to the versioned GitHub preview release.');
}

const command = process.argv[2] || 'build';
if (command === 'capture-release') {
  const manifest = await localRelease();
  await writeFile(committedRelease, JSON.stringify(manifest, null, 2) + '\n');
  console.log(`Captured verified release metadata: ${path.relative(root, committedRelease)}`);
} else if (command === 'build' || command === 'serve') {
  await build();
  if (command === 'serve') {
    const mime = { '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.svg': 'image/svg+xml', '.png': 'image/png', '.txt': 'text/plain; charset=utf-8', '.xml': 'application/xml', '.json': 'application/json', '.exe': 'application/octet-stream', '.msi': 'application/octet-stream' };
    const server = createServer(async (request, response) => {
      if (!['GET', 'HEAD'].includes(request.method)) { response.writeHead(405, { Allow: 'GET, HEAD' }).end(); return; }
      try {
        const pathname = decodeURIComponent(new URL(request.url, 'http://127.0.0.1').pathname);
        const selected = path.resolve(output, '.' + (pathname.endsWith('/') ? pathname + 'index.html' : pathname));
        if (!selected.startsWith(output + path.sep)) { response.writeHead(403).end('Forbidden'); return; }
        const resolved = await realpath(selected);
        if (!resolved.startsWith(output + path.sep) || !(await stat(resolved)).isFile()) { response.writeHead(404).end('Not found'); return; }
        const extension = path.extname(selected).toLowerCase();
        response.setHeader('Content-Type', mime[extension] || 'application/octet-stream');
        response.setHeader('Content-Length', (await stat(resolved)).size);
        response.setHeader('X-Content-Type-Options', 'nosniff');
        response.setHeader('Referrer-Policy', 'strict-origin-when-cross-origin');
        response.setHeader('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'");
        if (['.exe', '.msi'].includes(extension)) response.setHeader('Content-Disposition', `attachment; filename="${path.basename(selected)}"`);
        response.writeHead(200);
        if (request.method === 'HEAD') response.end();
        else createReadStream(resolved).on('error', () => response.destroy()).pipe(response);
      } catch (error) { response.writeHead(error instanceof URIError ? 400 : 404).end('Not found'); }
    });
    server.on('error', error => { console.error(`Website server: ${error.message}`); process.exitCode = 1; });
    server.listen(4173, '127.0.0.1', () => console.log('Reqglide landing page: http://127.0.0.1:4173'));
  }
} else {
  throw new Error('Usage: node scripts/site.mjs [build|serve|capture-release] [--remote-downloads]');
}
