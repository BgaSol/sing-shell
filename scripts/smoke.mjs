import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import {createHash} from 'node:crypto';

const executable = path.resolve(process.argv[2] ?? `src-tauri/target/release/sing-shell${process.platform === 'win32' ? '.exe' : ''}`);
const ownedDirectory=!process.env.SING_SHELL_TEST_DATA;
const directory = process.env.SING_SHELL_TEST_DATA ?? await fs.mkdtemp(path.join(os.tmpdir(), 'sing-shell-smoke-'));
let completed = false;
const runningCore=process.env.SING_SHELL_RUNNING_CORE;
const installInput=process.env.SING_SHELL_INSTALL_INPUT?JSON.parse(await fs.readFile(process.env.SING_SHELL_INSTALL_INPUT,'utf8')):null;
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function waitFile(name){const deadline=Date.now()+60_000;for(;;){try{return JSON.parse(await fs.readFile(path.join(directory,name),'utf8'));}catch(e){if(e.code!=='ENOENT')throw e;}assert.ok(Date.now()<deadline,`timeout waiting for ${name}`);await delay(50);}}
async function roundtrip(port){
  const echo=net.createServer(socket=>socket.pipe(socket));await new Promise(resolve=>echo.listen(0,'127.0.0.1',resolve));
  const socket=net.connect(port,'127.0.0.1');socket.setTimeout(5000,()=>socket.destroy(new Error('SOCKS timeout')));
  const chunks=[];let failure;socket.on('data',data=>chunks.push(data));socket.on('error',e=>{failure=e;});
  async function read(size){const deadline=Date.now()+5000;while(chunks.reduce((n,b)=>n+b.length,0)<size){if(failure)throw failure;assert.ok(Date.now()<deadline,'SOCKS response timeout');await delay(10);}const all=Buffer.concat(chunks);chunks.length=0;if(all.length>size)chunks.push(all.subarray(size));return all.subarray(0,size);}
  try{
    await new Promise((resolve,reject)=>{socket.once('connect',resolve);socket.once('error',reject);});
    socket.write(Buffer.from([5,1,0]));assert.deepEqual(await read(2),Buffer.from([5,0]));
    const dest=echo.address().port;socket.write(Buffer.from([5,1,0,1,127,0,0,1,dest>>8,dest&255]));const head=await read(4);assert.equal(head[1],0);await read(head[3]===1?6:18);
    socket.write('sing-shell-hidden');assert.equal((await read(17)).toString(),'sing-shell-hidden');
  }finally{socket.destroy();echo.close();}
}
try {
  let port;
  if(runningCore){
    const listener=net.createServer();await new Promise(resolve=>listener.listen(0,'127.0.0.1',resolve));port=listener.address().port;await new Promise(resolve=>listener.close(resolve));
    const config=path.join(directory,'running core.json');await fs.writeFile(config,JSON.stringify({inbounds:[{type:'socks',listen:'127.0.0.1',listen_port:port}],outbounds:[{type:'direct'}]}));
    await fs.writeFile(path.join(directory,'settings.json'),JSON.stringify({schemaVersion:3,coreRevision:0,preferencesRevision:0,coreSource:{mode:'external',path:runningCore},managed:{active:null,previous:null,trial:null,blocked:null},configPath:config,pendingConfigPath:null,updatePreferences:{automatic:false,automaticCore:false,network:{revision:0,fallbackEnabled:false,disabledBuiltins:[],customMirrors:[]}},startup:{restoreCoreOnLaunch:true,resumeCore:true},desktop:{backgroundNoticeShown:true}}));
  }
  if(installInput){
    const cache=path.join(directory,'updates/app');await fs.mkdir(cache,{recursive:true});const bytes=await fs.readFile(installInput.package);await fs.writeFile(path.join(cache,'package.bin'),bytes);
    const candidate={component:'app',version:installInput.version,target:process.env.EXPECTED_RUST_TARGET,url:'https://example.invalid/isolated-signed-candidate',signature:installInput.signature,sha256:createHash('sha256').update(bytes).digest('hex'),size:bytes.length,notes:'isolated production lifecycle acceptance',releaseUrl:'https://example.invalid/isolated',assetName:'',assetId:0,tag:'v'+installInput.version};
    await fs.writeFile(path.join(cache,'prepared.json'),JSON.stringify({schema_version:1,candidate,from_version:process.env.EXPECTED_VERSION,engine_hash:null}));
  }
  const args=['--smoke-test','--test-data-dir',directory,...(runningCore?['--restore-test','--login-start']:[]),...(installInput?['--install-test']:[])];
  const child = spawn(executable, args, { stdio: 'inherit', windowsHide: true });
  const timer = setTimeout(() => { child.kill(); }, 90_000);
  try {
    const exited=new Promise((resolve, reject) => {
      child.on('error', reject);
      child.on('exit', (code, signal) => code === 0 ? resolve() : reject(new Error(`smoke exited: ${code ?? signal}`)));
    });
    if(runningCore){
      const hidden=await waitFile('smoke-hidden.json');assert.equal(hidden.hidden,true);assert.ok(hidden.pid);
      await roundtrip(port);
      const second=spawn(executable,['--smoke-test','--test-data-dir',directory],{windowsHide:true,stdio:'inherit'});
      await new Promise((resolve,reject)=>{second.on('error',reject);second.on('exit',code=>code===0?resolve():reject(Error(`second instance ${code}`)));});
      await roundtrip(port);
      await fs.writeFile(path.join(directory,'smoke-continue'),'continue');
    }
    await exited;
    if(installInput){const next=await waitFile(`smoke-result-${installInput.version}.json`);assert.equal(next.version,installInput.version);assert.ok(next.restoredPid);assert.equal(next.loginSource,false);assert.equal(next.settings.startup.restoreCoreOnLaunch,true);assert.equal(next.settings.startup.resumeCore,true);assert.equal(next.installThroughLifecycle,false);}
  } finally { clearTimeout(timer); }
  const report = JSON.parse(await fs.readFile(path.join(directory,installInput?`smoke-result-${process.env.EXPECTED_VERSION}.json`:'smoke-result.json'), 'utf8'));
  assert.equal(report.completed, true);
  if(installInput)assert.equal(report.installThroughLifecycle,true);
  assert.equal(report.frontendReady, true);
  assert.equal(report.closeRequestIntercepted, true);
  if(runningCore){assert.ok(report.restoredPid);assert.equal(report.reactivated,true);assert.equal(report.restartArgsNormalized,true);assert.equal(report.settings.startup.resumeCore,true);const listener=net.createServer();await new Promise((resolve,reject)=>{listener.once('error',reject);listener.listen(port,'127.0.0.1',resolve);});await new Promise(resolve=>listener.close(resolve));}
  if(!process.env.EXPECTED_LEGACY) assert.equal(report.settings.schemaVersion,3);
  if(!process.env.EXPECTED_LEGACY){assert.equal(report.draftRetained,true);assert.equal(report.loginIO,true);if(process.platform!=='linux')assert.equal(report.trayAvailable,true);}
  if(process.env.EXPECTED_VERSION) assert.equal(report.version,process.env.EXPECTED_VERSION);
  if(process.env.EXPECTED_RUST_TARGET) assert.equal(report.target,process.env.EXPECTED_RUST_TARGET);
  if(process.env.EXPECTED_CONFIG) assert.equal(report.settings.configPath,process.env.EXPECTED_CONFIG);
  console.log(JSON.stringify(report));
  completed = true;
} finally {
  if (completed && ownedDirectory) {
    assert.ok(directory.startsWith(path.join(os.tmpdir(), 'sing-shell-smoke-')));
    await fs.rm(directory, { recursive: true, force: true });
  }
  else if(!completed) console.error(`Smoke diagnostics: ${directory}`);
}
