import test from "node:test";
import assert from "node:assert/strict";
import { formatter } from "../src/formatter.ts";
import { formatLabel, showLabel } from "../src/view.ts";
import { audit } from "../src/audit.ts";

test("the target property migrates, with no compatibility shim", () => {
  assert.deepEqual(Object.keys(formatter), ["renderLabel"]);
  assert.equal("formatLabel" in formatter, false);
  assert.deepEqual(audit, { formatLabel: "legacy-audit-field" });
});

test("all three reference forms keep working for varied values", () => {
  const values = ["", " Ada ", "quote", "formatLabel", " x-Y_9 ", "中文 标签", "\tline\n"];
  let seed = 0x5a17;
  for (let index = 0; index < 96; index += 1) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    values.push(` item-${seed.toString(36)}-${index} `);
  }
  for (const value of values) {
    const expected = value.trim().toUpperCase();
    assert.equal(formatter.renderLabel(value), expected);
    assert.equal(formatLabel(value), `[${value}]`);
    assert.deepEqual(showLabel(value), {
      direct: expected,
      keyed: expected,
      extracted: expected,
      local: `[${value}]`,
      literal: "formatLabel",
      audit: "legacy-audit-field",
    });
  }
});
