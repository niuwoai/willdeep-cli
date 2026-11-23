/**
 * Print the hook catalog. Run from sdk/hooks:
 *   yarn install
 *   yarn node --experimental-strip-types examples/list-hooks.ts
 */
import {
  AGENT_END_OUTCOMES,
  APPROVAL_MODES,
  APPROVER_OUTCOMES,
  HOOK_EVENTS,
  TOOL_OUTCOMES,
  canBlock,
} from "../src/index.ts";

const outcomes: Partial<Record<(typeof HOOK_EVENTS)[number], readonly string[]>> = {
  agent_end: AGENT_END_OUTCOMES,
  post_tool: TOOL_OUTCOMES,
  approval_resolved: [...APPROVER_OUTCOMES, "allow"],
};

for (const event of HOOK_EVENTS) {
  const effect = canBlock(event) ? "allow-or-deny" : "observe-only";
  const delivered = outcomes[event]?.join(", ") ?? "none";
  process.stdout.write(`${event}\t${effect}\toutcomes: ${delivered}\n`);
}
process.stdout.write(`approval modes: ${APPROVAL_MODES.join(", ")}\n`);
