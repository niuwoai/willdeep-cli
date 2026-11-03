// 从实际页面抽出脚本，避免生成夹具与发布页面不一致；去掉启动尾部，由测试驱动宿主桥。
import { readFileSync } from 'node:fs';
import { JSDOM } from '../../web/node_modules/jsdom/lib/api.js';

const { window } = new JSDOM('<!doctype html><body><main id="main"></main><div id="savebar"></div>' +
  '<div id="toasts"></div><div id="dialog-root"></div><nav id="toc"></nav><div id="banner"></div>' +
  '<div id="meta"></div><div id="toolbar"></div><input id="search"></body>', { runScripts: 'outside-only' });
const { document } = window;
const page = readFileSync(new URL('../../crates/willdeep-cli/assets/willdeep-config/ui/index.html', import.meta.url), 'utf8');
const script = page.match(/<script>([\s\S]*?)<\/script>/)[1];
const startup = script.lastIndexOf("\nif (typeof document !== 'undefined' && document.readyState");
if (startup < 0) throw new Error('Config page startup boundary not found');
window.eval(script.slice(0, startup) + '\nwindow.__wd = { parseInput, commitValue, renderControl, applyPreset, currentDefaultProvider, renderLibraries, setRuleEnabled, edits, S, Lib, renderAll, afterRowChange, loadModels, buildEdits, onInput, onChange, onClick, modelProviderSection };');

const { parseInput, commitValue, renderControl, applyPreset, currentDefaultProvider, renderLibraries, setRuleEnabled, edits, S, Lib,
  renderAll, afterRowChange, loadModels, buildEdits, onInput, onChange, onClick, modelProviderSection } = window.__wd;

let failed = 0;
let checked = 0;
function check(label, condition) {
  checked += 1;
  if (condition) return;
  failed += 1;
  console.error('FAIL ' + label);
}
window.addEventListener('error', event => check('page has no uncaught browser errors: ' + event.message, false));

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

const versionField = { type: 'int', key: 'version', hasValue: false, value: null, options: [1], optionLabels: { 1: '1' } };
const versionHtml = renderControl({ name: '', groupID: 'root', fields: [] }, versionField, 'version', 'idv', '');
check('a known-values int renders as a select', versionHtml.indexOf('<select') >= 0);
check('the known value is offered', versionHtml.indexOf('value="1"') >= 0);
commitValue('', 'version', 'int', '1', { present: false, value: null, secret: false }, versionField);
const versionEdit = edits.get('\u0000version');
check('the picked version is committed as an integer', versionEdit && versionEdit.value === 1);

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

// 只模拟宿主 config.models；页面仍走真实渲染、事件与编辑记录。
const requests = [];
window.willdeep = {
  executeCommand(command, args) {
    check('only the existing models command is used', command === 'config.models');
    return new Promise(resolve => requests.push({ args, resolve }));
  },
};
for (const [name, listener] of [['input', onInput], ['change', onChange], ['click', onClick]]) {
  document.addEventListener(name, listener);
}
const stringField = (key, value = null) => ({ key, type: 'string', value, hasValue: value !== null });
const section = (name, groupID, fields) => ({ name, groupID, fields, editable: true,
  kind: ['providers', 'subagents', 'worker_tiers'].includes(groupID) ? 'collection' : 'static',
  instance: name.includes('.') ? name.split('.').slice(1).join('.') : undefined });
const sections = [
  section('', 'root', [stringField('default_provider', 'some-im'), stringField('default_model', 'private-default')]),
  section('providers.some-im', 'providers', [stringField('api_base', 'https://one.example/v1'),
    stringField('model', 'private-default'), stringField('vision_model', 'private-vision')]),
  section('providers.other', 'providers', [stringField('api_base', 'https://two.example/v1'), stringField('model', 'other-model')]),
  section('subagents.tester', 'subagents', [stringField('provider_profile', 'some-im'), stringField('model', 'private-tester')]),
  section('subagents.reviewer', 'subagents', [stringField('provider_profile'), stringField('model')]),
  section('worker_tiers.advanced', 'worker_tiers', [stringField('provider_profile'), stringField('model')]),
  section('agent', 'agent', [stringField('compressor_profile', 'other'), stringField('compressor_model')]),
  section('local_model', 'local_model', [stringField('summary_model', 'local-summary')]),
];
S.snapshot = { sections, collections: ['providers', 'subagents', 'worker_tiers'].map(id => ({ id })), unknownKeys: [] };
S.added.clear();
S.removed.clear();
S.models.clear();
edits.clear();
renderAll();
const modelRow = (name, key = 'model') => document.querySelector('.row[data-section="' + name + '"][data-key="' + key + '"]');
const options = (name, key = 'model') => Array.from(modelRow(name, key).querySelector('select').options, option => option.value);
const settle = () => new Promise(resolve => setImmediate(resolve));
function change(name, key, value) {
  const control = modelRow(name, key).querySelector('[data-input]');
  control.value = value;
  control.dispatchEvent(new window.Event('change', { bubbles: true }));
}
function editField(name, key, value) {
  const field = S.fields.get(name + '\u0000' + key).field;
  commitValue(name, key, field.type, value, { present: field.hasValue, value: field.value, secret: false });
  afterRowChange(name, key);
}
check('models load automatically and share provider requests', requests.length === 2);
check('tester shows a model select while loading', !!modelRow('subagents.tester').querySelector('select'));
check('tester loading state is visible', modelRow('subagents.tester').textContent.includes('正在加载'));
requests.find(request => request.args.section === 'providers.some-im').resolve({ ok: true, models: ['listed', 'vision-listed'] });
requests.find(request => request.args.section === 'providers.other').resolve({ ok: true, models: ['other-model'] });
await settle();
check('tester uses its explicitly selected provider', options('subagents.tester').includes('listed'));
check('unlisted saved model remains selected', modelRow('subagents.tester').querySelector('select').value === 'private-tester');
check('worker tier without a profile uses the default provider', options('worker_tiers.advanced').includes('listed'));
check('subagent without a profile uses the default provider', options('subagents.reviewer').includes('listed'));
check('compressor uses its own profile', options('agent', 'compressor_model').includes('other-model') &&
  !options('agent', 'compressor_model').includes('listed'));
check('vision model uses the containing provider', options('providers.some-im', 'vision_model').includes('vision-listed'));
check('default model refreshes with provider candidates', options('', 'default_model').includes('listed'));
check('local summary keeps its own address and input', !!modelRow('local_model', 'summary_model').querySelector('input'));
check('loading and displaying candidates do not create edits', buildEdits().length === 0);
check('empty model remains empty rather than selecting a candidate', modelRow('worker_tiers.advanced').querySelector('select').value === '');
change('subagents.tester', 'model', 'listed');
change('subagents.tester', 'provider_profile', 'other');
check('switching the provider changes candidates', options('subagents.tester').includes('other-model') &&
  !options('subagents.tester').includes('vision-listed'));
check('switching provider preserves a dirty model', modelRow('subagents.tester').querySelector('select').value === 'listed' &&
  edits.get('subagents.tester\u0000model').value === 'listed');
change('', 'default_provider', 'other');
check('default provider changes inherited worker candidates', options('worker_tiers.advanced').includes('other-model') &&
  !options('worker_tiers.advanced').includes('listed'));
change('worker_tiers.advanced', 'provider_profile', 'some-im');
check('worker profile overrides the default', options('worker_tiers.advanced').includes('listed'));
change('worker_tiers.advanced', 'provider_profile', '');
check('clearing worker profile restores provider inheritance', options('worker_tiers.advanced').includes('other-model'));
change('agent', 'compressor_profile', 'some-im');
check('compressor profile change refreshes its model candidates', options('agent', 'compressor_model').includes('listed'));
change('agent', 'compressor_profile', '');
check('clearing compressor profile uses the default provider', options('agent', 'compressor_model').includes('other-model'));
change('subagents.tester', 'model', '');
check('clearing a saved model keeps the original unset semantics', edits.get('subagents.tester\u0000model').op === 'unset');
check('a cleared model is still blank after rerender', modelRow('subagents.tester').querySelector('select').value === '');

// 端点变更使旧请求失效；新回包先完成时，旧回包不能覆盖缓存及依赖行。
const oldLoad = loadModels('providers.other');
const stale = requests.at(-1);
editField('providers.other', 'api_base', 'https://new.example/v1');
const fresh = requests.at(-1);
check('endpoint change starts a replacement request', fresh !== stale && fresh.args.api_base === 'https://new.example/v1');
fresh.resolve({ ok: true, models: ['fresh-model'] });
await settle();
stale.resolve({ ok: true, models: ['stale-model'] });
await oldLoad;
check('stale responses cannot replace the current provider cache', S.models.get('providers.other').models.join() === 'fresh-model');
check('stale responses cannot replace dependent candidates', options('subagents.tester').includes('fresh-model') &&
  !options('subagents.tester').includes('stale-model'));

const switchedLoad = loadModels('providers.other');
change('subagents.tester', 'provider_profile', 'some-im');
requests.at(-1).resolve({ ok: true, models: ['late-other-model'] });
await switchedLoad;
check('a response for the previous provider cannot change the current list', options('subagents.tester').includes('listed') &&
  !options('subagents.tester').includes('late-other-model'));
change('subagents.tester', 'provider_profile', 'other');

// 手输每次 input 都记编辑，异步列表刷新不会丢失尚未失焦的自定义模型和光标。
modelRow('subagents.tester').querySelector('[data-act="toggle-model-input"]').click();
let manual = modelRow('subagents.tester').querySelector('[data-model-manual]');
manual.focus();
manual.value = 'hidden-virtual-model';
manual.setSelectionRange(6, 6);
manual.dispatchEvent(new window.Event('input', { bubbles: true }));
const failingLoad = loadModels('providers.other');
requests.at(-1).resolve({ ok: false, message: 'upstream unavailable' });
await failingLoad;
manual = modelRow('subagents.tester').querySelector('[data-model-manual]');
check('failed lookup keeps an active custom-model edit', manual.value === 'hidden-virtual-model' &&
  edits.get('subagents.tester\u0000model').value === 'hidden-virtual-model');
check('async refresh preserves manual focus and cursor', document.activeElement === manual && manual.selectionStart === 6);
check('lookup failure is explicit', modelRow('subagents.tester').textContent.includes('upstream unavailable'));
modelRow('subagents.tester').querySelector('[data-act="toggle-model-input"]').click();
check('custom model remains selectable after returning to the list', modelRow('subagents.tester').querySelector('select').value === 'hidden-virtual-model');
const emptyLoad = loadModels('providers.other');
requests.at(-1).resolve({ ok: true, models: [] });
await emptyLoad;
check('an empty response is explicit and has a manual fallback', modelRow('subagents.tester').textContent.includes('接口未返回可用模型') &&
  !modelRow('subagents.tester').querySelector('[data-act="toggle-model-input"]').disabled);
S.removed.add('providers.other');
afterRowChange('', 'default_provider');
check('removed providers do not leave cached candidates on dependents', !options('subagents.tester').includes('fresh-model') &&
  modelRow('subagents.tester').textContent.includes('提供商不存在'));
S.removed.clear();
const tester = sections.find(sec => sec.name === 'subagents.tester');
tester.editable = false;
renderAll();
check('read-only model controls and actions stay disabled', Array.from(modelRow('subagents.tester').querySelectorAll('select, button')).every(control => control.disabled));
check('unknown model names outside supported settings remain ordinary fields', modelProviderSection({ groupID: 'other' }, stringField('model')) === null);

if (failed) {
  console.error(failed + ' page check(s) failed');
  process.exit(1);
}
console.log('config page checks passed (' + checked + ' checks)');
window.close();
