import assert from 'node:assert/strict';
import {spawnSync,execFileSync} from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {validate,updater,hash} from './package-artifacts.mjs';
import {verifyBinary} from './verify-binary.mjs';

const target=process.env.EXPECTED_RUST_TARGET;
assert.equal(target,`${process.arch==='arm64'?'aarch64':'x86_64'}-${{win32:'pc-windows-msvc',linux:'unknown-linux-gnu',darwin:'apple-darwin'}[process.platform]}`);
const input=path.resolve('artifacts/packages',target),candidateInput=path.resolve('artifacts/candidates',target);
const original=validate(input),candidate=validate(candidateInput);
assert.equal(original.target,target);assert.equal(candidate.target,target);assert.equal(original.source,candidate.source);
const version=JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json')).version;
assert.equal(original.version,version);
const parts=version.split('.').map(Number);parts[2]++;assert.equal(candidate.version,parts.join('.'));
const nativeHost={platform:process.platform,arch:process.arch,release:os.release()};
const directory=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'sing-shell-package-')));
const driver=path.resolve(`src-tauri/target/debug/upgrade-fixture${process.platform==='win32'?'.exe':''}`);
const report={version,target,source:original.source,toolSource:process.env.GITHUB_SHA??original.source,run:process.env.PACKAGE_RUN_ID??process.env.GITHUB_RUN_ID??null,verificationRun:process.env.GITHUB_RUN_ID??null,nativeHost,passed:false,packages:original.files,verified:[],skipped:['publisher code signing','macOS notarization','TUN and system proxy']};
function run(command,args,env={}){const result=spawnSync(command,args,{stdio:'inherit',windowsHide:true,timeout:300_000,env:{...process.env,...env}});if(result.error)throw result.error;assert.equal(result.status,0,`${command}: ${result.status}/${result.signal}`);}
function file(role,manifest=original,root=input){const found=manifest.files.filter(f=>f.role===role);assert.equal(found.length,1,`expected one ${role}`);return path.join(root,found[0].name);}
function smoke(executable,label,expected=version,env={}){verifyBinary(executable,target);run(process.execPath,['scripts/smoke.mjs',executable],{APPIMAGE_EXTRACT_AND_RUN:'1',WEBKIT_DISABLE_DMABUF_RENDERER:'1',EXPECTED_VERSION:expected,...env});report.verified.push(label);}
function appimageSmoke(executable,label,expected=version,env={}){run(process.execPath,['scripts/smoke.mjs',executable],{APPIMAGE_EXTRACT_AND_RUN:'1',WEBKIT_DISABLE_DMABUF_RENDERER:'1',EXPECTED_VERSION:expected,...env});report.verified.push(label);}
function driverInput(manifest,root,executable,data,verifyOnly){const update=updater(manifest);const name=path.join(directory,`driver-${verifyOnly?'verify':'install'}-${manifest.version}.json`);fs.writeFileSync(name,JSON.stringify({root:data,executable,package:path.join(root,update.name),signature:fs.readFileSync(path.join(root,update.name+'.sig'),'utf8').trim(),current:verifyOnly?'0.0.1':version,version:manifest.version,verifyOnly}));return name;}
function verifySignature(manifest,root){run(driver,[driverInput(manifest,root,driver,directory,true)]);}
let official;
function upgrade(executable,isImage=false){
  const data=path.join(directory,'upgrade-data');fs.mkdirSync(data,{recursive:true});const config=path.join(directory,'preserved config.json');fs.writeFileSync(config,'{"outbounds":[{"type":"direct"}]}');fs.writeFileSync(path.join(data,'settings.json'),JSON.stringify({corePath:official,configPath:config}));
  const env={SING_SHELL_TEST_DATA:data,EXPECTED_CONFIG:config};
  (isImage?appimageSmoke:smoke)(executable,'signed baseline starts with migrated legacy settings',version,env);
  run(driver,[driverInput(candidate,candidateInput,executable,data,false)],isImage?{APPIMAGE:executable,APPIMAGE_EXTRACT_AND_RUN:'1'}:{});
  // NSIS may finish in a child installer after the updater driver exits.
  let result;const deadline=Date.now()+60_000;
  do{result=spawnSync(process.execPath,['scripts/smoke.mjs',executable],{encoding:'utf8',windowsHide:true,timeout:35_000,env:{...process.env,...env,EXPECTED_VERSION:candidate.version,APPIMAGE_EXTRACT_AND_RUN:'1',WEBKIT_DISABLE_DMABUF_RENDERER:'1'}});if(result.status===0)break;Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0,500);}while(Date.now()<deadline);
  assert.equal(result.status,0,`upgraded executable did not confirm ${candidate.version}: ${result.stdout}\n${result.stderr}`);
  const saved=JSON.parse(fs.readFileSync(path.join(data,'settings.json')));assert.equal(saved.configPath,config);assert.deepEqual(saved.engine,{mode:'external',path:official});assert.equal(fs.readFileSync(config,'utf8'),'{"outbounds":[{"type":"direct"}]}');
  report.upgrade={from:version,to:candidate.version,signatureVerified:true,actualVersionConfirmed:true,settingsPreserved:true,candidateFiles:candidate.files};report.verified.push('official updater installs the higher signed candidate; actual version and user settings confirmed');
}
try{
  verifySignature(original,input);verifySignature(candidate,candidateInput);report.verified.push('baseline and upgrade-candidate Tauri signatures');
  const coreVersion='1.14.4',platform={win32:'windows',linux:'linux',darwin:'darwin'}[process.platform],arch=process.arch==='arm64'?'arm64':'amd64';const upstream=`sing-box-${coreVersion}-${platform}-${arch}`,archive=`${upstream}.${process.platform==='win32'?'zip':'tar.gz'}`;
  const expected={'windows-amd64':'99185d75d314a07dbba4a130c8ad986cbeb629a07927975636b341ca61f991df','windows-arm64':'8cd0a4d0cd8cd480be5731ac31f17038969a42d12add9feb6e031fd43d85bbae','linux-amd64':'699a1b510aa4396b2176d32d1181920399ebf1e267835da6f3b598d0f7c24968','linux-arm64':'436441f76ee2f7fe21b5b2d426d3cb261a480455a704865789ad533e4273ec26','darwin-arm64':'0d8c7f5ed5d587b0f1a5e9772a708ec3bfa4ac4c9329604b01d8d9c2ba406838'}[`${platform}-${arch}`];
  run('gh',['release','download',`v${coreVersion}`,'--repo','SagerNet/sing-box','--pattern',archive,'--dir',directory]);const coreArchive=path.join(directory,archive);assert.equal(hash(coreArchive),expected);run('tar',['-xf',coreArchive,'-C',directory]);official=path.join(directory,upstream,process.platform==='win32'?'sing-box.exe':'sing-box');verifyBinary(official,target);
  run('cargo',['test','--manifest-path','src-tauri/Cargo.toml','--locked','--test','real_core','--','--ignored'],{SING_SHELL_TEST_CORE:official,SING_SHELL_TEST_ARCHIVE:coreArchive});report.officialCore={version:coreVersion,archive,sha256:expected};report.verified.push('official sing-box SOCKS5 roundtrip, stop, managed preparation, config compatibility and tamper rejection');
  if(process.platform==='win32'){
    const install=path.join(directory,'installed');run(file('nsis'),['/S',`/D=${install}`]);try{const executable=path.join(install,'sing-shell.exe');smoke(executable,'NSIS installed executable: native architecture, WebView, IPC and exit');upgrade(executable);}finally{run(path.join(install,'uninstall.exe'),['/S',`_?=${install}`]);}
  }else if(process.platform==='linux'){
    const extracted=path.join(directory,'deb');run('dpkg-deb',['-x',file('deb'),extracted]);smoke(path.join(extracted,'usr/bin/sing-shell'),'DEB extracted executable: native architecture, WebView, IPC and exit');
    const image=path.join(directory,'installed.AppImage');fs.copyFileSync(file('appimage'),image);fs.chmodSync(image,0o755);appimageSmoke(image,'AppImage extraction, WebView, IPC and exit');upgrade(image,true);
    const packageName=execFileSync('dpkg-deb',['-f',file('deb'),'Package'],{encoding:'utf8'}).trim();assert.equal(packageName,'sing-shell');run('sudo',['dpkg','-i',file('deb')]);try{smoke('/usr/bin/sing-shell','DEB system installation');run('sudo',['dpkg','-i',file('deb',candidate,candidateInput)]);smoke('/usr/bin/sing-shell','DEB system upgrade',candidate.version);}finally{run('sudo',['dpkg','--remove',packageName]);}
  }else{
    const mount=path.join(directory,'mounted');run('hdiutil',['attach',file('dmg'),'-nobrowse','-readonly','-mountpoint',mount]);const installed=path.join(directory,'installed/Sing Shell.app');try{smoke(path.join(mount,'Sing Shell.app/Contents/MacOS/sing-shell'),'DMG mounted executable: native architecture, WebView, IPC and exit');run('ditto',[path.join(mount,'Sing Shell.app'),installed]);}finally{run('hdiutil',['detach',mount]);}
    const extracted=path.join(directory,'app');fs.mkdirSync(extracted);run('tar',['-xzf',file('app'),'-C',extracted]);smoke(path.join(extracted,'Sing Shell.app/Contents/MacOS/sing-shell'),'app archive executable: native architecture, WebView, IPC and exit');upgrade(path.join(installed,'Contents/MacOS/sing-shell'));
  }
  validate(input);validate(candidateInput);report.passed=true;
}catch(error){report.error=String(error.stack??error);throw error;}finally{fs.mkdirSync('artifacts/verification',{recursive:true});fs.writeFileSync(`artifacts/verification/${target}.json`,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));}
