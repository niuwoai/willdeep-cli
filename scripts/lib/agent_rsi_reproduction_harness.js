'use strict';
// Host-only fixed harness: inputs are data. Sources have already been selected
// from the pinned public fixture manifest by the Ruby grader.
const fs = require('node:fs');
const vm = require('node:vm');
const crypto = require('node:crypto');
const path = require('node:path');
const MANIFEST_SHA256 = '4cb96af334f1d8a7113240e8cbe7c4e275009d68233ce6cab52b67926ce05af3';
const manifestBytes = fs.readFileSync(path.join(__dirname, '../../bench/agent-rsi/reproduction-free-v1/manifest.json'));
if (crypto.createHash('sha256').update(manifestBytes).digest('hex') !== MANIFEST_SHA256) {
  throw new Error('untrusted_manifest');
}
const manifest = JSON.parse(manifestBytes.toString('utf8'));
const allowedSources = new Set(manifest.tasks.flatMap(task => [task.fixture_sha256, task.reference_sha256])
  .concat(manifest.host_negative_fixtures.map(item => item.source_sha256)));
const MAX_INPUT_BYTES = 65_536;
const MAX_OUTPUT_BYTES = 16_384;
const TIMEOUT_MS = 100;

function execute(source, expectedSha, input) {
  if (typeof source !== 'string' || !allowedSources.has(expectedSha) || crypto.createHash('sha256').update(source).digest('hex') !== expectedSha) {
    throw new Error('trusted_source_digest_mismatch');
  }
  const context = vm.createContext({input: JSON.parse(JSON.stringify(input))}, {
    codeGeneration: {strings: false, wasm: false}
  });
  const value = vm.runInContext(source + '\nJSON.stringify(reviewed(input));', context, {timeout: TIMEOUT_MS});
  if (typeof value !== 'string') throw new Error('non_json_function_result');
  return JSON.parse(value);
}

try {
  const bytes = fs.readFileSync(0);
  if (bytes.length > MAX_INPUT_BYTES) throw new Error('host_payload_too_large');
  const payload = JSON.parse(bytes.toString('utf8'));
  const result = {
    fixture_actual: execute(payload.fixture_source, payload.fixture_sha256, payload.input),
    reference_expected: execute(payload.reference_source, payload.reference_sha256, payload.input)
  };
  const output = JSON.stringify(result);
  if (Buffer.byteLength(output) > MAX_OUTPUT_BYTES) throw new Error('execution_output_too_large');
  process.stdout.write(output);
} catch (error) {
  // No source, model report, input, path, or stack is reflected in failures.
  const timeout = error && error.code === 'ERR_SCRIPT_EXECUTION_TIMEOUT';
  process.stdout.write(JSON.stringify({execution_error: timeout ? 'trusted_execution_timeout' : 'trusted_execution_failed'}));
  process.exitCode = 1;
}
