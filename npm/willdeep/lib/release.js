'use strict';

const { createHash } = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const PLATFORMS = Object.freeze({
  'darwin-x64': 'willdeep-macos-universal.tar.gz',
  'darwin-arm64': 'willdeep-macos-universal.tar.gz',
  'linux-x64': 'willdeep-linux-amd64.tar.gz',
  'linux-arm64': 'willdeep-linux-arm64.tar.gz',
  'win32-x64': 'willdeep-windows-x64.zip',
});
const EXPECTED_ENTRIES = Object.freeze({
  '.tar.gz': 'willdeep',
  '.zip': 'willdeep.exe',
});
const MAX_ASSET_BYTES = 128 * 1024 * 1024;
const MAX_BINARY_BYTES = 128 * 1024 * 1024;
const RELEASE_HOSTS = new Set(['github.com', 'release-assets.githubusercontent.com']);

class InstallError extends Error {
  constructor(message) {
    super(message);
    this.name = 'InstallError';
  }
}

function resolveAsset(manifest, platform, arch) {
  if (!manifest || manifest.version == null || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(manifest.repository || '') || !/^v[0-9][A-Za-z0-9.-]*$/.test(manifest.tag || '') || !manifest.assets) {
    throw new InstallError('安装清单不完整。');
  }
  const name = PLATFORMS[`${platform}-${arch}`];
  if (!name) throw new InstallError(`暂不支持 ${platform}/${arch}。`);
  const asset = manifest.assets[name];
  if (!asset || !/^[a-f0-9]{64}$/.test(asset.sha256 || '')) {
    throw new InstallError(`版本 ${manifest.version} 还没有 ${name} 的校验值。`);
  }
  if (asset.name !== name) throw new InstallError(`安装清单里的资源名不一致：${name}。`);
  return { ...asset, name };
}

function assetUrl(manifest, assetName) {
  const encoded = assetName.split('/').map(encodeURIComponent).join('/');
  return `https://github.com/${manifest.repository}/releases/download/${encodeURIComponent(manifest.tag)}/${encoded}`;
}

async function downloadAsset(url, expected, fetchImpl, redirectCount = 0, trustedHosts = RELEASE_HOSTS) {
  if (redirectCount > 5) throw new InstallError('下载发行包时重定向次数过多。');
  const response = await fetchImpl(url, { redirect: 'manual' });
  if ([301, 302, 303, 307, 308].includes(response.status)) {
    const location = response.headers.get('location');
    if (!location) throw new InstallError('下载发行包时收到空的重定向。');
    const target = new URL(location, url);
    const trusted = target.protocol === 'https:' && target.port === '' && trustedHosts.has(target.hostname);
    const localTest = target.protocol === 'http:' && target.hostname === '127.0.0.1' && target.port !== '' && trustedHosts.has('127.0.0.1');
    if (!trusted && !localTest) throw new InstallError(`发行包重定向到不受信任的地址：${target.hostname}。`);
    return downloadAsset(target.href, expected, fetchImpl, redirectCount + 1, trustedHosts);
  }
  if (!response.ok) throw new InstallError(`下载发行包失败：HTTP ${response.status}。`);
  const declared = Number(response.headers.get('content-length'));
  if (Number.isFinite(declared) && declared > MAX_ASSET_BYTES) throw new InstallError('发行包超过 128 MiB 上限。');
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength > MAX_ASSET_BYTES) throw new InstallError('发行包超过 128 MiB 上限。');
  const digest = createHash('sha256').update(bytes).digest('hex');
  if (digest !== expected) throw new InstallError(`发行包 SHA256 不符：收到 ${digest}。`);
  return bytes;
}

function extractBinary(archivePath, assetName, destination) {
  const extension = Object.keys(EXPECTED_ENTRIES).find((item) => assetName.endsWith(item));
  const expected = EXPECTED_ENTRIES[extension];
  if (!expected) throw new InstallError(`不认识的发行包：${assetName}。`);
  const output = fs.mkdtempSync(path.join(os.tmpdir(), 'willdeep-npm-'));
  try {
    if (extension === '.tar.gz') {
      execFileSync('tar', ['-xzf', archivePath, '-C', output], { stdio: 'pipe' });
    } else if (process.platform === 'win32') {
      execFileSync('tar', ['-xf', archivePath, '-C', output], { stdio: 'pipe' });
    } else {
      execFileSync('unzip', ['-q', archivePath, '-d', output], { stdio: 'pipe' });
    }
    const extracted = fs.readdirSync(output);
    if (extracted.length !== 1 || extracted[0] !== expected) {
      throw new InstallError(`发行包内容异常：${extracted.join(', ') || '空包'}。`);
    }
    const source = path.join(output, expected);
    const metadata = fs.lstatSync(source);
    if (!metadata.isFile()) throw new InstallError('发行包里的程序必须是普通文件。');
    if (metadata.size > MAX_BINARY_BYTES) throw new InstallError('发行包里的程序超过 128 MiB 上限。');
    if (extension !== '.zip' && (metadata.mode & 0o111) === 0) {
      throw new InstallError('发行包里的程序没有可执行权限。');
    }
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    const temporary = path.join(path.dirname(destination), `.${path.basename(destination)}.${process.pid}.tmp`);
    fs.copyFileSync(source, temporary);
    fs.chmodSync(temporary, 0o755);
    fs.renameSync(temporary, destination);
  } finally {
    fs.rmSync(output, { recursive: true, force: true });
  }
}

async function installWilldeep(options) {
  const asset = resolveAsset(options.manifest, options.platform, options.arch);
  if (fs.existsSync(options.destination)) {
    const marker = path.join(options.packageRoot, '.installed-asset.json');
    if (fs.existsSync(marker)) {
      const installed = JSON.parse(fs.readFileSync(marker, 'utf8'));
      if (installed.version === options.manifest.version && installed.sha256 === asset.sha256) {
        return { reused: true, asset };
      }
    }
  }
  const archive = path.join(os.tmpdir(), `willdeep-${process.pid}-${Date.now()}-${asset.name}`);
  try {
    const bytes = await downloadAsset(
      assetUrl(options.manifest, asset.name),
      asset.sha256,
      options.fetchImpl,
      0,
      options.trustedHosts || RELEASE_HOSTS,
    );
    fs.writeFileSync(archive, bytes, { flag: 'wx', mode: 0o600 });
    extractBinary(archive, asset.name, options.destination);
    fs.writeFileSync(path.join(options.packageRoot, '.installed-asset.json'), `${JSON.stringify({
      version: options.manifest.version,
      name: asset.name,
      sha256: asset.sha256,
    })}\n`, { mode: 0o600 });
  } finally {
    fs.rmSync(archive, { force: true });
  }
  return { reused: false, asset };
}

module.exports = {
  InstallError,
  resolveAsset,
  assetUrl,
  downloadAsset,
  extractBinary,
  installWilldeep,
};
