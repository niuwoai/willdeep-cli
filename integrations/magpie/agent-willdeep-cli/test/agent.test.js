import test from "node:test";
import assert from "node:assert/strict";
import { agent, connect } from "../willdeep-cli.agent.js";
import { readFileSync } from "node:fs";

test("CLI detects the willdeep command without sharing the Desktop directory", () => {
  assert.equal(agent.id, "willdeep-cli");
  assert.equal(agent.name, "WillDeep CLI");
  assert.equal(agent.bin, "willdeep");
  assert.equal(agent.dir, undefined);
  assert.equal(agent.config, "~/.willdeep/integrations/magpie/cli/connection.json");
  assert.equal(agent.prefix, "");
  assert.match(agent.notice, /0\.142\.0-rc1/);
  assert.match(agent.notice, /WILLDEEP_HOME/);
  const svg = decodeURIComponent(agent.icon.slice("data:image/svg+xml,".length));
  assert.equal(svg, readFileSync(new URL("../icon.svg", import.meta.url), "utf8").trim());
});

test("CLI writes only its own public connection fields", () => {
  const output = connect({ gateway: { v1: "http://127.0.0.1:3425/v1", key: "magpie-willdeep-cli" }, model: { id: "mock/model-a", context: 8192, output: 4096 } });
  assert.deepEqual(output, { schema_version: 1, client_id: "willdeep-cli", enabled: true, base_url: "http://127.0.0.1:3425/v1", client_marker: "magpie-willdeep-cli", context_window: 8192, max_output_tokens: 4096 });
  assert.throws(() => connect({ gateway: { v1: "http://localhost:3425/v1", key: "magpie-willdeep" }, model: { id: "mock/model-a" } }));
});
