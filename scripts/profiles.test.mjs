import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registerHooks} from 'node:module';
import {existsSync} from 'node:fs';
import {fileURLToPath} from 'node:url';

registerHooks({resolve(specifier,context,next){
  if(specifier.startsWith('.')&&context.parentURL?.includes('/src/')){
    const url=new URL(specifier+'.ts',context.parentURL);
    if(existsSync(fileURLToPath(url)))return {url:url.href,shortCircuit:true};
  }
  return next(specifier,context);
}});
const {core}=await import('../src/core.ts');
const {createProfiles}=await import('../src/profiles.ts');
class Element {
  constructor(tag='div'){this.tag=tag;this.children=[];this.dataset={};this.value='';this.hidden=false;this.disabled=false;this.textContent='';}
  append(...children){this.children.push(...children);}
  replaceChildren(...children){this.children=children;}
  setAttribute(name,value){this[name]=value;}
  querySelectorAll(){return this.children.flatMap(child=>child instanceof Element?[...(child.tag==='button'&&child.dataset.node?[child]:[]),...child.querySelectorAll()]:[]);}
}
test('imported nodes stay selectable and a clean routing form follows node revisions',async()=>{
  const elements=new Map();
  globalThis.document={getElementById:id=>{if(!elements.has(id))elements.set(id,new Element());return elements.get(id);},createElement:tag=>new Element(tag),createTextNode:text=>text};
  const element=id=>document.getElementById(id);
  const state={settings:{profileRevision:0,profile:{mode:'generated',nodes:[],selectedNode:null,ruleCommit:'a'.repeat(40),routing:{defaultAction:'proxy',customRules:[],siteRules:[]}}},runtime:{pid:null},runtimeBusy:false};
  const node={id:'imported',name:'Test',protocol:'Trojan',server:'example.test',port:443};
  core.snapshot=async()=>structuredClone(state);
  core.profileCancel=async()=>{};
  core.profilePreview=async()=>({ticket:1,items:[{line:1,node,duplicate:false}]});
  core.profileCommit=async()=>{state.settings.profile.nodes.push(node);state.settings.profileRevision++;};
  core.profileSelect=async id=>{state.settings.profile.selectedNode=id;state.settings.profileRevision++;};
  let savedRevision;
  core.profileRouting=async(routing,revision)=>{savedRevision=revision;assert.equal(revision,state.settings.profileRevision);state.settings.profile.routing=structuredClone(routing);state.settings.profileRevision++;return {revision:state.settings.profileRevision,...structuredClone(state.settings.profile)};};
  const profiles=createProfiles();profiles.render(structuredClone(state));
  const settle=async()=>{await new Promise(setImmediate);await new Promise(setImmediate);};
  element('node-input').value='trojan://test@example.test:443';element('node-preview').onclick();await settle();
  element('node-commit').onclick();await settle();
  let choose=element('node-list').querySelectorAll()[0];
  assert.equal(choose.disabled,false,'import completion must release the select button');
  choose.onclick();await settle();assert.equal(state.settings.profile.selectedNode,node.id);
  profiles.render(structuredClone(state));
  element('routing-default').value='direct';element('routing-default').onchange();element('save-routing').onclick();await settle();
  assert.equal(savedRevision,2,'a clean form uses the latest revision after import and selection');
  assert.equal(state.settings.profile.routing.defaultAction,'direct');
  assert.match(element('routing-state').textContent,/已保存/);
});

test('preview and file failures invalidate old tickets and keep results as text',async()=>{
  const elements=new Map();
  globalThis.document={getElementById:id=>{if(!elements.has(id))elements.set(id,new Element());return elements.get(id);},createElement:tag=>new Element(tag),createTextNode:text=>text};
  const element=id=>document.getElementById(id);
  element('routing-panel').hidden=true;
  const settle=async()=>{await new Promise(setImmediate);await new Promise(setImmediate);};
  const node={id:'test',name:'<img src=x onerror=alert(1)>',protocol:'VLESS',server:'example.test',port:443};
  let cancellations=0,commits=0;
  core.profileCancel=async()=>{cancellations++;};
  core.profileCommit=async()=>{commits++;};
  core.profilePreview=async()=>({ticket:1,items:[{line:1,node,duplicate:false},{line:2,node:null,error:'invalid <tag>',duplicate:false}]});
  createProfiles();
  const preview=async()=>{element('node-preview').onclick();await settle();};
  await preview();
  assert.equal(element('node-commit').disabled,false,'a partially valid batch remains importable');
  assert.match(element('node-import-result').textContent,/<img src=x onerror=alert\(1\)>/);
  assert.match(element('node-import-result').textContent,/invalid <tag>/);
  assert.equal(element('node-import-result').children.length,0);
  core.profilePreview=async()=>{throw Error('preview failed');};
  await preview();
  assert.equal(element('node-commit').disabled,true,'failed new preview must not restore old ticket');
  element('node-commit').onclick();await settle();assert.equal(commits,0);
  core.profilePreview=async()=>({ticket:2,items:[{line:1,node:null,error:'bad link',duplicate:false}]});
  await preview();assert.equal(element('node-commit').disabled,true);
  core.profilePreview=async()=>({ticket:3,items:[{line:1,node,duplicate:false}]});
  const fileInput=element('node-file');
  const load=async file=>{fileInput.files=[file];fileInput.onchange();await settle();};
  for(const [file,error] of [
    [new File([new Uint8Array([0xff])],'bad.txt'),/UTF-8/],
    [{size:1,arrayBuffer:async()=>{throw Error('secret local path');}},/无法读取节点文件/],
    [{size:2*1024*1024+1},/2 MiB/],
  ]){
    await preview();assert.equal(element('node-commit').disabled,false);
    await load(file);
    assert.equal(element('node-commit').disabled,true);
    assert.match(element('node-import-result').textContent,error);
    assert.doesNotMatch(element('node-import-result').textContent,/secret local path/);
    assert.equal(fileInput.disabled,false);
    assert.equal(fileInput.value,'');
  }
  await load(new File(['vless://test'],'valid.txt'));
  assert.equal(element('node-input').value,'vless://test');
  assert.equal(element('node-commit').disabled,true,'file loading requires a new preview');
  await preview();assert.equal(element('node-commit').disabled,false);
  element('node-input').oninput();await settle();
  assert.equal(element('node-commit').disabled,true);
  assert.ok(cancellations>=10);
});
