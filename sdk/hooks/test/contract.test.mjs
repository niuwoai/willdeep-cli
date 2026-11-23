import assert from "node:assert/strict";
import test from "node:test";

const sdk = await import("../src/index.ts");

test("catalog matches the five events delivered by the host", () => {
  assert.deepEqual(sdk.HOOK_EVENTS, [
    "before_agent",
    "agent_end",
    "pre_tool",
    "post_tool",
    "approval_resolved",
  ]);
  assert.deepEqual(
    sdk.HOOK_EVENTS.filter((event) => sdk.canBlock(event)),
    ["before_agent", "pre_tool"],
  );
});

test("agent and tool events preserve the fields the host emits", () => {
  const before = sdk.parseHookEvent(
    '{"event":"before_agent","session_id":"s","workspace":"/work","tool":null,"detail":null,"outcome":null,"exit_code":null}\n',
  );
  assert.equal(before.event, "before_agent");
  assert.equal(before.outcome, null);
  assert.equal(before.tool, null);

  const ended = sdk.parseHookEvent(
    '{"event":"agent_end","session_id":null,"workspace":null,"tool":null,"detail":null,"outcome":"budget_limited","exit_code":null}',
  );
  assert.equal(ended.outcome, "budget_limited");

  const tool = sdk.parseHookEvent(
    '{"event":"post_tool","session_id":null,"workspace":null,"tool":"read_file","detail":"src/lib.rs","outcome":"error","exit_code":null}',
  );
  assert.equal(tool.outcome, "error");
});

test("approval sources keep their distinct outcomes", () => {
  const judge = sdk.parseHookEvent(JSON.stringify({
    event: "approval_resolved",
    tool: "run_command",
    detail: "cargo test",
    session_id: null,
    workspace: null,
    outcome: "allow",
    exit_code: null,
    extra: { source: "judge", mode: "smart" },
  }));
  assert.equal(judge.extra.source, "judge");

  assert.throws(
    () => sdk.parseHookEvent(JSON.stringify({
      event: "approval_resolved",
      tool: "run_command",
      detail: "cargo test",
      session_id: null,
      workspace: null,
      outcome: "allow",
      exit_code: null,
      extra: { source: "approver", mode: "smart" },
    })),
    sdk.HookInputError,
  );
});

test("invalid names, outcomes, and blocking combinations are rejected", () => {
  assert.throws(() => sdk.parseHookEvent('{"event":"session_start"}'), sdk.HookInputError);
  assert.throws(
    () => sdk.parseHookEvent('{"event":"agent_end","outcome":"success"}'),
    sdk.HookInputError,
  );
  assert.deepEqual(
    sdk.validateHookConfig({
      event: "post_tool",
      command: "logger",
      blocking: true,
    }),
    ["post_tool happens after the action and cannot block"],
  );
  assert.deepEqual(
    sdk.validateHookConfig({ event: "pre_tool", command: "  " }),
    ["command must not be empty"],
  );
});

test("truncated frames remain explicit and valid JSON is not repaired", () => {
  const truncated = sdk.parseHookEvent(JSON.stringify({
    event: "pre_tool",
    session_id: null,
    workspace: null,
    tool: "run_command",
    detail: "cargo test",
    outcome: null,
    exit_code: null,
    extra: { truncated: "true" },
  }));
  assert.equal(truncated.extra.truncated, "true");
  assert.throws(() => sdk.parseHookEvent('{"event":"pre_tool"'), sdk.HookInputError);
});
