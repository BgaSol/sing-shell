import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {createPublicKey,generateKeyPairSync} from 'node:crypto';
import {purpose,signedMaterial,verifyMaterial} from './core-material.mjs';
import {targets} from './targets.mjs';
test('core material signatures bind exact bytes and a separate purpose',()=>{
  const {privateKey,publicKey}=generateKeyPairSync('ed25519');
  const data={purpose,schemaVersion:1,platforms:Object.fromEntries(targets.map(t=>[t,{}]))};
  const envelope=signedMaterial(data,privateKey);assert.deepEqual(verifyMaterial(envelope,publicKey),data);
  assert.throws(()=>verifyMaterial({...envelope,payload:Buffer.from('{}').toString('base64')},publicKey));
  assert.throws(()=>verifyMaterial(signedMaterial({...data,purpose:'application'},privateKey),publicKey));
});
test('bundled versions use the committed core signing public key',()=>{
  const x=Buffer.from(fs.readFileSync('src-tauri/resources/cores/public-key.txt','utf8').trim(),'base64').toString('base64url');
  const key=createPublicKey({key:{kty:'OKP',crv:'Ed25519',x},format:'jwk'});
  for(const version of ['1.14.4','1.14.5']){
    const data=verifyMaterial(JSON.parse(fs.readFileSync(`src-tauri/resources/cores/${version}.json`)),key);assert.equal(data.version,version);
    for(const asset of Object.values(data.platforms)){assert.match(asset.archiveSha256,/^[a-f0-9]{64}$/);assert.match(asset.programSha256,/^[a-f0-9]{64}$/);assert.ok(asset.archiveSize>0&&asset.programSize>0);}
  }
});
