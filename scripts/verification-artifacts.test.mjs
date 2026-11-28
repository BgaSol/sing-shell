import {test} from 'node:test';
import assert from 'node:assert/strict';
import {latestVerificationArtifacts} from './verification-artifacts.mjs';
import {targets} from './targets.mjs';
const reports = () => targets.map((target, i) => ({
  id:i + 1, name:`verification-${target}`, created_at:'2026-11-28T00:00:00Z', expired:false,
}));
test('release aggregation selects the newest attempt without merging older same-name reports', () => {
  const artifacts = reports();
  const retry = {...artifacts[1], id:99, created_at:'2026-11-28T00:01:00Z'};
  const expected = [1,99,3,4,5];
  assert.deepEqual(latestVerificationArtifacts([retry,...artifacts]), expected);
  assert.deepEqual(latestVerificationArtifacts([...artifacts,retry]), expected);
  assert.deepEqual(latestVerificationArtifacts([...artifacts,{...artifacts[0],id:100}]), [100,2,3,4,5]);
});
test('release aggregation refuses missing or expired newest reports instead of falling back', () => {
  assert.throws(() => latestVerificationArtifacts(reports().slice(1)), /missing current/);
  assert.throws(() => latestVerificationArtifacts([
    ...reports(), {...reports()[0],id:99,created_at:'2026-11-28T00:01:00Z',expired:true},
  ]), /missing current/);
});
