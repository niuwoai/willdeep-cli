import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
const require = createRequire(process.env.WILLDEEP_TERMINAL_PACKAGE || new URL('../web/package.json', import.meta.url));
const { Terminal } = require('@xterm/headless');
const { frames, sent } = JSON.parse(await readFile(process.argv[2], 'utf8'));
const terminal = new Terminal({ cols: 120, rows: 35, allowProposedApi: true });
const firstSeen = new Map();
for (const frame of frames) {
  const bytes = Buffer.from(frame.data, 'base64');
  if (bytes.includes(Buffer.from('\x1b[?1049l'))) break;
  await new Promise(resolve => terminal.write(bytes, resolve));
  let text = '';
  for (let i = 0; i < terminal.buffer.active.length; i++) {
    text += terminal.buffer.active.getLine(i)?.translateToString(true) + '\n';
  }
  for (const match of text.matchAll(/FLOW\d{4}/g)) {
    if (!firstSeen.has(match[0])) firstSeen.set(match[0], frame.at);
  }
}
const samples = sent.map(event => ({ marker: event.marker, delay_ms: firstSeen.has(event.marker) ? (firstSeen.get(event.marker) - event.at) * 1000 : null }));
const delays = samples.map(s => s.delay_ms).filter(t => t !== null).sort((a, b) => a - b);
const displayTimes = [...new Set(firstSeen.values())].sort((a, b) => a - b);
const gaps = displayTimes.slice(1).map((time, i) => (time - displayTimes[i]) * 1000);
process.stdout.write(JSON.stringify({ events: sent.length, displayed: delays.length,
  median_ms: delays[Math.floor(delays.length * .5)], p95_ms: delays[Math.floor(delays.length * .95)],
  max_ms: delays.at(-1), max_display_gap_ms: Math.max(0, ...gaps),
  display_updates: displayTimes.length, samples }));
terminal.dispose();
