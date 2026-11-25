import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {targets} from './targets.mjs';
import {validate,updater} from './package-artifacts.mjs';
const version=JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json')).version;
const notes=fs.readFileSync(`docs/releases/v${version}.md`,'utf8');
const repository='BgaSol/sing-shell',tag=`v${version}`;
const platforms={},files=[];let source;
for(const target of targets){const root=path.resolve('artifacts/packages',target);const manifest=validate(root);const report=JSON.parse(fs.readFileSync(`artifacts/verification/${target}.json`));assert.equal(manifest.version,version);assert.equal(manifest.target,target);assert.equal(report.passed,true);assert.equal(report.version,version);assert.equal(report.target,target);assert.equal(report.source,manifest.source);assert.deepEqual(report.packages,manifest.files);assert.equal(report.upgrade?.actualVersionConfirmed,true);source??=manifest.source;assert.equal(source,manifest.source);const item=updater(manifest);const signature=fs.readFileSync(path.join(root,item.name+'.sig'),'utf8').trim();assert.ok(signature);platforms[target]={url:`https://github.com/${repository}/releases/download/${tag}/${encodeURIComponent(item.name)}`,signature,sha256:item.sha256,size:item.size};files.push(...manifest.files.map(f=>path.join(root,f.name)));}
const update={id:'sing-shell.app',version,notes,platforms};fs.writeFileSync('artifacts/core-update.json',JSON.stringify(update,null,2)+'\n');files.push(path.resolve('artifacts/core-update.json'));
function gh(args){return execFileSync('gh',args,{encoding:'utf8',stdio:['ignore','pipe','inherit']});}
const refs=execFileSync('git',['ls-remote','--tags','origin',`refs/tags/${tag}`,`refs/tags/${tag}^{}`],{encoding:'utf8'}).trim().split('\n').filter(Boolean).map(line=>line.split(/\s+/));
if(refs.length){const ref=refs.find(([,name])=>name.endsWith('^{}'))??refs[0];assert.equal(ref[0],source,'existing tag source mismatch');}
const existing=JSON.parse(gh(['api',`repos/${repository}/releases?per_page=100`]));const release=existing.find(r=>r.tag_name===tag);if(release){assert.equal(release.draft,true,'refuse to modify a published release');assert.equal(release.target_commitish,source,'draft source mismatch');}else{gh(['release','create',tag,'--repo',repository,'--draft','--target',source,'--title',`Sing Shell ${tag}`,'--notes-file',`docs/releases/${tag}.md`]);}
const draft=JSON.parse(gh(['release','view',tag,'--repo',repository,'--json','assets']));
for(const file of files){const name=path.basename(file),asset=draft.assets.find(a=>a.name===name);if(asset){const {hash}=await import('./package-artifacts.mjs');assert.equal(asset.digest,`sha256:${hash(file)}`,`existing asset changed: ${name}`);}else{gh(['release','upload',tag,file,'--repo',repository]);}}
console.log(JSON.stringify({version,source,release:`https://github.com/${repository}/releases/tag/${tag}`,files:files.map(f=>path.basename(f))},null,2));
