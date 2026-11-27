import { test } from 'node:test';
import assert from 'node:assert/strict';
import { RoutingDraft, canBypass, bypassDomains, setExit } from '../src/routing-model.ts';

const policy = (id, suffix=false) => ({id,name:id,enabled:true,exit:'direct',match:{kind:'custom',domains:[{value:`${id}.example.com`,includeSubdomains:suffix}],ipCidrs:[]}});
function model() { const m=new RoutingDraft(); m.load({defaultExit:'proxy',policies:[policy('a',true),policy('b'),policy('c')]},3); return m; }
function deferred() { let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject}; }

test('missing bypass defaults are clean; exit and enable changes preserve explicit intent',()=>{
 const m=model();assert.equal(m.dirty,false);assert.equal(m.value.policies[0].bypassTun,false);
 m.change('a',p=>{p.bypassTun=true;p.enabled=false;});assert.deepEqual(bypassDomains(m.value.policies[0]),[0]);
 assert.equal(m.value.policies[0].bypassTun,true);m.change('a',p=>setExit(p,'proxy'));assert.equal(m.value.policies[0].bypassTun,false);
 m.change('a',p=>setExit(p,'direct'));assert.equal(m.value.policies[0].bypassTun,false);
 assert.equal(canBypass(m.value.policies[0]),true);assert.equal(canBypass({...policy('s'),match:{kind:'source',sourceId:'cn-ip'}}),false);
});
test('an invalid bypass row blocks save but never prevents another candidate being merged',async()=>{
 const m=model();m.change('a',p=>p.bypassTun=true);assert.equal(m.issues.length,1);
 const generation=m.generation,b=structuredClone(m.value.policies[1]);b.name='edited';m.replace(b,false,generation);
 assert.equal(m.value.policies[1].name,'edited');assert.equal(m.issues.length,1);
 let written=false;await assert.rejects(m.save(async()=>{written=true;}),/修改标出的域名/);assert.equal(written,false);
 m.change('a',p=>p.match.domains[0].includeSubdomains=false);assert.equal(m.issues.length,0);
});
test('global ordering and one-step undo preserve subsequent edits and clamp restored positions',()=>{
 const m=model();m.move('a',-1);m.move('c',1);assert.deepEqual(m.value.policies.map(p=>p.id),['a','b','c']);
 m.move('c',-1);assert.deepEqual(m.value.policies.map(p=>p.id),['a','c','b']);
 m.remove('c');m.change('b',p=>p.name='keep');m.undo();assert.deepEqual(m.value.policies.map(p=>p.id),['a','c','b']);assert.equal(m.value.policies[2].name,'keep');
 m.remove('b');m.value.policies.pop();m.undo();assert.deepEqual(m.value.policies.map(p=>p.id),['a','b']);
});
test('source additions and undo reject duplicates without partial insertion',()=>{
 const m=model(),s={...policy('source'),match:{kind:'source',sourceId:'cn-ip'}};m.addSources([s]);m.remove('source');m.addSources([{...s,id:'replacement'}]);
 assert.throws(()=>m.undo(),/无法撤销/);assert.equal(m.value.policies.filter(p=>p.match.kind==='source').length,1);
 const before=structuredClone(m.value);assert.throws(()=>m.addSources([{...s,id:'other',match:{kind:'source',sourceId:'github'}},s]),/只能添加一次/);assert.deepEqual(m.value,before);
});
test('save freezes all mutations and only adopts the native normalized result',async()=>{
 const m=model();m.change('a',p=>p.name='new');const gate=deferred();let payload;
 const pending=m.save(async(routing,revision)=>{payload={routing,revision};return gate.promise;});assert.equal(m.saving,true);
 m.change('a',p=>p.name='lost');m.move('a',1);m.remove('b');m.setDefault('direct');m.load({defaultExit:'direct',policies:[]},8);
 assert.throws(()=>m.replace(policy('x'),true,m.generation),/草稿已改变/);assert.equal(m.value.policies[0].name,'new');assert.equal(payload.revision,3);
 gate.resolve({routing:{...payload.routing,policies:payload.routing.policies.map(p=>({...p,name:p.name.trim()}))},routingRevision:4});assert.equal(await pending,true);
 assert.equal(m.saving,false);assert.equal(m.dirty,false);assert.equal(m.revision,4);assert.equal(m.value.policies.length,3);
});
test('failed and conflicting saves preserve the exact draft and allow retry',async()=>{
 const m=model();m.remove('a');const before=structuredClone(m.value),undo=structuredClone(m.removed);
 await assert.rejects(m.save(async()=>{throw Error('revision conflict');}),/revision conflict/);
 assert.equal(m.saving,false);assert.equal(m.dirty,true);assert.deepEqual(m.value,before);assert.deepEqual(m.removed,undo);assert.equal(m.revision,3);
 await m.save(async routing=>({routing,routingRevision:4}));assert.equal(m.dirty,false);assert.equal(m.removed,null);
});
test('late editor results cannot overwrite reloads or later local changes',()=>{
 const m=model(),generation=m.generation,next=structuredClone(m.value.policies[0]);m.load(m.value,4);
 assert.throws(()=>m.replace(next,false,generation),/草稿已改变/);const fresh=m.generation;m.change('b',p=>p.enabled=false);
 assert.throws(()=>m.replace(next,false,fresh),/草稿已改变/);assert.equal(m.value.policies[1].enabled,false);
});
