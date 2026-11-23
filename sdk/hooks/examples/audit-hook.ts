/**
 * Read one host event from stdin and enforce a narrow command gate.
 *
 *   printf '%s' '{"event":"pre_tool","tool":"run_command","detail":"rm -rf build"}' \
 *     | yarn node --experimental-strip-types examples/audit-hook.ts
 *
 * Exit 0 allows. Exit 2 denies when this program is configured as a blocking
 * pre_tool hook. The host supplies the JSON; this example does not register
 * itself.
 */
import { HookInputError, parseHookEvent } from "../src/index.ts";

try {
  let input = "";
for await (const chunk of process.stdin) input += chunk;
const event = parseHookEvent(input);
  if (event.event === "pre_tool" && event.tool === "run_command") {
    process.stderr.write(`拒绝执行命令：${event.detail}\n`);
    process.exitCode = 2;
  } else {
    process.stderr.write(`观察 ${event.event}\n`);
  }
} catch (error) {
  const message = error instanceof HookInputError ? error.message : "无法读取 hook 事件";
  process.stderr.write(`${message}\n`);
  process.exitCode = 2;
}
