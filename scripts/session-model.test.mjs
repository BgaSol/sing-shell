import {test} from 'node:test';
import assert from 'node:assert/strict';
import {authorizationLabel} from '../src/session-model.ts';
test('authorization progress separates approval activation identity and protocol',()=>{
  const labels=[null,'registration','activation','authentication','protocol'].map(authorizationLabel);
  assert.equal(new Set(labels).size,5);
  assert.match(labels[1],/系统/);assert.match(labels[2],/就绪/);assert.match(labels[3],/身份/);assert.match(labels[4],/会话/);
});
