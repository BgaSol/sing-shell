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
function driverInput(manifest,root,executable,data,verifyOnly,currentVersion=version){const update=updater(manifest);const name=path.join(directory,`driver-${verifyOnly?'verify':'install'}-${manifest.version}.json`);fs.writeFileSync(name,JSON.stringify({root:data,executable,package:path.join(root,update.name),signature:fs.readFileSync(path.join(root,update.name+'.sig'),'utf8').trim(),current:verifyOnly?'0.0.1':currentVersion,version:manifest.version,verifyOnly}));return name;}
function verifySignature(manifest,root){run(driver,[driverInput(manifest,root,driver,directory,true)]);}
let official;
function upgrade(executable,isImage=false,destination=candidate,destinationRoot=candidateInput,from=version){
  const data=path.join(directory,`upgrade-data-${from}`);fs.mkdirSync(data,{recursive:true});const config=path.join(directory,`preserved config-${from}.json`);fs.writeFileSync(config,'{"outbounds":[{"type":"direct"}]}');fs.writeFileSync(path.join(data,'settings.json'),JSON.stringify({schemaVersion:1,revision:3,engine:{mode:'external',path:official},configPath:config,updatePreferences:{automatic:false,network:{revision:0,fallbackEnabled:true,disabledBuiltins:[],customMirrors:[]}}}));
  const env={SING_SHELL_TEST_DATA:data,EXPECTED_CONFIG:config,EXPECTED_LEGACY:from==='0.2.0'?'1':''};
  (isImage?appimageSmoke:smoke)(executable,'signed baseline starts with migrated legacy settings',from,env);
  run(driver,[driverInput(destination,destinationRoot,executable,data,false,from)],isImage?{APPIMAGE:executable,APPIMAGE_EXTRACT_AND_RUN:'1'}:{});
  if(process.platform==='win32'){
    // The official updater exits after ShellExecute; do not relaunch the old executable while NSIS replaces it.
    run('powershell.exe',['-NoProfile','-Command','$deadline=[DateTime]::UtcNow.AddSeconds(120); do { try { $version=[System.Diagnostics.FileVersionInfo]::GetVersionInfo($env:SING_SHELL_UPGRADE_EXECUTABLE).ProductVersion; if($version -eq $env:SING_SHELL_UPGRADE_VERSION){exit 0} } catch {} Start-Sleep -Milliseconds 250 } while([DateTime]::UtcNow -lt $deadline); throw "NSIS did not replace the executable within 120 seconds"'],{SING_SHELL_UPGRADE_EXECUTABLE:executable,SING_SHELL_UPGRADE_VERSION:destination.version});
  }
  // NSIS may finish in a child installer after the updater driver exits.
  let result;const deadline=Date.now()+60_000;
  do{result=spawnSync(process.execPath,['scripts/smoke.mjs',executable],{encoding:'utf8',windowsHide:true,timeout:35_000,env:{...process.env,...env,EXPECTED_VERSION:destination.version,EXPECTED_LEGACY:'',APPIMAGE_EXTRACT_AND_RUN:'1',WEBKIT_DISABLE_DMABUF_RENDERER:'1'}});if(result.status===0)break;Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0,500);}while(Date.now()<deadline);
  assert.equal(result.status,0,`upgraded executable did not confirm ${destination.version}: ${result.stdout}\n${result.stderr}`);
  const saved=JSON.parse(fs.readFileSync(path.join(data,'settings.json')));assert.equal(saved.configPath,config);assert.equal(saved.schemaVersion,2);assert.deepEqual(saved.coreSource,{mode:'external',path:official});assert.equal(saved.updatePreferences.automaticCore,false);assert.equal(fs.readFileSync(config,'utf8'),'{"outbounds":[{"type":"direct"}]}');
  report[from===version?'upgrade':'previousUpgrade']={from,to:destination.version,signatureVerified:true,actualVersionConfirmed:true,settingsPreserved:true,candidateFiles:destination.files};report.verified.push('official updater installs the higher signed candidate; actual version and user settings confirmed');
}
function publishedUpgrade(){
 const root=path.join(directory,'previous');fs.mkdirSync(root);
 const current=updater(original),name=current.name.replace(version,'0.2.0');
 const release=JSON.parse(execFileSync('gh',['api','repos/BgaSol/sing-shell/releases/tags/v0.2.0'],{encoding:'utf8'}));
 const files=[];for(const suffix of ['', '.sig']){const asset=release.assets.find(a=>a.name===name+suffix);assert.ok(asset);run('gh',['release','download','v0.2.0','--repo','BgaSol/sing-shell','--pattern',asset.name,'--dir',root]);const p=path.join(root,asset.name);assert.equal(`sha256:${hash(p)}`,asset.digest);files.push({name:asset.name,role:suffix?'signature':current.role,size:fs.statSync(p).size,sha256:hash(p)});}
 const previous={version:'0.2.0',target,source:release.target_commitish,files};verifySignature(previous,root);
 const installer=path.join(root,name);
 if(process.platform==='win32'){const installed=path.join(directory,'previous-installed');run(installer,['/S',`/D=${installed}`]);try{upgrade(path.join(installed,'sing-shell.exe'),false,original,input,'0.2.0');}finally{run(path.join(installed,'uninstall.exe'),['/S',`_?=${installed}`]);}}
 else if(process.platform==='linux'){const image=path.join(root,'installed.AppImage');fs.copyFileSync(installer,image);fs.chmodSync(image,0o755);upgrade(image,true,original,input,'0.2.0');}
 else{const extracted=path.join(root,'installed');fs.mkdirSync(extracted);run('tar',['-xzf',installer,'-C',extracted]);upgrade(path.join(extracted,'Sing Shell.app/Contents/MacOS/sing-shell'),false,original,input,'0.2.0');}
 report.verified.push('published signed v0.2.0 upgrades to this release with schema 1 migration and external selection preserved');
}

try{
  verifySignature(original,input);verifySignature(candidate,candidateInput);report.verified.push('baseline and upgrade-candidate Tauri signatures');
  const coreVersion='1.14.4',platform={win32:'windows',linux:'linux',darwin:'darwin'}[process.platform],arch=process.arch==='arm64'?'arm64':'amd64';const upstream=`sing-box-${coreVersion}-${platform}-${arch}`,archive=`${upstream}.${process.platform==='win32'?'zip':'tar.gz'}`;
  const expected={'windows-amd64':'99185d75d314a07dbba4a130c8ad986cbeb629a07927975636b341ca61f991df','windows-arm64':'8cd0a4d0cd8cd480be5731ac31f17038969a42d12add9feb6e031fd43d85bbae','linux-amd64':'699a1b510aa4396b2176d32d1181920399ebf1e267835da6f3b598d0f7c24968','linux-arm64':'436441f76ee2f7fe21b5b2d426d3cb261a480455a704865789ad533e4273ec26','darwin-arm64':'0d8c7f5ed5d587b0f1a5e9772a708ec3bfa4ac4c9329604b01d8d9c2ba406838'}[`${platform}-${arch}`];
  run('gh',['release','download',`v${coreVersion}`,'--repo','SagerNet/sing-box','--pattern',archive,'--dir',directory]);const coreArchive=path.join(directory,archive);assert.equal(hash(coreArchive),expected);run('tar',['-xf',coreArchive,'-C',directory]);official=path.join(directory,upstream,process.platform==='win32'?'sing-box.exe':'sing-box');verifyBinary(official,target);
  run('cargo',['test','--manifest-path','src-tauri/Cargo.toml','--locked','--features','test-fixtures','--test','real_core','--','--ignored'],{SING_SHELL_TEST_CORE:official,SING_SHELL_TEST_ARCHIVE:coreArchive});run('cargo',['test','--manifest-path','src-tauri/Cargo.toml','--locked','--features','test-fixtures','--test','managed_core']);report.verified.push('same-source managed core automation, compatibility, cancellation, rollback and crash recovery integration');report.officialCore={version:coreVersion,archive,sha256:expected};report.verified.push('official sing-box SOCKS5 roundtrip, stop, managed preparation, config compatibility and tamper rejection');
  publishedUpgrade();
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
