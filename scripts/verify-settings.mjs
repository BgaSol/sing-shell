import assert from 'node:assert/strict';

// Older schemas are refused byte-for-byte; schema 5 may materialize only these new defaults.
export function verifyPreservedSettings(before,after) {
  const expected=JSON.parse(before);
  if(expected.schemaVersion!==5){assert.deepEqual(after,before);return{connectionDefaultsAdded:false};}
  const added=expected.connectionRevision===undefined||expected.profile.connection===undefined;
  expected.connectionRevision??=0;
  expected.profile.connection??={ipv4Compatibility:false};
  if(expected.startup.lastConnection)expected.startup.lastConnection.options??={ipv4Compatibility:false};
  assert.deepEqual(JSON.parse(after),expected,'existing settings must be unchanged apart from missing connection defaults');
  return{connectionDefaultsAdded:added};
}
