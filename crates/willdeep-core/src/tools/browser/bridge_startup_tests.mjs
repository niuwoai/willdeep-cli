import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { EventEmitter, once } from 'node:events';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { isAbsolute, join } from 'node:path';
import { PassThrough } from 'node:stream';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';

const source = await readFile(new URL('./bridge.mjs', import.meta.url), 'utf8');
const boundary = source.indexOf("process.on('SIGTERM'");
assert.ok(boundary > 0, 'bridge cleanup/protocol boundary must exist');
// Exercise the actual startup and CDP code with deterministic OS/clock adapters.
// No Chrome, HTTP/WebSocket server, network access or wall-clock sleeps.
const startupSource = source.slice(0, boundary).replace(/^import .*;\n/gmu, '');

function fixture(options = {}) {
  let now = 0, removed = false;
  const timers = [], pauses = [], launches = [], calls = [], sockets = [], kills = [];
  const child = Object.assign(new EventEmitter(), {
    stderr: new PassThrough(), exitCode: null, signalCode: null,
    kill(signal) {
      kills.push(signal);
      queueMicrotask(() => {
        if (this.exitCode === null && this.signalCode === null) {
          this.signalCode = signal; this.emit('exit', null, signal);
        }
      });
    },
  });
  class Socket extends EventTarget {
    constructor(url) {
      super(); this.url = url; this.closed = false; sockets.push(this);
      if (!options.hangSocket) queueMicrotask(() => this.dispatchEvent(new Event('open')));
    }
    send(raw) {
      const message = JSON.parse(raw); calls.push(message);
      options.onSend?.(message, child);
      if (message.method === options.hangMethod) return;
      queueMicrotask(() => {
        let result = {};
        if (message.method === 'Browser.getVersion') result = { product: 'Chrome/fixture' };
        if (message.method === 'Target.getTargets') result = { targetInfos: [] };
        if (message.method === 'Target.createBrowserContext') result = { browserContextId: 'context' };
        if (message.method === 'Page.getFrameTree') result = { frameTree: { frame: { id: 'frame', url: 'about:blank' } } };
        if (message.method === 'Target.createTarget') {
          result = { targetId: 'page' };
          this.message({ method: 'Target.attachedToTarget', params: {
            sessionId: 'session', targetInfo: { targetId: 'page', type: 'page' },
          } });
        }
        this.message({ id: message.id, result });
      });
    }
    message(message) { this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(message) })); }
    close() { this.closed = true; }
  }
  const bridge = runInNewContext(`${startupSource}\n({ initialize, action, cleanup, diagnostics: () => chromeStderr.summary() });`, {
    Buffer, URL, WebSocket: Socket,
    process: { versions: { node: '24.0.0' }, env: { HOME: '/isolated' } },
    performance: { now: () => now }, isAbsolute, join,
    setTimeout(callback, ms) {
      timers.push(ms);
      return setTimeout(() => { now += ms; callback(); }, 0);
    },
    clearTimeout,
    async delay(ms) { pauses.push(ms); now += ms; },
    async stat() { return { isFile: () => true }; },
    async mkdir() {},
    async rm() { removed = true; },
    async readFile(path) {
      assert.equal(path, join('/isolated/profile', 'DevToolsActivePort'));
      options.onRead?.({ child, now });
      if (now < (options.portAt ?? 0)) throw Object.assign(new Error('not ready'), { code: 'ENOENT' });
      return '9222\n/devtools/browser/fixture\n';
    },
    spawn(path, flags, settings) {
      launches.push({ path, flags, settings });
      queueMicrotask(() => options.onSpawn?.(child));
      return child;
    },
  });
  return {
    ...bridge, child, timers, pauses, launches, calls, sockets, kills,
    elapsed: () => now, advance: ms => { now += ms; }, removed: () => removed,
    init: (budget = 59_000) => bridge.initialize({ id: 1, action: 'init',
      chrome_path: '/fixture/chrome', profile: '/isolated/profile',
      allowed_origins: ['https://example.test'], startup_timeout_ms: budget }),
  };
}

const privateLog = 'HOME=/private/fixture Authorization: Bearer test-credential https://user:password@remote.example.test/private?token=test-credential\n';
function assertRedacted(message) {
  for (const raw of ['HOME=', '/private/fixture', 'Authorization:', 'Bearer', 'test-credential', 'remote.example.test', 'user:password']) {
    assert.ok(!message.includes(raw), `diagnostic leaked ${raw}`);
  }
}

test('configured startup can exceed the former five-second limit without replay', async () => {
  const f = fixture({ portAt: 6250 });
  try {
    const result = await f.init();
    assert.equal(result.profile_isolated, true);
    assert.equal(result.chrome_version, 'Chrome/fixture');
    assert.equal(f.elapsed(), 6250);
    assert.equal(f.launches.length, 1);
    assert.equal(f.sockets.length, 1);
    assert.equal(f.sockets[0].url, 'ws://127.0.0.1:9222/devtools/browser/fixture');
    assert.equal(f.calls.filter(call => call.method === 'Target.createTarget').length, 1);
  } finally { await f.cleanup(); }
});

test('startup timeout uses the monotonic budget and caps the final polling pause', async () => {
  const f = fixture({ portAt: Infinity, onSpawn: child => child.stderr.write(privateLog) });
  try {
    await assert.rejects(f.init(6013), reason => {
      assert.equal(reason.code, 'UNKNOWN');
      assert.match(reason.message, /CDP startup timed out/u);
      assert.match(reason.message, /exit_code=none/u);
      assertRedacted(reason.message);
      return true;
    });
    assert.equal(f.elapsed(), 6013);
    assert.equal(f.pauses.at(-1), 13);
    assert.equal(f.launches.length, 1);
    assert.equal(f.sockets.length, 0);
  } finally { await f.cleanup(); }
  assert.ok(f.child.stderr.destroyed);
  assert.ok(f.removed());
});

test('early Chrome exit is distinct from a deadline and only emits bounded diagnostic labels', async () => {
  const f = fixture({ portAt: Infinity,
    onSpawn(child) {
      child.stderr.write(`Permission denied\n${privateLog}${'x'.repeat(16 * 1024)}`);
      child.stderr.write('y'.repeat(1024 * 1024));
    },
    onRead({ child, now }) {
      if (now >= 125) { child.exitCode = 23; child.emit('exit', 23, null); }
    },
  });
  try {
    await assert.rejects(f.init(), reason => {
      assert.equal(reason.code, 'MISSING_DEPENDENCY');
      assert.match(reason.message, /exited before startup completed/u);
      assert.match(reason.message, /exit_code=23/u);
      assert.match(reason.message, /stderr_tags=permission_denied/u);
      assert.match(reason.message, /stderr_bytes=8192; stderr_truncated=true/u);
      assertRedacted(reason.message);
      assert.ok(reason.message.length < 512);
      return true;
    });
    assert.equal(f.elapsed(), 125);
    assert.equal(f.launches.length, 1);
  } finally { await f.cleanup(); }
});

test('spawn errors expose a known OS code without the executable path or message', async () => {
  const f = fixture({ portAt: Infinity, onSpawn(child) {
    child.emit('error', Object.assign(new Error(privateLog), { code: 'EACCES' }));
  } });
  try {
    await assert.rejects(f.init(), reason => {
      assert.equal(reason.code, 'MISSING_DEPENDENCY');
      assert.match(reason.message, /spawn_error=EACCES/u);
      assertRedacted(reason.message);
      return true;
    });
    assert.equal(f.launches.length, 1);
    assert.equal(f.elapsed(), 0);
  } finally { await f.cleanup(); }
});

test('WebSocket startup consumes the same deadline as port discovery', async () => {
  const f = fixture({ portAt: 3000, hangSocket: true });
  try {
    await assert.rejects(f.init(10_000), reason => {
      assert.equal(reason.code, 'UNKNOWN');
      assert.match(reason.message, /WebSocket startup timed out/u);
      return true;
    });
    assert.equal(f.elapsed(), 10_000);
    assert.deepEqual(f.timers, [7000]);
    assert.ok(f.sockets[0].closed);
    assert.equal(f.child.listenerCount('exit'), 0);
    assert.equal(f.child.listenerCount('error'), 1);
  } finally { await f.cleanup(); }
});

test('CDP initialization also stays inside the startup deadline without command retry', async () => {
  const f = fixture({ portAt: 5950, hangMethod: 'Browser.getVersion' });
  try {
    await assert.rejects(f.init(6000), reason => {
      assert.equal(reason.code, 'UNKNOWN');
      assert.match(reason.message, /CDP Browser.getVersion startup timed out/u);
      return true;
    });
    assert.equal(f.elapsed(), 6000);
    assert.deepEqual(f.timers, [50, 50]);
    assert.equal(f.calls.length, 1);
    assert.ok(f.kills.includes('SIGKILL'));
  } finally { await f.cleanup(); }
});

test('Chrome exit during CDP initialization reports exit state immediately', async () => {
  const f = fixture({ onSend(message, child) {
    if (message.method === 'Browser.getVersion') {
      child.exitCode = 24; child.emit('exit', 24, null);
    }
  } });
  try {
    await assert.rejects(f.init(), reason => {
      assert.equal(reason.code, 'UNKNOWN');
      assert.match(reason.message, /exited before startup completed/u);
      assert.match(reason.message, /exit_code=24/u);
      assert.doesNotMatch(reason.message, /timed out/u);
      return true;
    });
    assert.equal(f.elapsed(), 0);
    assert.equal(f.calls.length, 1);
  } finally { await f.cleanup(); }
});

test('successful startup clears capture but drains stderr and preserves live CDP/close lifecycle', async () => {
  const f = fixture({ onSpawn: child => child.stderr.write(privateLog) });
  try {
    await f.init(1000);
    assert.equal(f.launches[0].settings.detached, false);
    assert.deepEqual(Array.from(f.launches[0].settings.stdio), ['ignore', 'ignore', 'pipe']);
    assert.equal(f.child.stderr.listenerCount('data'), 1);
    assert.equal(f.child.stderr.destroyed, false);
    assert.equal(f.sockets[0].closed, false);
    f.child.stderr.write(privateLog.repeat(16 * 1024));
    assert.equal(f.child.stderr.readableLength, 0);
    assert.match(f.diagnostics(), /stderr_bytes=0/u);
    assertRedacted(f.diagnostics());
    f.advance(100_000);
    const status = await f.action({ id: 2, action: 'status' });
    assert.equal(status.cdp_verified, true);
    assert.deepEqual(f.calls.slice(-2).map(call => call.method), ['Browser.getVersion', 'Page.getFrameTree']);
    assert.deepEqual(f.timers.slice(-2), [1000, 1000]);
    const closed = await f.action({ id: 3, action: 'close' });
    assert.equal(closed.closed, true);
    assert.ok(f.kills.includes('SIGKILL'));
    assert.ok(f.child.stderr.destroyed);
    assert.ok(f.sockets[0].closed);
    assert.ok(f.removed());
  } finally { await f.cleanup(); }
});

test('startup budget validation happens before process launch', async () => {
  for (const budget of [0, -1, 60_001, 1.5, '60000', null, NaN, Infinity]) {
    const f = fixture();
    try {
      await assert.rejects(f.init(budget), reason => reason.code === 'INVALID_ARGUMENTS');
      assert.equal(f.launches.length, 0);
    } finally { await f.cleanup(); }
  }
});

test('legacy direct initialization retains a bounded default startup budget', async () => {
  const f = fixture({ portAt: Infinity });
  try {
    await assert.rejects(f.initialize({ id: 1, action: 'init', chrome_path: '/fixture/chrome',
      profile: '/isolated/profile', allowed_origins: ['https://example.test'] }), reason => reason.code === 'UNKNOWN');
    assert.equal(f.elapsed(), 30_000);
  } finally { await f.cleanup(); }
});

test('real Node child stderr stays out of JSON protocol and owned profile is removed', { timeout: 10_000 }, async () => {
  const root = await mkdtemp(join(tmpdir(), 'willdeep-browser-startup-test-'));
  let child;
  try {
    const env = { HOME: root, TMPDIR: root, TMP: root, TEMP: root, LANG: 'C.UTF-8' };
    for (const key of ['SystemRoot', 'WINDIR']) if (process.env[key]) env[key] = process.env[key];
    child = spawn(process.execPath, [fileURLToPath(new URL('./bridge.mjs', import.meta.url))], {
      cwd: root, env, stdio: ['pipe', 'pipe', 'pipe'], timeout: 5000, killSignal: 'SIGKILL',
    });
    let stdout = '', stderr = '';
    child.stdout.on('data', chunk => { stdout += chunk; assert.ok(stdout.length < 64 * 1024); });
    child.stderr.on('data', chunk => { stderr += chunk; assert.ok(stderr.length < 64 * 1024); });
    // Node rejects Chrome's flags and exits; this exercises real stderr pipes
    // and framing without launching a browser or any server.
    child.stdin.end(`${JSON.stringify({ id: 71, action: 'init', chrome_path: process.execPath,
      profile: join(root, 'profile'), allowed_origins: ['https://example.test'], startup_timeout_ms: 3000 })}\n`);
    const [code, signal] = await once(child, 'close');
    assert.equal(code, 0);
    assert.equal(signal, null);
    assert.equal(stderr, '');
    const frames = stdout.trim().split('\n');
    assert.equal(frames.length, 1);
    const result = JSON.parse(frames[0]);
    assert.equal(result.id, 71);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'MISSING_DEPENDENCY');
    assert.equal(result.error.retryable, false);
    assert.match(result.error.message, /exited before startup completed/u);
    assert.match(result.error.message, /stderr_bytes=[1-9]\d*/u);
    assert.ok(!result.error.message.includes(root));
    assert.doesNotMatch(result.error.message, /bad option|example\.test/u);
    await assert.rejects(stat(join(root, 'profile')), reason => reason.code === 'ENOENT');
  } finally {
    child?.kill('SIGKILL');
    await rm(root, { recursive: true, force: true });
  }
});
