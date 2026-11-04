import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { agent as desktop, connect as connectDesktop } from "../agent-willdeep/willdeep.agent.js";
import { agent as cli, connect as connectCLI } from "../agent-willdeep-cli/willdeep-cli.agent.js";

const bundles = [
  { agent: desktop, connect: connectDesktop, directory: "agent-willdeep" },
  { agent: cli, connect: connectCLI, directory: "agent-willdeep-cli" }
];
const ownMarker = (agent) => "magpie-" + agent.id;
const validURLs = ["http://localhost/v1", "https://localhost:65535/v1", "http://127.0.0.1:1/v1", "http://127.255.255.255:4567/v1", "https://[::1]/v1", "http://[::1]:3425/v1"];
const invalidURLs = [undefined, null, 3425, {}, "", "http://localhost/v1\n", "HTTP://localhost/v1", "http://LOCALHOST/v1", "http://localhost./v1", "http://localhost.evil/v1", "http://127.1/v1", "http://127.0.1/v1", "http://2130706433/v1", "http://0x7f000001/v1", "http://127.00.0.1/v1", "http://127.0.0.256/v1", "http://128.0.0.1/v1", "http://0.0.0.0/v1", "http://192.168.1.2/v1", "https://example.com/v1", "http://[::ffff:127.0.0.1]/v1", "http://[0:0:0:0:0:0:0:1]/v1", "http://::1/v1", "http://localhost:0/v1", "http://localhost:65536/v1", "http://localhost:/v1", "http://localhost:-1/v1", "http://localhost:1.0/v1", "http://localhost:100000/v1", "http://user@localhost/v1", "http://user:password@localhost/v1", "http://localhost/v1/", "http://localhost/v1?", "http://localhost/v1#", "http://localhost/v1?x=1", "http://localhost/v1#x", "http://localhost/a/../v1", "http://localhost/%76%31", "http://localhost/v1/../v1", "http://localhost\\v1", " http://localhost/v1", "http://localhost/v1 ", "http://localhost\n/v1", "file://localhost/v1"];

for (const { agent, connect, directory } of bundles) {
  const input = (model = {}, gateway = {}) => ({ gateway: { v1: validURLs[0], key: ownMarker(agent), ...gateway }, model: { id: "vendor/nested/model-a", ...model } });

  test(`${agent.id}: only supported loopback URL spellings connect`, () => {
    for (const v1 of validURLs) assert.equal(connect(input({}, { v1 })).base_url, v1);
    for (const v1 of invalidURLs) assert.throws(() => connect(input({}, { v1 })), /gateway\.v1/, String(v1));
  });

  test(`${agent.id}: exact public marker, no secrets in errors`, () => {
    for (const key of [undefined, null, false, 1, {}, [], "magpie", "magpie-other", ownMarker(agent) + " ", ownMarker(agent) + "\n", "test-private-value-not-a-key"]) {
      assert.throws(() => connect(input({}, { key })), (err) => {
        assert.match(err.message, /gateway\.key/);
        assert.ok(!err.message.includes("test-private-value-not-a-key"));
        assert.ok(!err.message.includes("password"));
        return true;
      });
    }
  });

  test(`${agent.id}: valid complete model IDs remain untouched by the host field`, () => {
    for (const id of ["vendor/nested/model-a", "路由/模型", "模型/🪶", "m".repeat(1024), "🪶".repeat(256), "模".repeat(341) + "m"]) {
      const value = input({ id });
      const before = structuredClone(value);
      const output = connect(value);
      assert.ok(!Object.hasOwn(output, "model"));
      assert.equal(agent.model, "model");
      assert.equal(agent.prefix, "");
      assert.equal(agent.prefix + value.model.id, id);
      assert.deepEqual(value, before);
    }
    for (const id of [undefined, null, false, 1, {}, [], "", " ", "\t", " ", "m".repeat(1025), "🪶".repeat(257), "模".repeat(342), "x\ud800", "x\udc00"]) {
      assert.throws(() => connect(input({ id })), /model\.id/);
    }
    for (let c = 0; c <= 0x9f; c++) {
      if (c <= 0x1f || c >= 0x7f) assert.throws(() => connect(input({ id: "vendor/" + String.fromCharCode(c) })), /model\.id/);
    }
  });

  test(`${agent.id}: metadata preserves valid values and represents unknowns as null`, () => {
    for (const context of [4000, 8192, 1048576]) assert.equal(connect(input({ context })).context_window, context);
    for (const output of [1, 4096, 1048576]) assert.equal(connect(input({ output })).max_output_tokens, output);
    for (const context of [undefined, null, 0, -1, 3999, 1048577, 4000.5, NaN, Infinity, "8192", true, [], {}]) assert.equal(connect(input({ context })).context_window, null);
    for (const output of [undefined, null, 0, -1, 1048577, 1.5, NaN, Infinity, "4096", true, [], {}]) assert.equal(connect(input({ output })).max_output_tokens, null);
    assert.equal(connect(input({ context: 8192, output: 8193 })).max_output_tokens, null);
    assert.equal(connect(input({ context: 8192, output: 8192 })).max_output_tokens, 8192);
    assert.equal(connect(input({ context: 3999, output: 4096 })).max_output_tokens, 4096);
  });

  test(`${agent.id}: return exactly the seven non-model fields`, () => {
    assert.deepEqual(Object.keys(connect(input())).sort(), ["base_url", "client_id", "client_marker", "context_window", "enabled", "max_output_tokens", "schema_version"]);
    assert.equal(connect(input()).client_id, agent.id);
    assert.equal(connect(input()).client_marker, ownMarker(agent));
    for (const value of [null, undefined, [], {}, { gateway: null }, { gateway: [] }, { gateway: { v1: validURLs[0], key: ownMarker(agent) }, model: [] }]) assert.throws(() => connect(value));
  });

  test(`${agent.id}: local package is self-contained and has no installation hooks`, () => {
    const root = new URL(`../${directory}/`, import.meta.url);
    const pkg = JSON.parse(readFileSync(new URL("package.json", root), "utf8"));
    assert.equal(pkg.private, true);
    assert.ok(!pkg.name.startsWith("@magpie-community/"));
    assert.equal(pkg.magpie.agent, agent.id === "willdeep" ? "./willdeep.agent.js" : "./willdeep-cli.agent.js");
    assert.equal(pkg.main, undefined);
    assert.equal(pkg.exports, undefined);
    assert.equal(pkg.dependencies, undefined);
    assert.deepEqual(Object.keys(pkg.scripts), ["test"]);
    for (const file of pkg.files) {
      const source = readFileSync(new URL(file, root), "utf8");
      if (!file.endsWith(".js") || file.startsWith("test/")) continue;
      assert.doesNotMatch(source, /\b(?:require|fetch|XMLHttpRequest|WebSocket|eval)\s*\(|\b(?:process|Bun|Deno)\./);
      for (const match of source.matchAll(/\bfrom\s+["']([^"']+)["']/g)) {
        assert.match(match[1], /^\.\//);
        assert.ok(pkg.files.includes(match[1].slice(2)));
      }
    }
  });
}

test("community entry drafts preserve independent identity, icons and bilingual summaries", () => {
  const entries = JSON.parse(readFileSync(new URL("../community/registry.entries.draft.json", import.meta.url), "utf8"));
  assert.equal(entries.length, bundles.length);
  for (const { agent, directory } of bundles) {
    const pkg = JSON.parse(readFileSync(new URL(`../${directory}/package.json`, import.meta.url), "utf8"));
    const entry = entries.find((item) => item.package === pkg.name);
    assert.equal(entry.name, agent.name);
    assert.equal(entry.kind, "agent");
    assert.equal(entry.community, undefined);
    assert.equal(entry.icon, agent.icon);
    assert.ok(entry.summary.en && entry.summary.zh);
    const tooling = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    assert.equal(pkg.version, tooling.version);
    assert.ok(readFileSync(new URL(`../${directory}/CHANGELOG.md`, import.meta.url), "utf8").includes(`## ${pkg.version} ·`));
    assert.ok(readFileSync(new URL(`../${directory}/README.md`, import.meta.url), "utf8").includes(`**${pkg.version}**`));
    assert.ok(agent.notice.includes(pkg.willdeepCompatibility.minimumVersion));
  }
});

test("independent package validation stays byte-identical", () => {
  assert.equal(readFileSync(new URL("../agent-willdeep/contract.js", import.meta.url), "utf8"), readFileSync(new URL("../agent-willdeep-cli/contract.js", import.meta.url), "utf8"));
});

test("connected shared fixtures project to the same non-model connection fields", () => {
  // contents remains raw in the fixture file; this test only projects connected objects.
  // Duplicate-key/file-reader rejection belongs to the Rust/Swift consumers.
  const fixtures = JSON.parse(readFileSync(new URL("../../../docs/fixtures/magpie-agent-v1/cases.json", import.meta.url), "utf8"));
  assert.equal(fixtures.desktop_provider_uuid, "70D7B194-6038-4B32-8195-84F25E1648A3");
  assert.equal(fixtures.cli_profile_id, "magpie-cli");
  for (const item of fixtures.cases.filter((c) => c.expected === "connected")) {
    const parsed = JSON.parse(item.contents);
    const { connect } = bundles.find((b) => b.agent.id === item.client_id);
    assert.deepEqual(connect({ gateway: { v1: parsed.base_url, key: parsed.client_marker }, model: { id: parsed.model, context: parsed.context_window, output: parsed.max_output_tokens } }), {
      schema_version: parsed.schema_version, client_id: parsed.client_id, enabled: parsed.enabled,
      base_url: parsed.base_url, client_marker: parsed.client_marker,
      context_window: parsed.context_window ?? null, max_output_tokens: parsed.max_output_tokens ?? null
    }, item.id);
  }
});
