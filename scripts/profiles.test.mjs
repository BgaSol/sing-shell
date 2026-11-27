import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readNodeFile} from '../src/import-file.ts';
import {reorderVisible,setExit} from '../src/routing-model.ts';
test('changing to proxy clears bypass and switching back never silently restores it',()=>{const p={exit:'direct',bypassTun:true};assert.equal(setExit(p,'proxy'),true);assert.equal(p.bypassTun,false);assert.equal(setExit(p,'direct'),false);assert.equal(p.bypassTun,false);});
test('node file decoding rejects oversize, invalid UTF-8 and private I/O errors',async()=>{
 assert.equal(await readNodeFile(new File(['vless://test'],'nodes.txt')),'vless://test');
 for(const [file,pattern]of [[new File([new Uint8Array([255])],'bad.txt'),/UTF-8/],[{size:1,arrayBuffer:async()=>{throw Error('private path and credentials');}},/文件读取失败/],[{size:2097153},/2 MiB/]]){await assert.rejects(()=>readNodeFile(file),e=>pattern.test(e.message)&&!e.message.includes('credentials'));}
});
test('filtered drag ordering keeps hidden global slots and accepts user priority',()=>{
 const p=['a','hidden-one','b','hidden-two','c'].map(id=>({id}));
 assert.deepEqual(reorderVisible(p,['a','b','c'],'c','a').map(p=>p.id),['c','hidden-one','a','hidden-two','b']);
 assert.deepEqual(reorderVisible(p,p.map(p=>p.id),'c','a').map(p=>p.id),['c','a','hidden-one','b','hidden-two']);
 assert.deepEqual(reorderVisible(p,['a','b'],'missing','a'),p);
});
