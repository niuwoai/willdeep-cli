'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { installWilldeep } = require('./release');

async function main() {
  const packageRoot = path.resolve(__dirname, '..');
  const manifest = JSON.parse(fs.readFileSync(path.join(packageRoot, 'install-manifest.json'), 'utf8'));
  const destination = path.join(packageRoot, 'bin', process.platform === 'win32' ? 'willdeep.exe' : 'willdeep');
  const installed = await installWilldeep({
    manifest,
    platform: process.platform,
    arch: process.arch,
    destination,
    packageRoot,
    fetchImpl: global.fetch,
  });
  if (!installed.reused) {
    process.stdout.write(`已安装 WillDeep ${manifest.version}（${installed.asset.name}）。\n`);
  }
}

if (require.main === module) {
  main().catch((error) => {
    process.stderr.write(`${error.stack}\n`);
    process.exitCode = 1;
  });
}

module.exports = { main };
