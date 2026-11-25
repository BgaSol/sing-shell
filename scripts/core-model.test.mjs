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
 assert.equal(corePresentation(s).state,'已准备，自动采用已暂停');
 s.settings.coreSource={mode:'external',path:'C:/user/sing-box.exe'};assert.equal(corePresentation(s).managed,false);assert.match(corePresentation(s).state,/手动管理/);
});
