"""Execute the browser init callback without launching Chromium; not UI evidence."""
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]

class BrowserFixtureContract(unittest.TestCase):
    def test_native_observation_list_has_epoch_and_records(self):
        script = r'''
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const source = fs.readFileSync('scripts/verify-specialistbots-headless.mjs','utf8');
const start = source.indexOf('await page.addInitScript(');
const end = source.indexOf('const fixture =', start);
assert(start >= 0 && end > start);
const window = {};
const sessionStorage={getItem:()=>null,setItem:()=>{}};
const page = {addInitScript: fn => fn()};
(async () => {
  await vm.runInNewContext('(async()=>{' + source.slice(start,end) + '})()', {window,page,sessionStorage});
  const result = await window.__TAURI_INTERNALS__.invoke('list_specialist_observations',{project:'/synthetic'});
  assert(Number.isSafeInteger(result.epoch) && result.epoch >= 0, 'native status requires an epoch');
  assert(Array.isArray(result.records), 'native status requires records');
})().catch(error => {console.error(error);process.exitCode=1;});
'''
        result = subprocess.run(['node','-e',script],cwd=ROOT,capture_output=True,text=True,encoding="utf-8")
        self.assertEqual(result.returncode,0,result.stderr)

    def test_readonly_fixture_persists_partitions_and_revokes_pending_query(self):
        script = r'''
const fs=require('node:fs'), vm=require('node:vm'), assert=require('node:assert/strict');
const source=fs.readFileSync('scripts/verify-specialistbots-headless.mjs','utf8');
const start=source.indexOf('await page.addInitScript('),end=source.indexOf('const fixture =',start);
const data=new Map(),sessionStorage={getItem:k=>data.get(k)??null,setItem:(k,v)=>data.set(k,v)};
const window={},page={addInitScript:fn=>fn()};
const init=()=>vm.runInNewContext('(async()=>{'+source.slice(start,end)+'})()',{window,page,sessionStorage});
(async()=>{
 await init();const invoke=(...a)=>window.__TAURI_INTERNALS__.invoke(...a);
 const args={project:'/a',remoteId:'synthetic-github',repo:'fixture/repo',pr:7,expectedEpoch:0};
 const binding=await invoke('bind_specialist_project',args);
 assert(binding?.id,'binding required');
 await invoke('query_specialist_observation',{project:'/a',bindingId:binding.id});
 await init();
 let status=await invoke('list_specialist_observations',{project:'/a'});
 assert.equal(status.records[0].observations.length,1);
 assert.equal((await invoke('list_specialist_observations',{project:'/b'})).records.length,0);
 window.__SPECIALIST_FIXTURE__.deferQuery=true;
 const pending=invoke('query_specialist_observation',{project:'/a',bindingId:binding.id});
 await invoke('stop_specialist_observations',{project:'/a'});
 window.__SPECIALIST_FIXTURE__.releaseQuery();
 await assert.rejects(pending);
 assert.equal(window.__SPECIALIST_FIXTURE__.pendingQueries,0);
 assert.equal(window.__SPECIALIST_FIXTURE__.settledQueries,1);
 await assert.rejects(invoke('bind_specialist_project',args));
 status=await invoke('list_specialist_observations',{project:'/a'});
 assert.equal(status.epoch,1);assert.equal(status.records[0].active,false);
 assert.equal(status.records[0].observations.length,1);
 window.__SPECIALIST_FIXTURE__.failList=true;
 await assert.rejects(invoke('list_specialist_observations',{project:'/a'}));
 await assert.rejects(invoke('execute_specialist_task',{project:'/a'}));
})().catch(e=>{console.error(e);process.exitCode=1;});
'''
        result=subprocess.run(['node','-e',script],cwd=ROOT,capture_output=True,text=True,encoding="utf-8")
        self.assertEqual(result.returncode,0,result.stderr)
