// Persistent VT decoder for the isolated Enter-feedback PTY regression.
// Each request contains bytes through a completed synchronized-update frame.
import { createInterface } from "node:readline";
import { createRequire } from "node:module";
const require = createRequire(new URL("../../web/package.json", import.meta.url));
const { Terminal } = require("@xterm/headless");
const [cols = "120", rows = "36"] = process.argv.slice(2);
const terminal = new Terminal({ cols: Number(cols), rows: Number(rows), allowProposedApi: true });
const input = createInterface({ input: process.stdin });
for await (const line of input) {
  try {
    const request = JSON.parse(line);
    await new Promise((resolve) => terminal.write(Buffer.from(request.bytes, "base64"), resolve));
    const lines = Array.from({ length: Number(rows) }, (_, index) =>
      terminal.buffer.active.getLine(terminal.buffer.active.viewportY + index)?.translateToString(true) ?? "");
    process.stdout.write(JSON.stringify({ lines, text: lines.join("\n") }) + "\n");
  } catch (error) {
    process.stderr.write(String(error) + "\n");
    process.exitCode = 1;
    break;
  }
}
terminal.dispose();
