import test from "node:test";
import assert from "node:assert/strict";
import { agent, connect } from "../willdeep.agent.js";
import { readFileSync } from "node:fs";

test("Desktop uses its own support directory without a launcher", () => {
  assert.equal(agent.id, "willdeep");
  assert.equal(agent.name, "WillDeep");
  assert.equal(agent.dir, "~/.willdeep/integrations/magpie/desktop");
  assert.equal(agent.bin, undefined);
  assert.equal(agent.config, agent.dir + "/connection.json");
  assert.equal(agent.prefix, "");
  assert.match(agent.notice, /1\.427\.0-rc1/);
  assert.match(agent.notice, /新会话/);
  const svg = decodeURIComponent(agent.icon.slice("data:image/svg+xml,".length));
  assert.equal(svg, readFileSync(new URL("../icon.svg", import.meta.url), "utf8").trim());
});

test("Desktop writes only its own public connection fields", () => {
  const output = connect({ gateway: { v1: "http://localhost:3425/v1", key: "magpie-willdeep" }, model: { id: "mock/model-a" } });
  assert.deepEqual(output, { schema_version: 1, client_id: "willdeep", enabled: true, base_url: "http://localhost:3425/v1", client_marker: "magpie-willdeep", context_window: null, max_output_tokens: null });
  assert.throws(() => connect({ gateway: { v1: "http://localhost:3425/v1", key: "magpie-willdeep-cli" }, model: { id: "mock/model-a" } }));
});
