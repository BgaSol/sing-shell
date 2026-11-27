import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createPrivateKey,createPublicKey} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {signedMaterial,verifyMaterial,hash} from './core-material.mjs';
import {targets} from './targets.mjs';
const [materialPath,reportsDirectory]=process.argv.slice(2);
const data=JSON.parse(fs.readFileSync(materialPath));
assert.match(data.version,/^1\.14\.[0-9]+$/);
for(const target of targets){
  const report=JSON.parse(fs.readFileSync(`${reportsDirectory}/network-preflight-${target}/artifacts/network-preflight/${target}/integration-report.json`));
  assert.equal(report.passed,true);assert.equal(report.coreVersion,data.version);assert.equal(report.freshProtectedInstall,true);
}
const key=createPrivateKey(process.env.CORE_SIGNING_PRIVATE_KEY);
const raw=Buffer.from(createPublicKey(key).export({format:'jwk'}).x,'base64url').toString('base64');
assert.equal(raw,fs.readFileSync('src-tauri/resources/cores/public-key.txt','utf8').trim());
const envelope=signedMaterial(data,key);verifyMaterial(envelope,createPublicKey(key));
fs.mkdirSync('artifacts/core-publish',{recursive:true});
const file='artifacts/core-publish/material.json';fs.writeFileSync(file,JSON.stringify(envelope,null,2)+'\n');
const gh=args=>execFileSync('gh',args,{encoding:'utf8',stdio:['ignore','pipe','inherit']});
const releases=JSON.parse(gh(['api','repos/BgaSol/sing-shell/releases?per_page=100']));
const tag=`core-v${data.version}`;
const source=process.env.GITHUB_SHA??execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim();assert.match(source,/^[a-f0-9]{40}$/);
if(!releases.some(r=>r.tag_name===tag))gh(['release','create',tag,'--target',source,'--prerelease','--latest=false','--title',`Verified sing-box ${data.version}`,'--notes',`Official SagerNet/sing-box ${data.version} material, verified on five native platforms. Signed by Sing Shell for offline protected installation.`]);
const asset=JSON.parse(gh(['release','view',tag,'--json','assets'])).assets.find(a=>a.name==='material.json');
if(asset)assert.equal(asset.digest,`sha256:${hash(fs.readFileSync(file))}`,'refuse changed fixed-version material');
else gh(['release','upload',tag,file]);
const channel=releases.find(r=>r.tag_name==='core-channel');
if(!channel)gh(['release','create','core-channel','--target',source,'--prerelease','--latest=false','--title','Verified core channel','--notes','Signed recommendation for independently updated official sing-box cores.']);
else if(channel.assets.some(a=>a.name==='recommended.json')){
  const previous=JSON.parse(gh(['api',`repos/BgaSol/sing-shell/releases/assets/${channel.assets.find(a=>a.name==='recommended.json').id}`,'-H','Accept: application/octet-stream']));
  const old=verifyMaterial(previous,createPublicKey(key));
  assert.ok(Number(data.version.split('.')[2])>=Number(old.version.split('.')[2]),'refuse recommendation downgrade');
}
const recommendation='artifacts/core-publish/recommended.json';fs.copyFileSync(file,recommendation);
gh(['release','upload','core-channel',recommendation,'--clobber']);
console.log(JSON.stringify({version:data.version,targets:targets.length,sha256:hash(fs.readFileSync(file)),published:true}));
