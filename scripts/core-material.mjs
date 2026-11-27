import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {createHash, createPrivateKey, createPublicKey, sign, verify} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {targets} from './targets.mjs';
export const purpose = 'com.bgasol.singshell.core/v1';
export const hash = bytes => createHash('sha256').update(bytes).digest('hex');
export function signedMaterial(payload, key) {
  const bytes = Buffer.from(JSON.stringify(payload));
  return {payload: bytes.toString('base64'), signature: sign(null, bytes, key).toString('base64')};
}
export function verifyMaterial(envelope, key) {
  const bytes = Buffer.from(envelope.payload, 'base64');
  assert.ok(verify(null, bytes, key, Buffer.from(envelope.signature, 'base64')));
  const data = JSON.parse(bytes); assert.equal(data.purpose, purpose); assert.equal(data.schemaVersion, 1);
  assert.deepEqual(Object.keys(data.platforms).sort(), [...targets].sort());
  return data;
}
// Only publisher tooling uses the upstream API. Runtime clients use signed static material.
export async function collect(version) {
  assert.match(version, /^1\.14\.[0-9]+$/);
  const release = JSON.parse(execFileSync('gh', ['api', `repos/SagerNet/sing-box/releases/tags/v${version}`], {encoding:'utf8', maxBuffer:4*1024*1024}));
  assert.equal(release.draft, false); assert.equal(release.prerelease, false); assert.equal(release.tag_name, `v${version}`);
  const suffixes = ['windows-amd64.zip','windows-arm64.zip','linux-amd64.tar.gz','linux-arm64.tar.gz','darwin-arm64.tar.gz'];
  const platforms = {};
  const tar = process.platform==='linux'?'bsdtar':'tar';
  for (const [index,target] of targets.entries()) {
    const name = `sing-box-${version}-${suffixes[index]}`;
    const asset = release.assets.find(a=>a.name===name); assert.match(asset?.digest??'', /^sha256:[a-f0-9]{64}$/);
    const directory = path.resolve('artifacts/core-material',version,target); fs.mkdirSync(directory,{recursive:true});
    const archive = path.join(directory,name);
    if (!fs.existsSync(archive) || hash(fs.readFileSync(archive))!==asset.digest.slice(7)) {
      const response = await fetch(asset.browser_download_url,{signal:AbortSignal.timeout(120000)}); assert.ok(response.ok);
      fs.writeFileSync(archive,Buffer.from(await response.arrayBuffer()));
    }
    const bytes=fs.readFileSync(archive); assert.equal(bytes.length,asset.size);assert.equal(hash(bytes),asset.digest.slice(7));
    const entries = execFileSync(tar,['-tf',archive],{encoding:'utf8'}).trim().split(/\r?\n/);
    assert.ok(entries.length<128);for(const entry of entries)assert.ok(!path.isAbsolute(entry)&&!entry.split(/[\\/]/).includes('..'));
    const programName = name.endsWith('.zip')?'sing-box.exe':'sing-box';
    const programs=entries.filter(e=>e.split('/').pop()===programName);assert.equal(programs.length,1);
    const program=execFileSync(tar,['-xOf',archive,programs[0]],{maxBuffer:256*1024*1024});
    platforms[target]={assetId:asset.id,archiveSha256:hash(bytes),archiveSize:bytes.length,programSha256:hash(program),programSize:program.length};
    console.log(`Verified official ${version} ${target}`);
  }
  return {purpose,schemaVersion:1,version,repository:'SagerNet/sing-box',platforms};
}
if(process.argv[1]?.replaceAll('\\','/').endsWith('/core-material.mjs')) {
  const [version,output] = process.argv.slice(2);assert.ok(output);
  const data=await collect(version);
  if(process.env.CORE_MATERIAL_UNSIGNED==='1'){
    fs.mkdirSync(path.dirname(output),{recursive:true});fs.writeFileSync(output,JSON.stringify(data,null,2)+'\n');
  }else{
    const key=createPrivateKey(process.env.CORE_SIGNING_PRIVATE_KEY??fs.readFileSync(path.join(process.env.USERPROFILE,'.codex/keys/sing-shell-core-ed25519.pem')));
    const envelope=signedMaterial(data,key);verifyMaterial(envelope,createPublicKey(key));
    fs.mkdirSync(path.dirname(output),{recursive:true});fs.writeFileSync(output,JSON.stringify(envelope,null,2)+'\n');
  }
}
