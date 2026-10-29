import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

// Exercise the actual Rust-injected bridge together with the bundled page parser.
const root = new URL('../../', import.meta.url);
const rust = readFileSync(new URL('crates/willdeep-cli/src/web/configuration.rs', root), 'utf8');
const html = readFileSync(new URL('crates/willdeep-cli/assets/willdeep-config/ui/index.html', root), 'utf8');
const bridge = rust.match(/r#"<script>([\s\S]*?)<\/script>"#/)[1]
  .replaceAll('{{', '{').replaceAll('}}', '}')
  .replaceAll('{locale}', '"zh-CN"').replaceAll('{scheme}', '"light"');
const parser = html.slice(html.indexOf('function unwrapResult('), html.indexOf('function hostCapable('));

async function command(response) {
  let calls = 0;
  const context = vm.createContext({ window: {}, fetch: async (url, request) => {
    calls++;
    assert.equal(url, '/api/settings/config-command');
    assert.equal(request.method, 'POST');
    assert.deepEqual(JSON.parse(request.body), { command_id: 'config.snapshot', arguments: {} });
    return response;
  } });
  vm.runInContext(bridge + parser, context);
  const result = await vm.runInContext("callCommand('config.snapshot', {})", context);
  assert.equal(calls, 1, 'commands must not be retried automatically');
  return JSON.parse(JSON.stringify(result));
}

test('MCP snapshot is unwrapped into usable configuration sections', async () => {
  const snapshot = { ok: true, sections: [{ name: 'providers.example', fields: [] }] };
  const result = await command({ ok: true, status: 200, json: async () => ({
    content: [{ type: 'text', text: JSON.stringify(snapshot) }], isError: false,
  }) });
  assert.deepEqual(result, snapshot);
});

test('WebError retains the underlying failure instead of showing a question mark', async () => {
  const result = await command({ ok: false, status: 500, json: async () => ({
    error: 'configuration service unavailable: Ruby is required by the bundled configuration service',
  }) });
  assert.equal(result.ok, false);
  assert.match(result.message, /Ruby is required/);
  assert.match(result.message, /HTTP 500/);
});

test('HTTP authorization failures are visible even without JSON', async () => {
  const result = await command({ ok: false, status: 401, json: async () => { throw new SyntaxError('private upstream content'); } });
  assert.equal(result.ok, false);
  assert.match(result.message, /HTTP 401/);
  assert.ok(!result.message.includes('private upstream content'));
});

test('a successful proxy login page is not treated as a successful command', async () => {
  const result = await command({ ok: true, status: 200, json: async () => { throw new SyntaxError('login HTML'); } });
  assert.equal(result.ok, false);
  assert.match(result.message, /登录状态和代理服务/);
});

test('unexpected JSON is rejected with an actionable error', async () => {
  for (const payload of [null, [], {}, { status: 'ok' }]) {
    const result = await command({ ok: true, status: 200, json: async () => payload });
    assert.equal(result.ok, false);
    assert.match(result.message, /无效响应/);
  }
});

test('configuration validation errors remain unchanged', async () => {
  const failure = { ok: false, error: 'parse_error', message: 'invalid configuration' };
  const result = await command({ ok: true, status: 200, json: async () => ({
    content: [{ type: 'text', text: JSON.stringify(failure) }], isError: true,
  }) });
  assert.deepEqual(result, failure);
});
