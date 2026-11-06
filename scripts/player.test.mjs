import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

function harness(paused = false) {
  const elements = new Map(); const events = new Map(); const calls = [];
  const get = id => {if (!elements.has(id)) elements.set(id,{hidden:true,textContent:'',addEventListener(){},innerHTML:'',appendChild(){}});return elements.get(id);};
  const mock = {isPaused:()=>paused, getCurrentTime:()=>10,
    seek:n=>calls.push(['seek',n]), pause:()=>calls.push(['pause'])};
  const context = vm.createContext({URLSearchParams, URL, Date, location:{search:'',hostname:'tauri.localhost'},
    document:{getElementById:get},window:{clearTimeout(){},setInterval(){}},
    invoke:async()=>{},listen:(name,cb)=>{events.set(name,cb);return Promise.resolve(()=>{});},mock});
  let source = fs.readFileSync(new URL('../src/player.ts',import.meta.url),'utf8').replace(/^import .*;\r?\n/gm,'');
  vm.runInContext(ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.None}}).outputText,context);
  const run=code=>vm.runInContext(code,context);
  run('player=mock;ready=true;simStartMs=Date.now()-120000;durationSeconds=600;');
  return {run,calls,events,elements};
}
test('deliberate pause prevents periodic seek correction',()=>{
  const h=harness(true);h.run('driftCheck("test")');assert.equal(h.calls.length,0);
});
test('playing video outside drift tolerance rejoins live position',()=>{
  const h=harness(false);h.run('driftCheck("test")');
  assert.equal(h.calls[0][0],'seek');assert.ok(h.calls[0][1]>=120);
});
test('finished replay pauses and shows its end state',()=>{
  const h=harness(false);h.run('durationSeconds=60;driftCheck("test")');
  assert.deepEqual(h.calls,[['pause']]);assert.equal(h.elements.get('overlay').hidden,false);
});
test('delay retarget into the future pauses the open player',()=>{
  const h=harness(false);h.events.get('player-retarget')({payload:{vodId:'',startMs:Date.now()+60000,durationSeconds:600}});
  assert.deepEqual(h.calls,[['pause']]);assert.match(h.elements.get('overlayTitle').textContent,/not started/);
});
