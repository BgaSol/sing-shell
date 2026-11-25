import {test} from 'node:test';
import assert from 'node:assert/strict';
import {working,downloadable,ready,label} from '../src/update-model.ts';
test('check failures preserve a candidate but do not offer an unconfirmed download',()=>{
  const item={phase:'error',candidate:{version:'0.3.0'},errorStage:'check'};
  assert.equal(downloadable(item),false);
  assert.equal(downloadable({...item,errorStage:'download'}),true);
  assert.equal(working({...item,phase:'downloading'}),true);
  assert.deepEqual(ready({items:{app:{phase:'ready'},engine:{phase:'external'}}}),['app']);
  assert.equal(label({phase:'unsupported'}),'使用系统安装方式更新');
});
