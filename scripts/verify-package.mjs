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
const report={version,target,source:original.source,toolSource:process.env.GITHUB_SHA??original.source,run:process.env.PACKAGE_RUN_ID??process.env.GITHUB_RUN_ID??null,verificationRun:process.env.GITHUB_RUN_ID??null,nativeHost,passed:false,packages:original.files,verified:[],skipped:['publisher code signing','macOS notarization','system proxy is outside product scope','interactive UAC/polkit authorization','macOS launchd activation after user approval','suspend and physical network switching','actual desktop logout/login','physical tray visibility and macOS approval UI']};
function run(command,args,env={}){const result=spawnSync(command,args,{stdio:'inherit',windowsHide:true,windowsVerbatimArguments:process.platform==='win32'&&path.basename(command).toLowerCase()==='uninstall.exe',timeout:300_000,env:{...process.env,...env}});if(result.error)throw result.error;assert.equal(result.status,0,`${command}: ${result.status}/${result.signal}`);}
function file(role,manifest=original,root=input){const found=manifest.files.filter(f=>f.role===role);assert.equal(found.length,1,`expected one ${role}`);return path.join(root,found[0].name);}
fs.mkdirSync('artifacts/verification',{recursive:true});
const desktopEvidence=path.resolve('artifacts/verification',target+'-desktop.jsonl');
function smoke(executable,label,expected=version,env={}){verifyBinary(executable,target);run(process.execPath,['scripts/smoke.mjs',executable],{APPIMAGE_EXTRACT_AND_RUN:'1',WEBKIT_DISABLE_DMABUF_RENDERER:'1',EXPECTED_VERSION:expected,SING_SHELL_SMOKE_REPORT:desktopEvidence,...env});report.verified.push(label);}
function appimageSmoke(executable,label,expected=version,env={}){run(process.execPath,['scripts/smoke.mjs',executable],{APPIMAGE_EXTRACT_AND_RUN:'1',WEBKIT_DISABLE_DMABUF_RENDERER:'1',EXPECTED_VERSION:expected,...env});report.verified.push(label);}
function driverInput(manifest,root,executable,data,verifyOnly,currentVersion=version){const update=updater(manifest);const name=path.join(directory,`driver-${verifyOnly?'verify':'install'}-${manifest.version}.json`);fs.writeFileSync(name,JSON.stringify({root:data,executable,package:path.join(root,update.name),signature:fs.readFileSync(path.join(root,update.name+'.sig'),'utf8').trim(),current:verifyOnly?'0.0.1':currentVersion,version:manifest.version,verifyOnly}));return name;}
function verifySignature(manifest,root){run(driver,[driverInput(manifest,root,driver,directory,true)]);}
let official;
function networkPackage(executable,label){
  const env={SING_SHELL_NETWORK_PREFLIGHT:'1',SING_SHELL_NETWORK_EXECUTABLE:executable,SING_SHELL_NETWORK_CORE:official,APPIMAGE_EXTRACT_AND_RUN:'1'};
  if(process.platform==='win32')run(process.execPath,['scripts/network-integration.mjs',target],env);
  else run('sudo',['-E',process.execPath,'scripts/network-integration.mjs',target],env);
  const result=JSON.parse(fs.readFileSync(path.join('artifacts/network-preflight',target,'integration-report.json')));assert.equal(result.passed,true);report.networkIntegration=result;report.verified.push(label);
}
function ruleResources(root){
  const entries=fs.readdirSync(root,{recursive:true,withFileTypes:true});
  const manifests=entries.filter(e=>e.isFile()&&e.name==='manifest.json'&&path.basename(e.parentPath)==='rules');
  assert.equal(manifests.length,1,'bundled rules manifest');
  const rules=manifests[0].parentPath,manifest=JSON.parse(fs.readFileSync(path.join(rules,'manifest.json')));
  assert.equal(manifest.commit,'8b8788649349cf73fcd12fc81a5cae580e60360c');
  for(const asset of manifest.rules){assert.equal(hash(path.join(rules,asset.file)),asset.sha256);assert.ok(fs.existsSync(path.join(rules,'source',asset.id+'.json')));}
  for(const name of ['LICENSE','COPYING','NOTICE.md'])assert.ok(fs.statSync(path.join(rules,name)).size>100);
  report.verified.push('installed resources include checked SRS, editable rule sources and complete upstream GPL notices');
}

function upgrade(executable,isImage=false,destination=candidate,destinationRoot=candidateInput,from=version){
  const data=path.join(directory,`upgrade-data-${from}`);fs.mkdirSync(data,{recursive:true});const config=path.join(directory,`preserved config-${from}.json`);fs.writeFileSync(config,'{"outbounds":[{"type":"direct"}]}');const network={revision:0,fallbackEnabled:true,disabledBuiltins:[],customMirrors:[]};const savedSettings=from==='0.2.0'?{schemaVersion:1,revision:3,engine:{mode:'external',path:official},configPath:config,updatePreferences:{automatic:false,network}}:{schemaVersion:2,coreRevision:3,preferencesRevision:2,coreSource:{mode:'external',path:official},managed:{active:null,previous:null,trial:null,blocked:null},configPath:config,pendingConfigPath:null,updatePreferences:{automatic:false,automaticCore:false,network}};
  if(!['0.2.0','0.3.0','0.4.0'].includes(from)){
    const node={id:'preserved-vless',name:'Existing VLESS',source:'本地导入',spec:{type:'vless',server:'example.com',server_port:443,uuid:'550e8400-e29b-41d4-a716-446655440000',tls:{enabled:true,server_name:'tls.example.com'},transport:{type:'ws',path:'/wsss',headers:{Host:'cdn.example.com'}}}};
    Object.assign(savedSettings,{schemaVersion:4,profileRevision:7,profile:{mode:'externalConfig',nodes:[node],selectedNode:node.id,ruleCommit:'8b8788649349cf73fcd12fc81a5cae580e60360c',routing:{defaultAction:'proxy',customRules:[],siteRules:[]}},startup:{restoreCoreOnLaunch:false,resumeCore:false,lastConnection:null},desktop:{backgroundNoticeShown:true}});
  }
  fs.writeFileSync(path.join(data,'settings.json'),JSON.stringify(savedSettings));
  const env={SING_SHELL_TEST_DATA:data,EXPECTED_CONFIG:config,EXPECTED_LEGACY:['0.2.0','0.3.0','0.4.0'].includes(from)?'1':''};
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
  const saved=JSON.parse(fs.readFileSync(path.join(data,'settings.json')));assert.equal(saved.configPath,config);assert.equal(saved.schemaVersion,4);assert.deepEqual(saved.coreSource,{mode:'external',path:official});assert.equal(saved.updatePreferences.automaticCore,false);assert.equal(fs.readFileSync(config,'utf8'),'{"outbounds":[{"type":"direct"}]}');
  if(savedSettings.profile){assert.deepEqual(saved.profile,savedSettings.profile);assert.equal(saved.profileRevision,7);report.verified.push(`existing VLESS credentials, TLS, transport, selection and routing preserved from ${from}`);}
  (report.previousUpgrades??=[]).push({from,to:destination.version,signatureVerified:true,actualVersionConfirmed:true,settingsPreserved:true,nodePreserved:!!savedSettings.profile,candidateFiles:destination.files});report.verified.push('official updater installs the higher signed candidate; actual version and user settings confirmed');
}
function windowsLogin(action,executable){
  assert.equal(process.env.GITHUB_ACTIONS,'true');
  const code=String.raw`$ErrorActionPreference='Stop';$run='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run';$approved='HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run';$name='com.bgasol.singshell';$expected='"'+$env:SING_SHELL_LOGIN_EXE+'" --login-start';switch($env:SING_SHELL_LOGIN_ACTION){'seed'{New-Item -Path $run -Force|Out-Null;New-Item -Path $approved -Force|Out-Null;New-ItemProperty -Path $run -Name $name -Value $expected -PropertyType String -Force|Out-Null;New-ItemProperty -Path $approved -Name $name -Value ([byte[]](3,0,0,0,0,0,0,0,0,0,0,0)) -PropertyType Binary -Force|Out-Null};'preserved'{if((Get-ItemPropertyValue -Path $run -Name $name) -ne $expected){throw 'Run registration changed during update'};if((Get-ItemPropertyValue -Path $approved -Name $name)[0] -ne 3){throw 'System disabled registration was overwritten'}};'removed'{if(Get-ItemProperty -Path $run -Name $name -ErrorAction SilentlyContinue){throw 'Owned login registration survived uninstall'};Remove-ItemProperty -Path $approved -Name $name -ErrorAction SilentlyContinue};'foreign'{if((Get-ItemPropertyValue -Path $run -Name $name) -ne $expected){throw 'Foreign installation registration changed'};Remove-ItemProperty -Path $run -Name $name;Remove-ItemProperty -Path $approved -Name $name -ErrorAction SilentlyContinue}}`;
  run('powershell.exe',['-NoProfile','-Command',code],{SING_SHELL_LOGIN_ACTION:action,SING_SHELL_LOGIN_EXE:executable});
}
function windowsInstallation(executable,scope){
  const code=String.raw`$ErrorActionPreference='Stop';$hive=if($env:SING_SHELL_INSTALL_SCOPE -eq '/AllUsers'){'HKLM:'}else{'HKCU:'};$other=if($hive -eq 'HKLM:'){'HKCU:'}else{'HKLM:'};$key='\Software\Microsoft\Windows\CurrentVersion\Uninstall\Sing Shell';$expected=Split-Path -Parent $env:SING_SHELL_LOGIN_EXE;$location=(Get-ItemPropertyValue -LiteralPath ($hive+$key) -Name InstallLocation).Trim('"');if($location -ne $expected){throw 'Installation scope or path changed'};if(Test-Path -LiteralPath ($other+$key)){throw 'Duplicate installation in the other scope'}`;
  run('powershell.exe',['-NoProfile','-Command',code],{SING_SHELL_INSTALL_SCOPE:scope,SING_SHELL_LOGIN_EXE:executable});
}
function registrationSmoke(executable,isImage=false){
  const data=path.join(directory,'registration-data');fs.mkdirSync(data,{recursive:true});
  const env={SING_SHELL_TEST_DATA:data,SING_SHELL_VERIFY_REGISTRATION:'1',XDG_CONFIG_HOME:path.join(data,'xdg')};
  (isImage?appimageSmoke:smoke)(executable,'native current-user login registration, read-back, system-disabled facts and cleanup',version,env);
}
function appimageRegistrationSmoke(image){
  assert.equal(process.env.GITHUB_ACTIONS,'true');
  const root=fs.mkdtempSync(path.join(os.homedir(),'.sing-shell-ci-')),stable=path.join(root,'Sing Shell.AppImage');
  try{fs.copyFileSync(image,stable);fs.chmodSync(stable,0o755);registrationSmoke(stable,true);}
  finally{assert.equal(path.dirname(root),os.homedir());assert.ok(path.basename(root).startsWith('.sing-shell-ci-'));fs.rmSync(root,{recursive:true,force:true});}
}
function productionUpgrade(executable,isImage=false){
  const update=updater(candidate),info=path.join(directory,'production-install.json');
  fs.writeFileSync(info,JSON.stringify({version:candidate.version,package:path.join(candidateInput,update.name),signature:fs.readFileSync(path.join(candidateInput,update.name+'.sig'),'utf8').trim()}));
  (isImage?appimageSmoke:smoke)(executable,'production lifecycle: login launch, core restoration, preflight, resource cleanup, signed install and restarted core restoration',version,{SING_SHELL_RUNNING_CORE:official,SING_SHELL_INSTALL_INPUT:info});
  report.upgrade={from:version,to:candidate.version,signatureVerified:true,actualVersionConfirmed:true,settingsPreserved:true,productionLifecycle:true,restoredCore:true,loginArgumentConsumed:true,candidateFiles:candidate.files};
}
function publishedUpgrade(previousVersion){
 const root=path.join(directory,'previous-'+previousVersion);fs.mkdirSync(root);
 const current=updater(original),name=current.name.replace(version,previousVersion);
 const release=JSON.parse(execFileSync('gh',['api','repos/BgaSol/sing-shell/releases/tags/v'+previousVersion],{encoding:'utf8'}));
 const files=[];for(const suffix of ['', '.sig']){const asset=release.assets.find(a=>a.name===name+suffix);assert.ok(asset);run('gh',['release','download','v'+previousVersion,'--repo','BgaSol/sing-shell','--pattern',asset.name,'--dir',root]);const p=path.join(root,asset.name);assert.equal(`sha256:${hash(p)}`,asset.digest);files.push({name:asset.name,role:suffix?'signature':current.role,size:fs.statSync(p).size,sha256:hash(p)});}
 const previous={version:previousVersion,target,source:release.target_commitish,files};verifySignature(previous,root);
 const installer=path.join(root,name);
 if(process.platform==='win32'){const installed=path.join(directory,'previous-installed'),executable=path.join(installed,'sing-shell.exe');run(installer,['/S',`/D=${installed}`]);try{windowsInstallation(executable,'/CurrentUser');windowsLogin('seed',executable);upgrade(executable,false,original,input,previousVersion);windowsInstallation(executable,'/CurrentUser');windowsLogin('preserved',executable);}finally{run(path.join(installed,'uninstall.exe'),['/S',`_?=${installed}`]);}windowsLogin('removed',executable);}
 else if(process.platform==='linux'){const image=path.join(root,'installed.AppImage');fs.copyFileSync(installer,image);fs.chmodSync(image,0o755);upgrade(image,true,original,input,previousVersion);}
 else{const extracted=path.join(root,'installed');fs.mkdirSync(extracted);run('tar',['-xzf',installer,'-C',extracted]);upgrade(path.join(extracted,'Sing Shell.app/Contents/MacOS/sing-shell'),false,original,input,previousVersion);}
 report.verified.push(`published signed v${previousVersion} upgrades to this release with schema migration and external selection preserved`);
}

try{
  verifySignature(original,input);verifySignature(candidate,candidateInput);report.verified.push('baseline and upgrade-candidate Tauri signatures');
  const coreVersion='1.14.4',platform={win32:'windows',linux:'linux',darwin:'darwin'}[process.platform],arch=process.arch==='arm64'?'arm64':'amd64';const upstream=`sing-box-${coreVersion}-${platform}-${arch}`,archive=`${upstream}.${process.platform==='win32'?'zip':'tar.gz'}`;
  const expected={'windows-amd64':'99185d75d314a07dbba4a130c8ad986cbeb629a07927975636b341ca61f991df','windows-arm64':'8cd0a4d0cd8cd480be5731ac31f17038969a42d12add9feb6e031fd43d85bbae','linux-amd64':'699a1b510aa4396b2176d32d1181920399ebf1e267835da6f3b598d0f7c24968','linux-arm64':'436441f76ee2f7fe21b5b2d426d3cb261a480455a704865789ad533e4273ec26','darwin-arm64':'0d8c7f5ed5d587b0f1a5e9772a708ec3bfa4ac4c9329604b01d8d9c2ba406838'}[`${platform}-${arch}`];
  run('gh',['release','download',`v${coreVersion}`,'--repo','SagerNet/sing-box','--pattern',archive,'--dir',directory]);const coreArchive=path.join(directory,archive);assert.equal(hash(coreArchive),expected);run('tar',['-xf',coreArchive,'-C',directory]);official=path.join(directory,upstream,process.platform==='win32'?'sing-box.exe':'sing-box');verifyBinary(official,target);
  run('cargo',['test','--manifest-path','src-tauri/Cargo.toml','--locked','--features','test-fixtures','--test','real_core','--','--ignored'],{SING_SHELL_TEST_CORE:official,SING_SHELL_TEST_ARCHIVE:coreArchive});run('cargo',['test','--manifest-path','src-tauri/Cargo.toml','--locked','--features','test-fixtures','--test','managed_core']);run('cargo',['test','--manifest-path','src-tauri/Cargo.toml','--locked','--features','test-fixtures','--test','profile','--','--include-ignored'],{SING_SHELL_TEST_CORE:official});report.verified.push('same-source managed core automation, compatibility, cancellation, rollback and crash recovery integration');report.officialCore={version:coreVersion,archive,sha256:expected};report.verified.push('official sing-box SOCKS5 roundtrip, stop, managed preparation, config compatibility and tamper rejection');
  publishedUpgrade('0.2.0');publishedUpgrade('0.3.0');publishedUpgrade('0.4.0');publishedUpgrade('0.5.0');
  if(process.platform==='win32'){
    for(const scope of ['/AllUsers','/CurrentUser']){
      const install=scope==='/AllUsers'?path.join(process.env.ProgramFiles,`Sing Shell CI-${process.pid}`):path.join(directory,'installed');
      assert.ok(!fs.existsSync(install),'isolated installation directory must not already exist');
      const executable=path.join(install,'sing-shell.exe');
      run(file('nsis'),['/S',scope,`/D=${install}`]);
      try{
        windowsInstallation(executable,scope);ruleResources(install);
        networkPackage(executable,`actual NSIS ${scope} application: authorized worker, encrypted IPv4/IPv6 TCP/UDP TUN forwarding and network cleanup`);
        registrationSmoke(executable);windowsLogin('seed',executable);
        smoke(executable,`NSIS ${scope} installed executable: native architecture, WebView, IPC and exit`);
        smoke(executable,'running official core survives hide and second-instance activation, then exits and releases its port',version,{SING_SHELL_RUNNING_CORE:official});
        productionUpgrade(executable);windowsInstallation(executable,scope);windowsLogin('preserved',executable);
      }finally{run(path.join(install,'uninstall.exe'),['/S',scope,`_?=${install}`]);}
      windowsLogin('removed',executable);
      report.verified.push(`NSIS ${scope} update preserves scope, path, login registration and Windows disabled state; uninstall removes only owned registration`);
      run(file('nsis'),['/S',scope,`/D=${install}`]);
      const foreign=path.join(directory,'another installation/sing-shell.exe');windowsLogin('seed',foreign);
      run(path.join(install,'uninstall.exe'),['/S',scope,`_?=${install}`]);windowsLogin('foreign',foreign);
      report.verified.push(`NSIS ${scope} uninstall preserves another installation login registration`);
    }
  }else if(process.platform==='linux'){
    const extracted=path.join(directory,'deb');run('dpkg-deb',['-x',file('deb'),extracted]);ruleResources(extracted);smoke(path.join(extracted,'usr/bin/sing-shell'),'DEB extracted executable: native architecture, WebView, IPC and exit');networkPackage(path.join(extracted,'usr/bin/sing-shell'),'actual DEB executable: authorized worker and encrypted TUN forwarding');
    const image=path.join(directory,'installed.AppImage');fs.copyFileSync(file('appimage'),image);fs.chmodSync(image,0o755);const imageResources=path.join(directory,'image-resources');fs.mkdirSync(imageResources);const unpack=spawnSync(image,['--appimage-extract'],{cwd:imageResources,stdio:'ignore',timeout:60_000});assert.equal(unpack.status,0);ruleResources(path.join(imageResources,'squashfs-root'));networkPackage(image,'actual AppImage executable: authorized worker and encrypted TUN forwarding');appimageRegistrationSmoke(image);appimageSmoke(image,'AppImage extraction, WebView, IPC and exit');appimageSmoke(image,'running official core survives hide and second-instance activation, then releases its port',version,{SING_SHELL_RUNNING_CORE:official});productionUpgrade(image,true);
    const packageName=execFileSync('dpkg-deb',['-f',file('deb'),'Package'],{encoding:'utf8'}).trim();assert.equal(packageName,'sing-shell');run('sudo',['dpkg','-i',file('deb')]);try{registrationSmoke('/usr/bin/sing-shell');smoke('/usr/bin/sing-shell','DEB system installation');run('sudo',['dpkg','-i',file('deb',candidate,candidateInput)]);smoke('/usr/bin/sing-shell','DEB system upgrade',candidate.version);}finally{run('sudo',['dpkg','--remove',packageName]);}
  }else{
    const mount=path.join(directory,'mounted');run('hdiutil',['attach',file('dmg'),'-nobrowse','-readonly','-mountpoint',mount]);const installed=path.join(directory,'installed/Sing Shell.app');try{smoke(path.join(mount,'Sing Shell.app/Contents/MacOS/sing-shell'),'DMG mounted executable: native architecture, WebView, IPC and exit');run('ditto',[path.join(mount,'Sing Shell.app'),installed]);}finally{run('hdiutil',['detach',mount]);}
    const extracted=path.join(directory,'app');fs.mkdirSync(extracted);run('tar',['-xzf',file('app'),'-C',extracted]);smoke(path.join(extracted,'Sing Shell.app/Contents/MacOS/sing-shell'),'app archive executable: native architecture, WebView, IPC and exit');const loginBundle=path.join(os.homedir(),'Applications',`Sing Shell CI-${process.pid}.app`);fs.mkdirSync(path.dirname(loginBundle),{recursive:true});assert.ok(!fs.existsSync(loginBundle));run('ditto',[installed,loginBundle]);try{run('/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister',['-f',loginBundle]);registrationSmoke(path.join(loginBundle,'Contents/MacOS/sing-shell'));}finally{assert.equal(path.dirname(loginBundle),path.join(os.homedir(),'Applications'));assert.equal(path.basename(loginBundle),`Sing Shell CI-${process.pid}.app`);fs.rmSync(loginBundle,{recursive:true,force:true});}
    ruleResources(installed);assert.ok(fs.existsSync(path.join(installed,'Contents/Library/LaunchDaemons/com.bgasol.singshell.network.plist')));networkPackage(path.join(installed,'Contents/MacOS/sing-shell'),'actual macOS app bundle: privileged worker, encrypted IPv4/IPv6 TCP/UDP and TUN cleanup; launchd approval separately scoped');smoke(path.join(installed,'Contents/MacOS/sing-shell'),'running official core survives hide and second-instance activation, then releases its port',version,{SING_SHELL_RUNNING_CORE:official});productionUpgrade(path.join(installed,'Contents/MacOS/sing-shell'));
  }
  validate(input);validate(candidateInput);report.passed=true;
}catch(error){report.error=String(error.stack??error);throw error;}finally{if(fs.existsSync(desktopEvidence)){report.desktopEvidence=fs.readFileSync(desktopEvidence,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse);if(report.desktopEvidence.some(s=>s.loginRegistration?.registered?.state==='blocked'))report.skipped.push('macOS main-app login registration requires system approval; registration read-back and removal verified');if(report.desktopEvidence.some(s=>s.backgroundFallback))report.skipped.push('tray host unavailable: visible foreground fallback verified');}fs.mkdirSync('artifacts/verification',{recursive:true});fs.writeFileSync(`artifacts/verification/${target}.json`,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));}
