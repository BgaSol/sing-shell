import {test} from 'node:test';
import assert from 'node:assert/strict';
import {verifyPreservedSettings} from './verify-settings.mjs';

test('upgrade permits only additive connection defaults and preserves every prior value',()=>{
  const previous={schemaVersion:5,profile:{nodes:[{id:'node',spec:{password:'fixture'}}],routing:{policies:[]}},startup:{lastConnection:{node:{id:'node'}}},managed:{active:null},preferencesRevision:2};
  const next=structuredClone(previous);next.connectionRevision=0;next.profile.connection={ipv4Compatibility:false};next.startup.lastConnection.options={ipv4Compatibility:false};
  const bytes=value=>Buffer.from(JSON.stringify(value));
  assert.equal(verifyPreservedSettings(bytes(previous),bytes(next)).connectionDefaultsAdded,true);
  for(const mutate of [v=>v.preferencesRevision++,v=>v.profile.nodes[0].spec.password='changed',v=>v.profile.routing.policies.push({id:'extra'}),v=>v.profile.connection.ipv4Compatibility=true,v=>v.connectionRevision=1,v=>v.extra=true]){
    const changed=structuredClone(next);mutate(changed);assert.throws(()=>verifyPreservedSettings(bytes(previous),bytes(changed)));
  }
  assert.equal(verifyPreservedSettings(bytes(next),bytes(next)).connectionDefaultsAdded,false);
  const old=Buffer.from('{"schemaVersion":4}');verifyPreservedSettings(old,old);assert.throws(()=>verifyPreservedSettings(old,Buffer.from('{ "schemaVersion":4}')));
});
test('schema-five upgrade defaults bypass without altering rule intent or saved connection',()=>{
 const policy={id:'rd',name:'RustDesk',exit:'direct',enabled:true,match:{kind:'custom',domains:[],ipCidrs:['192.0.2.1/32']}};
 const before={schemaVersion:5,connectionRevision:0,profile:{connection:{ipv4Compatibility:false},routing:{policies:[policy]}},startup:{lastConnection:{options:{ipv4Compatibility:false},routing:{policies:[policy]}}}};
 const after=structuredClone(before);after.profile.routing.policies[0].bypassTun=false;after.startup.lastConnection.routing.policies[0].bypassTun=false;
 verifyPreservedSettings(JSON.stringify(before),JSON.stringify(after));after.profile.routing.policies[0].bypassTun=true;assert.throws(()=>verifyPreservedSettings(JSON.stringify(before),JSON.stringify(after)));
});
