import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
const require = createRequire(new URL("../../../web/package.json", import.meta.url));
const { Terminal } = require("@xterm/headless");
const terminal = new Terminal({ cols: 88, rows: 32, allowProposedApi: true });
const matched = Object.fromEntries(["accounted_tokens", "max_concurrent", "run_count", "blocked"].map((key) => [key, false]));
let screen = "";
for (const chunk of JSON.parse(await readFile(process.argv[2], "utf8"))) {
  await new Promise((resolve) => terminal.write(Buffer.from(chunk, "base64"), resolve));
  screen = Array.from({ length: terminal.buffer.active.length }, (_, index) => terminal.buffer.active.getLine(index)?.translateToString(true) ?? "").join("\n");
  for (const key of Object.keys(matched)) matched[key] ||= screen.includes(key);
}
terminal.dispose();
process.stdout.write(JSON.stringify({ matched, screen }));
