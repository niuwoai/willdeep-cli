// Persistent VT decoder for isolated TUI progress/delivery/viewport verification.
import { createInterface } from "node:readline";
import { createRequire } from "node:module";
const require = createRequire(new URL("../../web/package.json", import.meta.url));
const { Terminal } = require("@xterm/headless");
const [columns = "100", rows = "32"] = process.argv.slice(2);
const terminal = new Terminal({
  cols: Number(columns), rows: Number(rows), allowProposedApi: true,
});
const base = terminal._core.unicodeService._activeProvider;
const unicode = Object.create(base);
unicode.version = "interaction-fixture";
unicode.wcwidth = (codepoint) => codepoint === 0x1f600 ? 2 : base.wcwidth(codepoint);
terminal.unicode.register(unicode);
terminal.unicode.activeVersion = unicode.version;
for await (const line of createInterface({ input: process.stdin })) {
  try {
    const request = JSON.parse(line);
    if (request.columns && request.rows) terminal.resize(request.columns, request.rows);
    if (request.bytes) {
      await new Promise((resolve) => terminal.write(Buffer.from(request.bytes, "base64"), resolve));
    }
    const lines = [], cells = [];
    for (let y = 0; y < terminal.rows; y++) {
      const row = terminal.buffer.active.getLine(terminal.buffer.active.viewportY + y);
      lines.push(row?.translateToString(true) ?? "");
      for (let x = 0; x < terminal.cols; x++) {
        const cell = row?.getCell(x);
        if (cell) cells.push({
          x, y, text: cell.getChars(), width: cell.getWidth(),
          bg: cell.getBgColor(), fg: cell.getFgColor(), inverse: cell.isInverse(),
          bg_rgb: cell.isBgRGB(), fg_rgb: cell.isFgRGB(),
          bold: cell.isBold(), dim: cell.isDim(),
        });
      }
    }
    process.stdout.write(JSON.stringify({ lines, cells }) + "\n");
  } catch (error) {
    process.stderr.write(String(error) + "\n");
    process.exitCode = 1;
    break;
  }
}
terminal.dispose();
