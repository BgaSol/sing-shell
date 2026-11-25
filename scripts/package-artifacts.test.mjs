import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {validate,hash,updater} from './package-artifacts.mjs';
test('release metadata binds the original bytes and native updater role',()=>{
 const directory=fs.mkdtempSync(path.join(os.tmpdir(),'sing-shell-artifact-'));
 const binary=path.join(directory,'package.exe');fs.writeFileSync(binary,'verified bytes');
 const manifest={source:'a'.repeat(40),target:'aarch64-pc-windows-msvc',version:'0.2.0',files:[{name:'package.exe',role:'nsis',size:14,sha256:hash(binary)}]};
 fs.writeFileSync(path.join(directory,'package.json'),JSON.stringify(manifest));
 try{assert.equal(validate(directory).source,manifest.source);assert.equal(updater(manifest).name,'package.exe');fs.writeFileSync(binary,'tampered bytes');assert.throws(()=>validate(directory));}finally{fs.unlinkSync(binary);fs.unlinkSync(path.join(directory,'package.json'));fs.rmdirSync(directory);}
});
