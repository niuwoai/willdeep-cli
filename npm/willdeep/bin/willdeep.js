#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const binary = path.join(__dirname, process.platform === 'win32' ? 'willdeep.exe' : 'willdeep');
if (!fs.existsSync(binary)) {
  process.stderr.write('willdeep 还没有安装完成。请重新执行 npm install willdeep。\n');
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });
child.on('error', (error) => {
  process.stderr.write(`无法启动 willdeep：${error.message}\n`);
  process.exit(1);
});
child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code == null ? 1 : code);
});
