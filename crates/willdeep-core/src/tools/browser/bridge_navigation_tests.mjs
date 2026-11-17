import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { createInterface } from 'node:readline';
import { PassThrough } from 'node:stream';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const source = (await readFile(new URL('./bridge.mjs', import.meta.url), 'utf8'))
  .replace(/^import .*;\n/gmu, '');
const ORIGIN = 'https://example.test';
const PAGE_URL = `${ORIGIN}/form`;
const FRAME = 'main-frame', LOADER = 'form-loader', SESSION = 'session';
const document = { nodeId: 1, backendNodeId: 1, nodeName: '#document', nodeType: 9,
  children: [{ nodeId: 2, backendNodeId: 2, nodeName: 'INPUT', nodeType: 1,
    attributes: ['id', 'name'], children: [] }] };

// Run the complete bridge JSONL protocol. Only OS, clock and CDP endpoints are
// deterministic adapters: no private bridge functions, real browser or server.
async function fixture(options = {}) {
  const input = new PassThrough(), output = new PassThrough();
  const requests = new Map(), commands = [], watchers = [], timers = new Map(), pauses = [];
  let socket, next = 1, now = 0, frame = { id: FRAME, loaderId: 'blank-loader', url: 'about:blank' };
  const child = Object.assign(new EventEmitter(), {
    stderr: new PassThrough(), exitCode: null, signalCode: null,
    kill(signal) {
      this.signalCode = signal;
      queueMicrotask(() => this.emit('exit', null, signal));
    },
  });
  const fakeProcess = Object.assign(new EventEmitter(), {
    versions: { node: '24.0.0' }, env: {}, stdin: input, stdout: output,
  });
  const replies = createInterface({ input: output });
  replies.on('line', line => {
    const response = JSON.parse(line), pending = requests.get(response.id);
    assert.ok(pending, 'every JSONL response must match one request');
    requests.delete(response.id); pending(response);
  });
  class Socket extends EventTarget {
    constructor() {
      super(); socket = this;
      queueMicrotask(() => this.dispatchEvent(new Event('open')));
    }
    send(raw) {
      const command = JSON.parse(raw); commands.push(command);
      queueMicrotask(() => {
        let result = {};
        switch (command.method) {
          case 'Browser.getVersion': result = { product: 'Chrome/fixture' }; break;
          case 'Target.getTargets': result = { targetInfos: [] }; break;
          case 'Target.createBrowserContext': result = { browserContextId: 'context' }; break;
          case 'Target.createTarget':
            result = { targetId: 'page' };
            emit('Target.attachedToTarget', { sessionId: SESSION, targetInfo: { targetId: 'page', type: 'page' } });
            break;
          case 'Page.getFrameTree': result = { frameTree: { frame } }; break;
          case 'Page.createIsolatedWorld': result = { executionContextId: 12 }; break;
          case 'Runtime.evaluate': result = { result: { value: 'complete' } }; break;
          case 'DOM.getDocument': result = { root: document }; break;
          case 'DOM.resolveNode': result = { object: { objectId: 'input-object' } }; break;
          case 'Runtime.callFunctionOn': result = { result: { value: true } }; break;
          case 'Page.navigate': result = { frameId: FRAME, loaderId: LOADER }; break;
        }
        const override = options.respond?.(command, { emit, commit, finish, reply });
        if (override !== false) reply(command, override ?? result);
        for (const watcher of watchers.splice(0)) watcher();
      });
    }
    close() {}
  }
  function reply(command, result) {
    socket.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ id: command.id, result }) }));
  }
  function emit(method, params, sessionId = SESSION) {
    socket.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ method, params, sessionId }) }));
  }
  function commit(url = PAGE_URL, loaderId = LOADER, frameId = FRAME) {
    const fragment = url.indexOf('#');
    frame = { id: frameId, loaderId, url: fragment < 0 ? url : url.slice(0, fragment),
      ...(fragment < 0 ? {} : { urlFragment: url.slice(fragment) }) };
    emit('Page.frameNavigated', { frame });
    emit('DOM.documentUpdated', {});
  }
  function finish(loaderId = LOADER, frameId = FRAME) {
    emit('Page.lifecycleEvent', { frameId, loaderId, name: 'DOMContentLoaded' });
  }
  const running = runInNewContext(`(async () => { ${source}\n })()`, {
    Buffer, URL: globalThis.URL, WebSocket: Socket, process: fakeProcess,
    performance: { now: () => now }, isAbsolute, join, createInterface,
    setTimeout(callback, ms) { const id = {}; timers.set(id, { callback, ms }); return id; },
    clearTimeout(id) { timers.delete(id); },
    async delay(ms) { now += ms; pauses.push(ms); },
    async stat() { return { isFile: () => true }; }, async mkdir() {}, async rm() {},
    async readFile() { return '9222\n/devtools/browser/fixture\n'; },
    spawn() { return child; },
  });
  function request(action, params = {}) {
    const id = next++;
    return new Promise(resolve => {
      requests.set(id, resolve);
      input.write(`${JSON.stringify({ id, action, ...params })}\n`);
    });
  }
  async function observed(predicate) {
    // Microtasks include bridge await continuations and protocol line delivery.
    for (let turn = 0; turn < 100; turn++) {
      if (predicate()) return;
      await Promise.resolve();
    }
    throw new Error('expected protocol milestone was not observed');
  }
  const initial = await request('init', { chrome_path: '/fixture/chrome', profile: '/fixture/profile',
    allowed_origins: [ORIGIN] });
  assert.equal(initial.ok, true);
  return {
    initial: initial.result, request, emit, commit, finish, commands, pauses, observed,
    reply, setFrame(value) { frame = value; },
    expire() {
      const pending = [...timers.values()];
      assert.ok(pending.length, 'one bounded navigation/CDP wait must be active');
      for (const timer of pending) { now += timer.ms; timer.callback(); }
    },
    async close() { input.end(); await running; replies.close(); },
  };
}
function opened(response) {
  assert.equal(response.ok, true, JSON.stringify(response));
  assert.equal(response.result.action, 'open');
  assert.equal(response.result.url, PAGE_URL);
  assert.ok(response.result.epoch > 1);
  return response.result;
}
function refused(response, code) {
  assert.equal(response.ok, false, JSON.stringify(response));
  assert.equal(response.error.code, code);
  assert.equal(response.error.retryable, false);
}
function navigationCount(f) { return f.commands.filter(command => command.method === 'Page.navigate').length; }

test('open tolerates its own commit between the navigate acknowledgement and readiness query', { timeout: 2000 }, async () => {
  let committed = false;
  const f = await fixture({ respond(command, { commit, finish }) {
    if (command.method === 'Page.navigate') {
      // Commit is released explicitly after the command acknowledgement.
      return { frameId: FRAME, loaderId: LOADER };
    }
    if (command.method === 'Page.getFrameTree' && !committed && f.commands.some(value => value.method === 'Page.navigate')) {
      committed = true; commit(); finish();
    }
  } });
  try {
    const pending = f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL });
    await f.observed(() => navigationCount(f) === 1);
    // The old implementation starts readiness on the old epoch and fails when
    // commit arrives while Page.getFrameTree is in flight. A commit barrier
    // waits for these events and never queries the obsolete document.
    await f.observed(() => f.commands.some(command => command.method === 'Page.getFrameTree' && command.id > f.commands.find(value => value.method === 'Page.navigate').id) || f.commands.at(-1).method === 'Page.navigate');
    for (let turn = 0; turn < 12; turn++) await Promise.resolve();
    if (!committed) { committed = true; f.commit(); f.finish(); }
    const receipt = opened(await pending);
    const snapshot = await f.request('snapshot', { expected_epoch: receipt.epoch });
    assert.equal(snapshot.ok, true, JSON.stringify(snapshot));
    assert.equal(snapshot.result.epoch, receipt.epoch);
    assert.equal(snapshot.result.nodes[0].id, 'name');
    assert.equal(navigationCount(f), 1);
  } finally { await f.close(); }
});

test('open waits for the acknowledged loader, never reporting the old complete document', { timeout: 2000 }, async () => {
  const f = await fixture();
  try {
    let response;
    const pending = f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL }).then(value => { response = value; return value; });
    await f.observed(() => navigationCount(f) === 1);
    for (let turn = 0; turn < 50; turn++) await Promise.resolve();
    assert.equal(response, undefined, 'old about:blank readiness cannot complete open');
    f.emit('Page.lifecycleEvent', { frameId: FRAME, loaderId: 'old-loader', name: 'DOMContentLoaded' });
    f.finish(LOADER, 'child-frame');
    for (let turn = 0; turn < 10; turn++) await Promise.resolve();
    assert.equal(response, undefined, 'other loader/frame readiness cannot complete open');
    f.commit();
    for (let turn = 0; turn < 10; turn++) await Promise.resolve();
    assert.equal(response, undefined, 'commit alone does not establish DOM readiness');
    f.finish();
    opened(await pending);
    assert.equal(navigationCount(f), 1);
    assert.ok(!f.commands.some(command => command.method === 'Runtime.evaluate'), 'old document.readyState must not be consulted');
    assert.deepEqual(f.pauses, [], 'navigation completion must follow CDP events without fixed sleeps');
  } finally { await f.close(); }
});

test('commit and DOM readiness before the navigate acknowledgement are retained, including redirects', { timeout: 2000 }, async () => {
  const destination = `${ORIGIN}/redirected`;
  const f = await fixture({ respond(command, { commit, finish }) {
    if (command.method === 'Page.navigate') { commit(destination); finish(); }
  } });
  try {
    const response = await f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL });
    assert.equal(response.ok, true, JSON.stringify(response));
    assert.equal(response.result.url, destination);
    const snapshot = await f.request('snapshot', { expected_epoch: response.result.epoch });
    assert.equal(snapshot.ok, true, JSON.stringify(snapshot));
    assert.equal(navigationCount(f), 1);
  } finally { await f.close(); }
});

test('same-document open accepts CDP urlFragment, waits for its event and invalidates earlier epochs', { timeout: 2000 }, async () => {
  const f = await fixture({ respond(command, { commit, finish }) {
    if (command.method === 'Page.navigate' && !command.params.url.includes('#')) { commit(); finish(); }
    if (command.method === 'Page.navigate' && command.params.url.includes('#')) return { frameId: FRAME };
  } });
  try {
    const first = opened(await f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL }));
    let response;
    const pending = f.request('open', { expected_epoch: first.epoch, url: `${PAGE_URL}#details` }).then(value => { response = value; return value; });
    await f.observed(() => navigationCount(f) === 2);
    for (let turn = 0; turn < 50; turn++) await Promise.resolve();
    assert.equal(response, undefined);
    f.setFrame({ id: FRAME, loaderId: LOADER, url: PAGE_URL, urlFragment: '#details' });
    f.emit('Page.navigatedWithinDocument', { frameId: FRAME, url: `${PAGE_URL}#details` });
    const second = await pending;
    assert.equal(second.ok, true, JSON.stringify(second));
    assert.equal(second.result.url, `${PAGE_URL}#details`);
    assert.ok(second.result.epoch > first.epoch);
    const status = await f.request('status');
    assert.equal(status.ok, true, JSON.stringify(status));
    assert.equal(status.result.url, `${PAGE_URL}#details`);
    const snapshot = await f.request('snapshot', { expected_epoch: second.result.epoch });
    assert.equal(snapshot.ok, true, JSON.stringify(snapshot));
    assert.equal(snapshot.result.epoch, second.result.epoch);
    refused(await f.request('fill', { expected_epoch: first.epoch, backend_node_id: 2, text: 'stale' }), 'STALE_PAGE');
    assert.ok(!f.commands.some(command => command.method === 'Runtime.callFunctionOn'));
    const filled = await f.request('fill', { expected_epoch: second.result.epoch, backend_node_id: 2, text: 'current' });
    assert.equal(filled.ok, true, JSON.stringify(filled));
    assert.equal(navigationCount(f), 2);
  } finally { await f.close(); }
});

test('same-document open rejects a successor loader committed during DOM synchronization without readiness', { timeout: 2000 }, async () => {
  let replaceDocument = false;
  const f = await fixture({ respond(command, { commit, finish }) {
    if (command.method === 'Page.navigate') {
      if (command.params.url === PAGE_URL) { commit(); finish(); }
      else return { frameId: FRAME };
    }
    if (command.method === 'DOM.getDocument' && replaceDocument) {
      replaceDocument = false;
      commit(`${ORIGIN}/unexpected`, 'unexpected-loader');
    }
  } });
  try {
    const first = opened(await f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL }));
    const pending = f.request('open', { expected_epoch: first.epoch, url: `${PAGE_URL}#details` });
    await f.observed(() => navigationCount(f) === 2);
    f.setFrame({ id: FRAME, loaderId: LOADER, url: PAGE_URL, urlFragment: '#details' });
    replaceDocument = true;
    f.emit('Page.navigatedWithinDocument', { frameId: FRAME, url: `${PAGE_URL}#details` });
    refused(await pending, 'STALE_PAGE');
    assert.equal(replaceDocument, false, 'the successor commit must occur during DOM.getDocument');
    assert.equal(navigationCount(f), 2);
    assert.deepEqual(f.pauses, [], 'a replaced document must be rejected without a delay or retry');
  } finally { await f.close(); }
});

for (const change of ['navigation', 'DOM']) {
  test(`a real ${change} change after open still rejects stale element input`, { timeout: 2000 }, async () => {
    const f = await fixture({ respond(command, { commit, finish }) {
      if (command.method === 'Page.navigate') { commit(); finish(); }
    } });
    try {
      const receipt = opened(await f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL }));
      assert.equal((await f.request('snapshot', { expected_epoch: receipt.epoch })).ok, true);
      if (change === 'navigation') { f.commit(`${ORIGIN}/next`, 'next-loader'); f.finish('next-loader'); }
      else f.emit('DOM.documentUpdated', {});
      refused(await f.request('fill', { expected_epoch: receipt.epoch, backend_node_id: 2, text: 'stale' }), 'STALE_PAGE');
      assert.ok(!f.commands.some(command => command.method === 'DOM.resolveNode' || command.method === 'Runtime.callFunctionOn'));
      assert.equal(navigationCount(f), 1);
    } finally { await f.close(); }
  });
}

test('a different committed loader cannot satisfy the requested navigation', { timeout: 2000 }, async () => {
  const f = await fixture({ respond(command, { commit, finish }) {
    if (command.method === 'Page.navigate') { commit(); finish(); }
    if (command.method === 'DOM.getDocument') commit(`${ORIGIN}/unexpected`, 'unexpected-loader');
  } });
  try {
    refused(await f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL }), 'STALE_PAGE');
    assert.equal(navigationCount(f), 1);
  } finally { await f.close(); }
});

test('missing navigation commit has a bounded UNKNOWN outcome and is never retried', { timeout: 2000 }, async () => {
  const f = await fixture();
  try {
    const pending = f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL });
    await f.observed(() => navigationCount(f) === 1);
    for (let turn = 0; turn < 10; turn++) await Promise.resolve();
    f.expire();
    refused(await pending, 'UNKNOWN');
    assert.equal(navigationCount(f), 1);
  } finally { await f.close(); }
});

test('CDP connection failure during commit wait remains UNKNOWN without retry', { timeout: 2000 }, async () => {
  const f = await fixture();
  try {
    const pending = f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL });
    await f.observed(() => navigationCount(f) === 1);
    for (let turn = 0; turn < 10; turn++) await Promise.resolve();
    f.emit('Target.detachedFromTarget', { sessionId: SESSION });
    refused(await pending, 'UNKNOWN');
    assert.equal(navigationCount(f), 1);
  } finally { await f.close(); }
});

test('Chrome navigation rejection remains NAVIGATION_FAILED without a success receipt or retry', { timeout: 2000 }, async () => {
  const f = await fixture({ respond(command) {
    if (command.method === 'Page.navigate') return { frameId: FRAME, errorText: 'net::ERR_ABORTED' };
  } });
  try {
    refused(await f.request('open', { expected_epoch: f.initial.epoch, url: PAGE_URL }), 'NAVIGATION_FAILED');
    assert.equal(navigationCount(f), 1);
  } finally { await f.close(); }
});
