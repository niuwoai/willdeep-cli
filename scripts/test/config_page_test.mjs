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
window.eval(source + '\nwindow.__wd = { parseInput, commitValue, renderControl, applyPreset, currentDefaultProvider, renderLibraries, setRuleEnabled, edits, S, Lib };');

const { parseInput, commitValue, renderControl, applyPreset, currentDefaultProvider, renderLibraries, setRuleEnabled, edits, S, Lib } = window.__wd;

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

// 区间跟着字段走：128 回合那次故障就是页面不看区间，存下了运行时拒绝的值。
parseInput.field = { min: 1, max: 1000 };
check('a turn count inside the range is accepted', parseInput('int', '128').value === 128);
check('a turn count past the range is rejected here', !!parseInput('int', '1001').error);
check('the rejection names both bounds', parseInput('int', '1001').error.includes('1000'));
parseInput.field = null;
check('a field without bounds still accepts any safe integer', parseInput('int', '1001').value === 1001);

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

S.snapshot = { sections: [], collections: [], unknownKeys: [] };
S.sections.set('rules', {
  name: 'rules', groupID: 'rules', kind: 'static', editable: true,
  fields: [{ key: 'files', type: 'stringArray', hasValue: false, value: [] }],
});
Lib.skillsLoading = false;
Lib.skills = {
  ok: true,
  skills: [
    { identifier: 'demo', name: 'Demo', description: '示例技能', tier: 'standard', directory: '/home/skills/demo', editable: true },
    { identifier: 'shared', name: 'Shared <b>', description: '只读', tier: null, directory: '/home/.agents/skills/shared', editable: false },
  ],
};
Lib.rulesLoading = false;
Lib.rules = {
  ok: true, configured: [], rulesDir: '/home/rules',
  entries: [
    { name: 'agentrules.md', path: '/home/rules/agentrules.md', kind: 'file', bytes: 1024, enabled: false },
    { name: 'team', path: '/home/rules/team', kind: 'directory', bytes: null, enabled: false },
  ],
};
document.getElementById('main').innerHTML = '<div id="libraries"></div>';
setRuleEnabled('/home/rules/agentrules.md', true);
const libraries = renderLibraries();
check('editable skill offers edit and delete', libraries.includes('data-act="skill-edit"') && libraries.includes('data-act="skill-remove"'));
check('a read-only skill offers view only', libraries.includes('data-act="skill-view"'));
check('skill text is escaped', libraries.includes('Shared &lt;b&gt;') && !libraries.includes('Shared <b>'));
check('enabling a rule writes the rule-file list', edits.get('rules\u0000files').value.join() === '/home/rules/agentrules.md');
check('an enabled rule is marked and can move', libraries.includes('data-act="rule-move"') && libraries.includes('data-act="rule-disable"'));
const directoryRow = libraries.slice(libraries.indexOf('/home/rules/team'));
check('a directory rule explains itself and cannot be edited as a file',
  directoryRow.includes('data-act="rule-enable"') && !directoryRow.includes('rule-edit'));

if (failed) {
  console.error(failed + ' page check(s) failed');
  process.exit(1);
}
console.log('config page checks passed');
