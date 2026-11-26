import {test} from 'node:test';
import assert from 'node:assert/strict';
import {corePresentation} from '../src/core-model.ts';
import {ready} from '../src/update-model.ts';
const snapshot={settings:{coreSource:{mode:'managed'},managed:{active:{version:'1.14.3'}},updatePreferences:{automaticCore:true}},core:{initialized:true,usable:true,canRetry:true,notice:null,maintenance:{phase:'downloading',error:null,downloadedBytes:1024,totalBytes:2048}},runtime:{pid:1,version:'1.14.3'}};
test('core preparation does not count as application restart and preserves running version',()=>{
 const p=corePresentation(snapshot);assert.match(p.version,/正在运行 1.14.3/);assert.equal(p.busy,true);
 assert.deepEqual(ready({items:{engine:{phase:'ready'},app:{phase:'idle'}}}),[]);
});
test('paused automatic adoption and external ownership have explicit presentation',()=>{
 const s=structuredClone(snapshot);s.core.maintenance.phase='ready';s.settings.updatePreferences.automaticCore=false;
 s.core.maintenance.phase='paused';assert.equal(corePresentation(s).state,'核心维护已暂停，可明确重试');
 s.settings.coreSource={mode:'external',path:'C:/user/sing-box.exe'};assert.equal(corePresentation(s).managed,false);assert.match(corePresentation(s).state,/手动管理/);
});

test('prepared material and repair actions are distinct from installed and update actions',()=>{
 const s=structuredClone(snapshot);s.runtime.pid=null;s.settings.managed.active=null;s.core.usable=false;
 Object.assign(s.core.maintenance,{phase:'error',error:'write failed',failureKind:'local',preparedVersion:'1.14.4'});
 const p=corePresentation(s);assert.match(p.version,/1.14.4 待采用/);assert.equal(p.action,'retry');assert.equal(p.retryLabel,'重试准备核心');
 s.core.maintenance.failureKind='compatibility';assert.equal(corePresentation(s).retryLabel,'重新校验');
 Object.assign(s.core.maintenance,{phase:'ready',error:null});assert.equal(corePresentation(s).action,'update');
 s.core.maintenance.phase='idle';s.core.maintenance.warning='cleanup failed';assert.equal(corePresentation(s).error,'cleanup failed');
});
