// 配置页的纯逻辑：常用值一键填入、默认模型跟着默认提供商走。
// 页面脚本从 index.html 里原样抽出，启动尾部不带进来，所以这里不会自己发请求。

import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { JSDOM } from '../../web/node_modules/jsdom/lib/api.js';

const require = createRequire(import.meta.url);
const { window } = new JSDOM('<!doctype html><body><main id="main"></main><div id="savebar"></div>' +
  '<div id="toasts"></div><div id="dialog-root"></div><nav id="toc"></nav><div id="banner"></div>' +
  '<div id="meta"></div><div id="toolbar"></div><input id="search"></body>');
globalThis.window = window;
globalThis.document = window.document;

const source = readFileSync(new URL('./fixtures/config_page.js', import.meta.url), 'utf8');
window.eval(source + '\nwindow.__wd = { parseInput, commitValue, renderControl, applyPreset, currentDefaultProvider, edits, S };');

const { parseInput, commitValue, renderControl, applyPreset, currentDefaultProvider, edits, S } = window.__wd;

let failed = 0;
function check(label, condition) {
  if (condition) return;
  failed += 1;
  console.error('FAIL ' + label);
}

const parsed = parseInput('int', '1048576');
check('1M parses as an integer', parsed.value === 1048576);
check('blank clears an int', parseInput('int', '  ').clear === true);
check('words are rejected', !!parseInput('int', '256k').error);

const original = { present: true, value: 32768, secret: false, plaintext: null };
commitValue('providers.magpie', 'max_output_tokens', 'int', '1048576', original);
const edit = edits.get('providers.magpie\u0000max_output_tokens');
check('preset commits the exact number', edit && edit.op === 'set' && edit.value === 1048576);
commitValue('providers.magpie', 'max_output_tokens', 'int', '32768', original);
check('returning to the saved value drops the edit', !edits.has('providers.magpie\u0000max_output_tokens'));

const field = {
  type: 'int', key: 'context_window', hasValue: false, value: null,
  presets: [{ label: '32K', value: 32768 }, { label: '1M', value: 1048576 }],
};
const html = renderControl({ name: 'providers.magpie', groupID: 'providers', fields: [] }, field, 'k', 'id1', '');
check('chips show the labels', html.includes('>32K<') && html.includes('>1M<'));
check('chips carry the exact values', html.includes('data-value="32768"') && html.includes('data-value="1048576"'));

const row = document.createElement('div');
row.className = 'row';
row.setAttribute('data-section', 'providers.magpie');
row.setAttribute('data-key', 'max_output_tokens');
row.innerHTML = '<input data-input value="">';
document.body.appendChild(row);
S.fields.set('providers.magpie\u0000max_output_tokens', {
  section: { name: 'providers.magpie', groupID: 'providers', fields: [field], kind: 'collection' },
  field: Object.assign({}, field, { key: 'max_output_tokens' }),
});
applyPreset(row, '131072');
const stored = edits.get('providers.magpie\u0000max_output_tokens');
check('clicking a chip fills the number', stored && stored.value === 131072);

S.sections.set('', {
  name: '', fields: [{ key: 'default_provider', type: 'string', hasValue: true, value: 'magpie' }],
});
check('default model follows the saved provider', currentDefaultProvider() === 'magpie');

if (failed) {
  console.error(failed + ' page check(s) failed');
  process.exit(1);
}
console.log('config page checks passed');
