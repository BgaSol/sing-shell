import {test} from 'node:test';
import assert from 'node:assert/strict';
import {authorizationLabel,connectionReady} from '../src/session-model.ts';
test('authorization progress separates approval activation identity and protocol',()=>{
  const labels=[null,'registration','activation','authentication','protocol'].map(authorizationLabel);
  assert.equal(new Set(labels).size,5);
  assert.match(labels[1],/系统/);assert.match(labels[2],/就绪/);assert.match(labels[3],/身份/);assert.match(labels[4],/会话/);
});

test('a spawned core PID is not a confirmed TUN connection during replacement',()=>{const s={runtime:{pid:123},core:{phase:'starting'}};assert.equal(connectionReady(s),false);s.core.phase='running';assert.equal(connectionReady(s),true);s.runtime.pid=null;assert.equal(connectionReady(s),false);s.core.phase='cleanupRequired';assert.equal(connectionReady(s),false);});
