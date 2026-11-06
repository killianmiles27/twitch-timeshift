import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

function deferred() {
  let resolve, reject;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return {promise, resolve, reject};
}
function harness(overrides = {}) {
  const elements = new Map();
  const element = () => ({hidden: false, value: '', textContent: '', innerHTML: '',
    dataset: {}, style: {}, open: false, disabled: false,
    classList: {add() {}, remove() {}, toggle() {}},
    addEventListener() {}, setAttribute() {}, querySelector() {return null;},
    querySelectorAll() {return [];}, focus() {}, showModal() {this.open = true;}});
  const document = {getElementById(id) {if (!elements.has(id)) elements.set(id, element()); return elements.get(id);},
    querySelector() {return null;}, activeElement: null, addEventListener() {}};
  const config = {streamers: ['example'], timezone: 'UTC', offsetHours: 24, autoRefreshSeconds: 0};
  const context = vm.createContext({document, Intl, Date, CSS: {escape: s => s},
    window: {clearTimeout() {}, setTimeout() {}, setInterval() {}, addEventListener() {}},
    saveConfig: async c => c, getStatuses: async () => ({entries: [], offsetHours: 24, generatedAt: new Date().toISOString()}),
    getCredentialsStatus: async () => ({configured: true, source: 'file'}), ...overrides});
  let source = fs.readFileSync(new URL('../src/main.ts', import.meta.url), 'utf8');
  source = source.replace(/^import[\s\S]*?from "\.\/api";/, '').replace('void boot();', '');
  vm.runInContext(ts.transpileModule(source, {compilerOptions: {target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None}}).outputText, context);
  vm.runInContext(`desired = ${JSON.stringify(config)};`, context);
  return {context, elements, run: code => vm.runInContext(code, context)};
}

test('rapid delay and timezone edits survive serialized saves', async () => {
  const first = deferred(); const saved = [];
  const h = harness({saveConfig: async c => {saved.push(structuredClone(c)); return saved.length === 1 ? first.promise : c;}});
  const p1 = h.run('mutate(d => d.offsetHours = 48, {refetch: false})');
  const p2 = h.run('mutate(d => d.timezone = "Europe/Berlin", {refetch: false})');
  first.resolve(saved[0]); await Promise.all([p1, p2]);
  assert.equal(saved.at(-1).offsetHours, 48);
  assert.equal(saved.at(-1).timezone, 'Europe/Berlin');
});

test('failed persistence does not fetch and mislabel an old schedule', async () => {
  let calls = 0;
  const h = harness({saveConfig: async () => {throw Error('disk unavailable');}, getStatuses: async () => {calls++;}});
  await h.run('mutate(d => d.offsetHours = 48, {refetch: true})');
  await h.run('requestRefresh()');
  assert.equal(calls, 0);
  assert.match(h.elements.get('liveList').innerHTML, /Could not load/);
  assert.equal(h.run('desired.offsetHours'), 48);
});

test('an obsolete in-flight report is discarded and a follow-up runs', async () => {
  const first = deferred(); let calls = 0;
  const h = harness({getStatuses: async () => {calls++; return calls === 1 ? first.promise : {entries: [], offsetHours: 48, generatedAt: new Date().toISOString()};}});
  const fetching = h.run('requestRefresh()');
  await Promise.resolve(); await Promise.resolve();
  const saving = h.run('mutate(d => d.offsetHours = 48, {refetch: true})');
  await saving;
  first.resolve({entries: [], offsetHours: 24, generatedAt: new Date().toISOString()});
  await fetching;
  assert.equal(h.run('report.offsetHours'), 48); assert.equal(calls, 2);
});

test('schedule boundaries reclassify without a network refresh', () => {
  const h = harness();
  h.run(`report = {entries: [{login:'example', state:'upcoming', simStart:new Date(Date.now()-1000).toISOString(), simEnd:new Date(Date.now()+60000).toISOString()}]}; render = () => {}; tick();`);
  assert.equal(h.run('report.entries[0].state'), 'live');
  h.run('report.entries[0].simEnd = new Date(Date.now()-1).toISOString(); tick();');
  assert.equal(h.run('report.entries[0].state'), 'offline');
});

test('initial API failure removes loading placeholders and offers retry', async () => {
  const h = harness({getStatuses: async () => {throw Error('offline');}});
  await h.run('requestRefresh()');
  assert.match(h.elements.get('liveList').innerHTML, /data-retry/);
  assert.doesNotMatch(h.elements.get('liveList').innerHTML, /skeleton/);
});

test('adding the first streamer after an empty report still offers retry on failure', async () => {
  const h = harness({getStatuses: async () => {throw Error('credentials missing');}});
  h.run('report = {entries: [], offsetHours: 24, generatedAt: new Date().toISOString()};');
  await h.run('requestRefresh()');
  assert.match(h.elements.get('liveList').innerHTML, /data-retry/);
});
