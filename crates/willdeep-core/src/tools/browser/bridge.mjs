// Built-in Node >=24 CDP bridge. No packages, arbitrary JS action or user profile.
import { spawn } from 'node:child_process';
import { mkdir, readFile, rm, stat } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

const MAX_FRAME = 12 * 1024 * 1024;
const MAX_TEXT = 16 * 1024;
const MAX_NODES = 200;
const MAX_PNG = 8 * 1024 * 1024;
const RPC_TIMEOUT = 15_000;
const STATUS_TIMEOUT = 1000;
const DEFAULT_STARTUP_TIMEOUT_MS = 30_000;
const MAX_STARTUP_TIMEOUT_MS = 60_000;
const STARTUP_POLL_MS = 25;
const MAX_CHROME_STDERR_BYTES = 8192;
const CONTROL_WORLD = 'willdeep-controlled-dom';
let chrome, cdp, profile, origins, pageSession, pageTarget, context;
let chromeStderr, launchFailure;
let epoch = 1, pageUrl = 'about:blank', documentId, blocked = 0, mainStatus = null;
let fatal, initialized = false, creatingPage = false, pageReady;
let cleanupStarted = false, navigation;
const configured = new Set();
const error = (code, message) => Object.assign(new Error(message), { code });

function keys(value, allowed, required = []) {
  if (!value || typeof value !== 'object' || Array.isArray(value)
      || Object.keys(value).some(key => !allowed.includes(key))
      || required.some(key => !Object.hasOwn(value, key))) {
    throw error('INVALID_ARGUMENTS', 'Missing or unsupported bridge arguments');
  }
}
function string(value, limit = MAX_TEXT) {
  if (typeof value !== 'string' || Buffer.byteLength(value) > limit) {
    throw error('INVALID_ARGUMENTS', 'String argument is missing or too large');
  }
  return value;
}
function checkedUrl(raw) {
  string(raw, 8192);
  if (/[\x00-\x20\x7f\\]/u.test(raw)) throw error('ORIGIN_DENIED', 'Invalid URL characters');
  let url;
  try { url = new URL(raw); } catch { throw error('ORIGIN_DENIED', 'Invalid absolute URL'); }
  // Reconstructing from origin omits userinfo, including an empty username.
  if (!['http:', 'https:'].includes(url.protocol) || !url.href.startsWith(`${url.origin}/`)
      || !/^[a-z0-9.:[\]-]+$/iu.test(url.hostname) || url.hostname.length > 255
      || !origins.has(url.origin)) {
    throw error('ORIGIN_DENIED', 'URL scheme, credentials or origin is not permitted');
  }
  return url.href;
}
function permitted(raw) { try { checkedUrl(raw); return true; } catch { return false; } }
const frameUrl = frame => frame.url + (frame.urlFragment ?? '');
function checkEpoch(expected) {
  if (!Number.isSafeInteger(expected) || expected !== epoch) {
    throw error('STALE_PAGE', 'Page epoch changed; obtain a new snapshot');
  }
  if (pageUrl !== 'about:blank' && !permitted(pageUrl)) throw error('ORIGIN_DENIED', 'Current page origin is forbidden');
}
function status() {
  if (fatal) throw fatal;
  return { action: 'status', epoch, url: pageUrl, blocked_requests: blocked,
    http_status: mainStatus, profile_isolated: true, node_version: process.versions.node,
    network_boundary: 'navigation and intercepted requests; not process network isolation' };
}

function captureChromeStderr(stream) {
  let bytes = Buffer.alloc(MAX_CHROME_STDERR_BYTES), length = 0;
  let capturing = true, truncated = false, readFailed = false;
  stream.on('data', chunk => {
    // Always drain the pipe, including after successful startup. Retain only a
    // bounded prefix; a full pipe must never block Chrome or its CDP transport.
    if (!capturing) return;
    const remaining = bytes.length - length;
    length += chunk.copy(bytes, length, 0, Math.min(chunk.length, remaining));
    truncated ||= chunk.length > remaining;
  });
  stream.on('error', () => { readFailed = true; });
  return {
    stop() { capturing = false; bytes.fill(0); bytes = Buffer.alloc(0); length = 0; },
    summary() {
      // Only fixed labels leave this collector. Never echo arbitrary stderr,
      // which can contain environment values, credentials, paths or remote URLs.
      const text = bytes.toString('utf8', 0, length);
      const tags = [
        ['sandbox_unavailable', /No usable sandbox|SUID sandbox|Running as root without --no-sandbox|Failed to move to new namespace/iu],
        ['permission_denied', /Permission denied|Operation not permitted/iu],
        ['profile_unavailable', /SingletonLock|ProcessSingleton|Failed to create.*(?:directory|profile)/iu],
        ['missing_runtime_library', /error while loading shared libraries|Library not loaded|dyld\[/iu],
        ['display_unavailable', /Missing X server|cannot open display/iu],
        ['resource_exhausted', /Cannot allocate memory|Too many open files|Resource temporarily unavailable/iu],
        ['browser_crash', /FATAL:|Check failed|Segmentation fault|Received signal/iu],
      ].filter(([, pattern]) => pattern.test(text)).map(([tag]) => tag);
      return `stderr_tags=${tags.join(',') || (length ? 'unclassified' : 'none')}; stderr_bytes=${length}; stderr_truncated=${truncated}; stderr_read_failed=${readFailed}`;
    },
  };
}
function startupError(message, code = 'UNKNOWN') {
  const exitCode = Number.isInteger(chrome?.exitCode) ? chrome.exitCode : 'none';
  const signal = /^SIG[A-Z0-9]{1,12}$/u.test(chrome?.signalCode) ? chrome.signalCode : 'none';
  return error(code, `${message}; spawn_error=${launchFailure ?? 'none'}; exit_code=${exitCode}; exit_signal=${signal}; ${chromeStderr.summary()}`);
}
function startupRemaining(deadline, stage) {
  const remaining = deadline - performance.now();
  if (remaining <= 0) throw startupError(`Chrome ${stage} startup timed out`);
  return remaining;
}
function checkStartupProcess(code = 'MISSING_DEPENDENCY') {
  if (launchFailure || chrome.exitCode !== null || chrome.signalCode !== null) {
    throw startupError('Chrome exited before startup completed', code);
  }
}

class Cdp {
  constructor(socket, startupDeadline) {
    this.socket = socket; this.next = 1; this.pending = new Map();
    this.startupDeadline = startupDeadline;
    chrome.once('exit', () => {
      if (this.startupDeadline !== undefined) this.fail(startupError('Chrome exited before startup completed'));
    });
    socket.addEventListener('message', event => {
      try {
        if (typeof event.data !== 'string' || Buffer.byteLength(event.data) > MAX_FRAME) {
          throw error('UNKNOWN', 'CDP frame exceeded its budget');
        }
        const message = JSON.parse(event.data);
        if (message.id) {
          const pending = this.pending.get(message.id);
          if (!pending) return;
          this.pending.delete(message.id); clearTimeout(pending.timer);
          if (message.error) pending.reject(error('CDP_ERROR', `CDP ${pending.method} failed`));
          else pending.resolve(message.result ?? {});
        } else {
          Promise.resolve(onEvent(message)).catch(reason => this.fail(reason));
        }
      } catch (reason) { this.fail(reason); }
    });
    socket.addEventListener('close', () => this.fail(error('UNKNOWN', 'CDP connection closed')));
    socket.addEventListener('error', () => this.fail(error('UNKNOWN', 'CDP connection failed')));
  }
  send(method, params = {}, sessionId, timeout = RPC_TIMEOUT) {
    if (fatal) return Promise.reject(fatal);
    if (this.startupDeadline !== undefined) {
      try { timeout = Math.min(timeout, startupRemaining(this.startupDeadline, 'CDP')); }
      catch (reason) { this.fail(reason); return Promise.reject(reason); }
    }
    const id = this.next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        const reason = this.startupDeadline !== undefined
          ? startupError(`Chrome CDP ${method} startup timed out`)
          : error('UNKNOWN', `CDP ${method} timed out; no automatic retry`);
        reject(reason); this.fail(reason);
      }, timeout);
      this.pending.set(id, { resolve, reject, timer, method });
      try { this.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })); }
      catch { this.fail(error('UNKNOWN', 'CDP write failed')); }
    });
  }
  fail(reason) {
    fatal ??= reason instanceof Error ? reason : error('UNKNOWN', 'CDP failed');
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(fatal); }
    this.pending.clear();
    navigation?.fail(fatal);
    chrome?.kill('SIGKILL');
  }
}
const send = (method, params = {}) => cdp.send(method, params, pageSession);

class Navigation {
  constructor() {
    this.frames = new Map(); this.ready = new Set(); this.within = new Map();
    this.check = () => {};
  }
  observe(message) {
    if (message.sessionId !== pageSession) return;
    const p = message.params ?? {};
    if (message.method === 'Page.frameNavigated' && !p.frame.parentId) {
      this.frames.set(p.frame.id, p.frame.loaderId);
    } else if (message.method === 'Page.lifecycleEvent' && ['DOMContentLoaded','load'].includes(p.name)) {
      this.ready.add(JSON.stringify([p.frameId, p.loaderId]));
    } else if (message.method === 'Page.navigatedWithinDocument') {
      this.within.set(p.frameId, p.url);
    }
    this.check();
  }
  wait(result, url) {
    if (fatal) return Promise.reject(fatal);
    if (typeof result.frameId !== 'string' || (result.loaderId !== undefined && typeof result.loaderId !== 'string')) {
      return Promise.reject(error('UNKNOWN', 'Chrome omitted navigation identity; no automatic retry'));
    }
    return new Promise((resolve, reject) => {
      this.reject = reject;
      this.timer = setTimeout(() => cdp.fail(error('UNKNOWN', 'Navigation commit/readiness timed out; no automatic retry')), RPC_TIMEOUT);
      this.check = () => {
        const complete = result.loaderId !== undefined
          ? this.frames.get(result.frameId) === result.loaderId && this.ready.has(JSON.stringify([result.frameId, result.loaderId]))
          : this.within.get(result.frameId) === url;
        if (complete) { this.dispose(); resolve(); }
      };
      this.check();
    });
  }
  fail(reason) { this.dispose(); this.reject?.(reason); }
  dispose() { clearTimeout(this.timer); this.check = () => {}; }
}

async function liveStatus() {
  // Probe the browser transport and the owned renderer independently. A live
  // Node process/cache says nothing about a hung Chrome or detached target.
  if (!chrome || chrome.exitCode !== null || chrome.signalCode !== null) {
    throw error('UNKNOWN', 'Owned Chrome is no longer running');
  }
  const [, { frameTree }] = await Promise.all([
    cdp.send('Browser.getVersion', {}, undefined, STATUS_TIMEOUT),
    cdp.send('Page.getFrameTree', {}, pageSession, STATUS_TIMEOUT),
  ]);
  if (!frameTree?.frame || frameUrl(frameTree.frame) !== pageUrl) {
    throw error('STALE_PAGE', 'Live page and bridge epoch disagree');
  }
  return { ...status(), cdp_verified: true };
}

async function isolatedWorld(expected) {
  checkEpoch(expected);
  const { frameTree } = await send('Page.getFrameTree');
  checkEpoch(expected);
  if (!frameTree?.frame || frameUrl(frameTree.frame) !== pageUrl) throw error('STALE_PAGE', 'Controlled frame changed');
  const { executionContextId } = await send('Page.createIsolatedWorld', {
    frameId: frameTree.frame.id, worldName: CONTROL_WORLD,
  });
  checkEpoch(expected);
  return executionContextId;
}

// Defense in depth for transports not reliably intercepted by CDP Fetch. The
// CSP and Fetch hooks remain the origin boundary; this bootstrap is not a sandbox.
const BOOTSTRAP = `(() => {
  const denied = function() { throw new DOMException('Disabled in controlled browser', 'SecurityError'); };
  for (const name of ['WebSocket','WebTransport','RTCPeerConnection','webkitRTCPeerConnection','Worker','SharedWorker']) {
    try { Object.defineProperty(globalThis, name, { value: denied, configurable: false, writable: false }); } catch {}
  }
  try { Object.defineProperty(Navigator.prototype, 'serviceWorker', { get: () => undefined, configurable: false }); } catch {}
  try { Object.defineProperty(globalThis, 'open', { value: () => null, configurable: false, writable: false }); } catch {}
})();`;

async function configurePage(sessionId) {
  await cdp.send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true }, sessionId);
  await cdp.send('Page.enable', {}, sessionId);
  await cdp.send('Network.enable', {}, sessionId);
  await cdp.send('Network.setBypassServiceWorker', { bypass: true }, sessionId);
  await cdp.send('Network.setCacheDisabled', { cacheDisabled: true }, sessionId);
  await cdp.send('Network.setBlockedURLs', { urls: ['ws://*', 'wss://*', 'file://*', 'ftp://*'] }, sessionId);
  await cdp.send('Fetch.enable', { patterns: [
    { urlPattern: '*', requestStage: 'Request' },
    { urlPattern: '*', resourceType: 'Document', requestStage: 'Response' },
  ], handleAuthRequests: true }, sessionId);
  await cdp.send('Page.setLifecycleEventsEnabled', { enabled: true }, sessionId);
  await cdp.send('Page.setInterceptFileChooserDialog', { enabled: true }, sessionId);
  await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: BOOTSTRAP }, sessionId);
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false }, sessionId);
  await cdp.send('Runtime.runIfWaitingForDebugger', {}, sessionId);
  configured.add(sessionId);
}

async function onEvent(message) {
  const p = message.params ?? {};
  if (message.method === 'Target.attachedToTarget') {
    const info = p.targetInfo;
    if (info.type === 'page' && creatingPage && !pageTarget) {
      pageTarget = info.targetId; pageSession = p.sessionId;
      pageReady = configurePage(p.sessionId);
      await pageReady;
    } else {
      // Includes popups, iframe targets and service/shared/dedicated workers.
      // Targets remain paused until closed; no worker is resumed.
      await cdp.send('Target.closeTarget', { targetId: info.targetId });
    }
  } else if (message.method === 'Fetch.authRequired') {
    await cdp.send('Fetch.continueWithAuth', { requestId: p.requestId, authChallengeResponse: { response: 'CancelAuth' } }, message.sessionId);
  } else if (message.method === 'Fetch.requestPaused') {
    const requestUrl = p.request.url;
    if (!configured.has(message.sessionId) && message.sessionId !== pageSession) {
      await cdp.send('Fetch.failRequest', { requestId: p.requestId, errorReason: 'BlockedByClient' }, message.sessionId);
      return;
    }
    if (!permitted(requestUrl) || p.resourceType === 'WebSocket') {
      blocked++;
      await cdp.send('Fetch.failRequest', { requestId: p.requestId, errorReason: 'BlockedByClient' }, message.sessionId);
      return;
    }
    if (p.responseStatusCode) {
      const sources = [...origins].join(' ');
      const csp = `default-src ${sources}; script-src ${sources} 'unsafe-inline'; style-src ${sources} 'unsafe-inline'; img-src ${sources} data:; connect-src ${sources}; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action ${sources}`;
      const headers = [...(p.responseHeaders ?? []), { name: 'Content-Security-Policy', value: csp }];
      // Do not remove a site's existing CSP; additional policies only restrict.
      await cdp.send('Fetch.continueResponse', { requestId: p.requestId, responseCode: p.responseStatusCode, responseHeaders: headers }, message.sessionId);
    } else {
      await cdp.send('Fetch.continueRequest', { requestId: p.requestId }, message.sessionId);
    }
  } else if (message.sessionId === pageSession && message.method === 'Page.frameNavigated' && !p.frame.parentId) {
    epoch++; pageUrl = frameUrl(p.frame); documentId = undefined;
    if (pageUrl !== 'about:blank' && !permitted(pageUrl)) throw error('ORIGIN_DENIED', 'Page navigated outside allowed origins');
  } else if (message.sessionId === pageSession && message.method === 'Page.navigatedWithinDocument') {
    epoch++; pageUrl = p.url;
    if (!permitted(pageUrl)) throw error('ORIGIN_DENIED', 'History navigation outside allowed origins');
  } else if (message.sessionId === pageSession && message.method === 'DOM.documentUpdated') {
    epoch++; documentId = undefined;
  } else if (message.sessionId === pageSession && message.method === 'Network.responseReceived' && p.type === 'Document') {
    mainStatus = p.response.status;
  } else if (message.method === 'Target.detachedFromTarget' && p.sessionId === pageSession && initialized && !cleanupStarted) {
    throw error('UNKNOWN', 'Controlled page was closed');
  } else if (message.method === 'Network.webSocketCreated') {
    throw error('ORIGIN_DENIED', 'WebSocket transport is disabled');
  } else if (message.method === 'Page.fileChooserOpened') {
    throw error('UPLOAD_DISABLED', 'File chooser/upload is disabled');
  }
  navigation?.observe(message);
}

async function initialize(request) {
  keys(request, ['id','action','chrome_path','profile','allowed_origins','startup_timeout_ms'], ['chrome_path','profile','allowed_origins']);
  if (initialized || chrome) throw error('INVALID_ARGUMENTS', 'Bridge can be initialized only once');
  const startupTimeout = Object.hasOwn(request, 'startup_timeout_ms') ? request.startup_timeout_ms : DEFAULT_STARTUP_TIMEOUT_MS;
  if (!Number.isSafeInteger(startupTimeout) || startupTimeout < 1 || startupTimeout > MAX_STARTUP_TIMEOUT_MS) {
    throw error('INVALID_ARGUMENTS', 'Startup timeout must be an integer between 1 and 60000 milliseconds');
  }
  const startupDeadline = performance.now() + startupTimeout;
  if (Number(process.versions.node.split('.')[0]) < 24 || typeof WebSocket !== 'function') {
    throw error('MISSING_DEPENDENCY', 'Browser bridge requires Node 24 or newer with built-in WebSocket');
  }
  string(request.chrome_path, 8192); string(request.profile, 8192);
  if (!isAbsolute(request.chrome_path) || !isAbsolute(request.profile)) throw error('INVALID_ARGUMENTS', 'Chrome and profile paths must be absolute');
  if (!(await stat(request.chrome_path)).isFile()) throw error('MISSING_DEPENDENCY', 'Configured Chrome is not a file');
  if (!Array.isArray(request.allowed_origins) || request.allowed_origins.length === 0 || request.allowed_origins.length > 32) {
    throw error('INVALID_ARGUMENTS', 'Configure 1..32 exact origins');
  }
  origins = new Set(request.allowed_origins);
  if (origins.size !== request.allowed_origins.length) throw error('INVALID_ARGUMENTS', 'Duplicate origins');
  for (const origin of origins) {
    if (new URL(checkedUrl(origin)).origin !== origin) throw error('INVALID_ARGUMENTS', 'Origins must be canonical without a trailing slash or path');
  }
  // mkdir without recursive refuses all existing profiles, including user data.
  await mkdir(request.profile, { mode: 0o700 }); profile = request.profile;
  const hosts = [...new Set([...origins].map(origin => new URL(origin).hostname))];
  const flags = [
    '--headless=new', '--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0',
    `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check',
    '--disable-background-networking', '--disable-component-update', '--disable-default-apps',
    '--disable-extensions', '--disable-sync', '--disable-domain-reliability',
    '--disable-client-side-phishing-detection', '--safebrowsing-disable-auto-update',
    '--disable-breakpad', '--disable-crash-reporter', '--metrics-recording-only', '--no-pings',
    '--disable-quic', '--force-webrtc-ip-handling-policy=disable_non_proxied_udp',
    '--disable-features=ServiceWorker,SharedWorker,WebTransport,Prerender2,SpeculationRulesPrefetch,OptimizationHints,MediaRouter,AutofillServerCommunication,CertificateTransparencyComponentUpdater',
    '--proxy-server=direct://', '--proxy-bypass-list=*',
    `--host-resolver-rules=MAP * ~NOTFOUND, ${hosts.map(host => `EXCLUDE ${host}`).join(', ')}`,
    'about:blank',
  ];
  const cleanEnv = Object.fromEntries(['HOME','TMPDIR','TMP','TEMP','LANG','SystemRoot','WINDIR']
    .filter(name => process.env[name] !== undefined).map(name => [name, process.env[name]]));
  chrome = spawn(request.chrome_path, flags, { env: cleanEnv, stdio: ['ignore', 'ignore', 'pipe'], detached: false });
  chromeStderr = captureChromeStderr(chrome.stderr);
  chrome.on('error', reason => {
    // Spawn error messages include executable paths. Expose only known OS codes.
    launchFailure = ['EACCES','EPERM','ENOENT','ENOEXEC','ENOMEM','EMFILE','ENFILE','ETXTBSY','EAGAIN'].includes(reason.code) ? reason.code : 'OTHER';
  });
  let portFile;
  while (portFile === undefined) {
    checkStartupProcess();
    startupRemaining(startupDeadline, 'CDP');
    try { portFile = await readFile(join(profile, 'DevToolsActivePort'), 'utf8'); }
    catch (reason) { if (reason.code !== 'ENOENT') throw reason; }
    checkStartupProcess();
    const remaining = startupRemaining(startupDeadline, 'CDP');
    if (portFile === undefined) await delay(Math.min(STARTUP_POLL_MS, remaining));
  }
  const [port, path] = portFile.trim().split('\n');
  if (!/^\d{1,5}$/u.test(port) || Number(port) < 1 || Number(port) > 65535 || !/^\/devtools\/browser\/[\w-]+$/u.test(path)) {
    throw startupError('Invalid Chrome CDP endpoint');
  }
  const socket = new WebSocket(`ws://127.0.0.1:${port}${path}`);
  await new Promise((resolve, reject) => {
    const finish = reason => {
      clearTimeout(timer);
      socket.removeEventListener('open', opened); socket.removeEventListener('error', failed);
      chrome.removeListener('exit', exited); chrome.removeListener('error', exited);
      if (reason) { socket.close(); reject(reason); } else resolve();
    };
    const opened = () => finish();
    const failed = () => finish(startupError('Chrome WebSocket failed during startup'));
    const exited = () => finish(startupError('Chrome exited before startup completed'));
    const timer = setTimeout(() => finish(startupError('Chrome WebSocket startup timed out')),
      startupRemaining(startupDeadline, 'WebSocket'));
    socket.addEventListener('open', opened, { once: true });
    socket.addEventListener('error', failed, { once: true });
    chrome.once('exit', exited); chrome.once('error', exited);
  });
  cdp = new Cdp(socket, startupDeadline);
  const version = await cdp.send('Browser.getVersion');
  await cdp.send('Browser.setDownloadBehavior', { behavior: 'deny', eventsEnabled: true });
  const targets = await cdp.send('Target.getTargets');
  for (const target of targets.targetInfos) { if (target.type === 'page') await cdp.send('Target.closeTarget', { targetId: target.targetId }); }
  ({ browserContextId: context } = await cdp.send('Target.createBrowserContext', { disposeOnDetach: true }));
  await cdp.send('Browser.setDownloadBehavior', { behavior: 'deny', browserContextId: context, eventsEnabled: true });
  await cdp.send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true });
  creatingPage = true;
  const target = await cdp.send('Target.createTarget', { url: 'about:blank', browserContextId: context });
  for (let attempt = 0; !pageReady && attempt < 100; attempt++) {
    checkStartupProcess('UNKNOWN');
    await delay(Math.min(10, startupRemaining(startupDeadline, 'page attachment')));
  }
  if (!pageReady || target.targetId !== pageTarget) throw startupError('Could not attach isolated page');
  await pageReady; creatingPage = false; initialized = true;
  await send('DOM.enable');
  startupRemaining(startupDeadline, 'CDP');
  cdp.startupDeadline = undefined;
  chromeStderr.stop();
  return { ...status(), chrome_version: version.product };
}

const attrs = node => Object.fromEntries(Array.from({ length: (node.attributes ?? []).length / 2 }, (_, i) => [node.attributes[i * 2], node.attributes[i * 2 + 1]]));
const compact = text => String(text).replace(/\s+/gu, ' ').trim();
function textOf(node, budget = { count: 0 }) {
  if (++budget.count > 20_000) return '';
  if (['SCRIPT','STYLE','NOSCRIPT','TEMPLATE'].includes(node.nodeName)) return '';
  const a = attrs(node);
  if ('hidden' in a || a['aria-hidden'] === 'true' || a.type === 'hidden') return '';
  if (node.nodeType === 3) return node.nodeValue.slice(0, 32_768);
  let result = '';
  for (const child of node.children ?? []) { result += ` ${textOf(child, budget)}`; if (result.length > 32_768) break; }
  return compact(result).slice(0, 32_768);
}
async function tree() {
  const { root } = await send('DOM.getDocument', { depth: -1, pierce: false });
  documentId = root.nodeId;
  return root;
}
async function snapshot(expected) {
  checkEpoch(expected);
  const root = await tree();
  const generation = epoch, nodes = [], labels = new Map();
  let visited = 0, truncated = false;
  function collectLabels(node) {
    if (++visited > 20_000) return;
    const a = attrs(node);
    if (node.nodeName === 'LABEL' && a.for) labels.set(a.for, textOf(node).slice(0, 256));
    for (const child of node.children ?? []) collectLabels(child);
  }
  collectLabels(root); visited = 0;
  function walk(node, hidden = false) {
    if (++visited > 20_000) { truncated = true; return; }
    const a = attrs(node);
    hidden ||= 'hidden' in a || a['aria-hidden'] === 'true' || a.type === 'hidden';
    if (!hidden && (['A','BUTTON','INPUT','TEXTAREA','SELECT','H1','H2','H3','P','LI'].includes(node.nodeName) || a.role)) {
      if (nodes.length < MAX_NODES) {
        const name = a['aria-label'] || labels.get(a.id) || a.placeholder || textOf(node) || a.name || a.id || node.nodeName.toLowerCase();
        nodes.push({ backend_node_id: node.backendNodeId, tag: node.nodeName.toLowerCase(),
          role: a.role || null, name: name.slice(0, 256), id: (a.id || '').slice(0, 128),
          input_type: node.nodeName === 'INPUT' ? (a.type || 'text') : null,
          href: a.href ? a.href.slice(0, 1024) : null, disabled: 'disabled' in a });
      } else truncated = true;
    }
    if (hidden || ['SCRIPT','STYLE','TEMPLATE'].includes(node.nodeName)) return;
    for (const child of node.children ?? []) walk(child, hidden);
  }
  walk(root);
  checkEpoch(generation);
  return { action: 'snapshot', epoch, url: pageUrl, text: textOf(root), nodes, truncated };
}
async function resolveNode(id, expected) {
  if (!Number.isSafeInteger(id) || id <= 0) throw error('INVALID_ARGUMENTS', 'Invalid backend node identity');
  await tree(); // Refreshes DOM's current document before resolving a backend id.
  const executionContextId = await isolatedWorld(expected);
  // A page can replace DOM prototypes and element instance methods in its own
  // world. Never resolve these wrappers into that world, even for fixed source.
  const result = await send('DOM.resolveNode', { backendNodeId: id, executionContextId, objectGroup: 'willdeep-action' });
  checkEpoch(expected);
  if (!result.object.objectId) throw error('STALE_REF', 'Element is no longer in the document');
  return result.object.objectId;
}
async function callNode(objectId, functionDeclaration, args = []) {
  const result = await send('Runtime.callFunctionOn', { objectId, functionDeclaration,
    arguments: args.map(value => ({ value })), returnByValue: true, awaitPromise: false });
  if (result.exceptionDetails) throw error('UNKNOWN', 'Fixed DOM operation raised an exception');
  return result.result.value;
}
async function settle() {
  // One bounded observation wait, never a replay/retry of input or navigation.
  await delay(100);
  if (fatal) throw fatal;
  return status();
}
async function action(request) {
  if (request.action === 'init') return initialize(request);
  if (!initialized) throw error('NOT_STARTED', 'Initialize the isolated browser first');
  if (fatal) throw fatal;
  if (request.action === 'status') { keys(request, ['id','action']); return liveStatus(); }
  if (request.action === 'close') { keys(request, ['id','action']); await cleanup(); return { action: 'close', closed: true }; }
  const allowed = {
    open: ['url'], snapshot: [], fill: ['backend_node_id','text'], click: ['backend_node_id'],
    screenshot: [], assert: ['backend_node_id','assertion','expected'],
  }[request.action];
  if (!allowed) throw error('INVALID_ARGUMENTS', 'Unsupported browser action');
  keys(request, ['id','action','expected_epoch', ...allowed], ['expected_epoch']);
  checkEpoch(request.expected_epoch);
  if (request.action === 'snapshot') return snapshot(request.expected_epoch);
  if (request.action === 'open') {
    const url = checkedUrl(request.url);
    // Same-document Page.navigate replies omit loaderId. Bind them to the actual
    // document before dispatch so a later document cannot inherit their receipt.
    const { frameTree: before } = await send('Page.getFrameTree');
    checkEpoch(request.expected_epoch);
    if (!before?.frame || frameUrl(before.frame) !== pageUrl) throw error('STALE_PAGE', 'Controlled frame changed');
    mainStatus = null;
    // Observe before dispatch: commit/readiness events can precede the navigate
    // response. Its frame/loader, never the previous document's readyState,
    // identifies the navigation whose normal epoch changes this action owns.
    navigation = new Navigation();
    try {
      const result = await send('Page.navigate', { url });
      if (result.errorText) throw error('NAVIGATION_FAILED', 'Chrome rejected navigation; no automatic retry');
      const loaderId = result.loaderId ?? before.frame.loaderId;
      if (typeof loaderId !== 'string') throw error('UNKNOWN', 'Chrome omitted navigation loader identity; no automatic retry');
      await navigation.wait(result, url);
      // Synchronize DOM's document after readiness, then verify that the live
      // frame still belongs to this navigation before issuing its epoch receipt.
      await tree();
      const generation = epoch;
      const { frameTree } = await send('Page.getFrameTree');
      checkEpoch(generation);
      if (!frameTree?.frame || frameTree.frame.id !== result.frameId || frameUrl(frameTree.frame) !== pageUrl
          || frameTree.frame.loaderId !== loaderId
          || (result.loaderId === undefined && (result.frameId !== before.frame.id || pageUrl !== url))) {
        throw error('STALE_PAGE', 'Committed navigation and live page disagree');
      }
      return { ...status(), action: 'open' };
    } finally { navigation.dispose(); navigation = undefined; }
  }
  if (request.action === 'screenshot') {
    const before = epoch;
    const { data } = await send('Page.captureScreenshot', { format: 'png', fromSurface: true, captureBeyondViewport: false });
    if (typeof data !== 'string' || data.length > Math.ceil(MAX_PNG / 3) * 4) throw error('BUDGET_EXCEEDED', 'PNG exceeded 8 MiB');
    checkEpoch(before);
    return { action: 'screenshot', epoch, url: pageUrl, png: data };
  }
  if (request.action === 'assert' && request.assertion === 'url_equals') {
    if (Object.hasOwn(request, 'backend_node_id')) throw error('INVALID_ARGUMENTS', 'URL assertion does not accept an element');
    const expected = checkedUrl(request.expected);
    return { action: 'assert', epoch, assertion: 'url_equals', passed: pageUrl === expected };
  }
  const id = await resolveNode(request.backend_node_id, request.expected_epoch);
  try {
    checkEpoch(request.expected_epoch);
    if (request.action === 'assert') {
      string(request.expected);
      if (!['text_contains','text_equals'].includes(request.assertion)) throw error('INVALID_ARGUMENTS', 'Unsupported assertion');
      const text = await callNode(id, "function(){ if(!this.isConnected || this.ownerDocument !== document) return null; return (this.innerText ?? this.textContent ?? '').slice(0,32768); }");
      if (text === null) throw error('STALE_REF', 'Element is detached');
      checkEpoch(request.expected_epoch);
      return { action: 'assert', epoch, assertion: request.assertion,
        passed: request.assertion === 'text_equals' ? text === request.expected : text.includes(request.expected) };
    }
    if (request.action === 'fill') {
      string(request.text);
      // Only a fixed DOM function is used; text is a CDP value, never source code.
      const result = await callNode(id, `function(value){
        if(!this.isConnected || this.ownerDocument !== document) return false;
        const tag=this.tagName, type=(this.getAttribute('type')||'text').toLowerCase();
        if(this.disabled || this.readOnly || !this.getClientRects().length) return false;
        if(tag!=='TEXTAREA' && (tag!=='INPUT' || !['text','search','email','tel','url','number'].includes(type))) return false;
        const proto=tag==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(proto,'value').set.call(this,value);
        this.dispatchEvent(new Event('input',{bubbles:true}));
        this.dispatchEvent(new Event('change',{bubbles:true})); return true;
      }`, [request.text]);
      if (!result) throw error('ELEMENT_DENIED', 'Only visible editable text inputs/textarea can be filled; file and hidden inputs are forbidden');
    } else if (request.action === 'click') {
      const eligible = await callNode(id, "function(){ return this.isConnected && this.ownerDocument===document && !this.disabled && this.getAttribute('type')!=='file' && (this.tagName==='A'||this.tagName==='BUTTON'||this.getAttribute('role')==='button') && this.getClientRects().length>0; }");
      if (!eligible) throw error('ELEMENT_DENIED', 'Only visible enabled links/buttons can be clicked; file inputs are forbidden');
      await send('DOM.scrollIntoViewIfNeeded', { backendNodeId: request.backend_node_id });
      const { quads } = await send('DOM.getContentQuads', { backendNodeId: request.backend_node_id });
      const quad = quads[0];
      if (!quad || quad.length !== 8) throw error('ELEMENT_DENIED', 'Element has no clickable rectangle');
      const x = (quad[0]+quad[2]+quad[4]+quad[6])/4, y = (quad[1]+quad[3]+quad[5]+quad[7])/4;
      const hit = await callNode(id, 'function(x,y){ return this.contains(document.elementFromPoint(x,y)); }', [x,y]);
      if (!hit) throw error('ELEMENT_DENIED', 'Element is covered by another element');
      checkEpoch(request.expected_epoch);
      await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
      checkEpoch(request.expected_epoch);
      await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
    }
    return { ...await settle(), action: request.action };
  } finally {
    if (!fatal) await send('Runtime.releaseObjectGroup', { objectGroup: 'willdeep-action' });
  }
}

async function cleanup() {
  if (cleanupStarted) return;
  cleanupStarted = true;
  if (chrome && chrome.exitCode === null) {
    chrome.kill('SIGKILL');
    await Promise.race([new Promise(resolve => chrome.once('exit', resolve)), delay(1000)]);
  }
  cdp?.socket.close();
  // An inherited stderr writer must not keep Node alive after owned cleanup.
  chrome?.stderr?.destroy();
  chromeStderr?.stop();
  if (profile) await rm(profile, { recursive: true, force: true });
}
process.on('SIGTERM', () => { cleanup().finally(() => process.exit(0)); });
process.on('SIGINT', () => { cleanup().finally(() => process.exit(0)); });
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
try {
  for await (const line of input) {
    let request;
    try {
      if (Buffer.byteLength(line) > 64 * 1024) throw error('INVALID_ARGUMENTS', 'Bridge request exceeds its byte budget');
      request = JSON.parse(line);
      if (!Number.isSafeInteger(request.id) || request.id <= 0) throw error('INVALID_ARGUMENTS', 'Request needs a positive numeric ID');
      const result = await action(request);
      process.stdout.write(`${JSON.stringify({ id: request.id, ok: true, result })}\n`);
      if (request.action === 'close') break;
    } catch (reason) {
      // No action is replayed. Unclassified CDP failures may follow a side effect.
      const code = reason.code === 'CDP_ERROR' ? 'UNKNOWN' : (reason.code ?? 'UNKNOWN');
      process.stdout.write(`${JSON.stringify({ id: request?.id ?? null, ok: false,
        error: { code, message: reason.code ? reason.message : 'Browser bridge failed', retryable: false } })}\n`);
      if (code === 'UNKNOWN' || !initialized) break;
    }
  }
} finally { await cleanup(); }
