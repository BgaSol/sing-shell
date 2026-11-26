import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
import dgram from 'node:dgram';
import os from 'node:os';
import {spawn,spawnSync} from 'node:child_process';
import {echoPair} from './network-echo.mjs';

assert.equal(process.env.SING_SHELL_NETWORK_PREFLIGHT,'1','disposable native runner only');
const target=process.argv[2],directory=path.resolve('artifacts/network-preflight',target),control=await fs.mkdtemp(path.join(os.tmpdir(),'ss-net-'));
await fs.mkdir(directory,{recursive:true});
const suffix=process.platform==='win32'?'.exe':'';
const executable=process.env.SING_SHELL_NETWORK_EXECUTABLE??path.resolve(`src-tauri/target/release/network-fixture${suffix}`);
const core=process.env.SING_SHELL_NETWORK_CORE??path.join(directory,`sing-box-1.14.4-${process.platform==='win32'?'windows':process.platform==='darwin'?'darwin':'linux'}-${process.arch==='arm64'?'arm64':'amd64'}`,`sing-box${suffix}`);
const report={target,passed:false,verified:[],skipped:['interactive UAC/polkit approval','macOS launchd activation after user approval','suspend and physical network switching']};
const timeout=setTimeout(async()=>{report.error='integration deadline exceeded';report.log=logs.slice(-32768);if(worker)worker.kill();if(proxy)proxy.kill();await fs.writeFile(path.join(directory,'integration-report.json'),JSON.stringify(report,null,2));console.error(JSON.stringify(report));process.exit(1);},240_000);
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function waitFile(name){const deadline=Date.now()+210_000;for(;;){try{return JSON.parse(await fs.readFile(path.join(control,name),'utf8'));}catch(e){if(e.code!=='ENOENT')throw e;}assert.ok(Date.now()<deadline,`timeout ${name}`);await delay(100);}}
async function tcpEcho(host,port){await new Promise((resolve,reject)=>{const socket=net.connect({host,port});socket.setTimeout(8000,()=>socket.destroy(Error('TCP tunnel timeout')));socket.on('error',reject);socket.once('connect',()=>socket.write('sing-shell-proxy'));let data='';socket.on('data',chunk=>{data+=chunk;if(data==='sing-shell-proxy'){socket.end();resolve();}});});}
async function udpEcho(host,port){const socket=dgram.createSocket(net.isIP(host)===6?'udp6':'udp4');try{await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('UDP tunnel timeout')),8000);socket.once('error',e=>{clearTimeout(timer);reject(e);});socket.once('message',message=>{clearTimeout(timer);try{assert.equal(message.toString(),'sing-shell-udp');resolve();}catch(e){reject(e);}});socket.send('sing-shell-udp',port,host);});}finally{socket.close();}}
function authorizedPid(parent){
  const command=process.platform==='win32'?spawnSync('powershell.exe',['-NoProfile','-Command',`@(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -match '--network-worker ' -and $_.CommandLine.TrimEnd().EndsWith(' ${parent}') } | Select-Object -ExpandProperty ProcessId) | ConvertTo-Json -Compress`],{encoding:'utf8',windowsHide:true}):spawnSync('/bin/ps',['-axo','pid=,command='],{encoding:'utf8'});
  assert.equal(command.status,0);
  const matches=process.platform==='win32'?[JSON.parse(command.stdout)].flat():command.stdout.split('\n').filter(line=>line.includes('--network-worker ')&&new RegExp(`\\s${parent}\\s*$`).test(line)).map(line=>Number(line.trim().split(/\s+/)[0]));
  assert.equal(matches.length,1,'one authenticated worker owned by the fixture controller');
  assert.ok(Number.isInteger(matches[0])&&matches[0]>0&&matches[0]!==parent);return matches[0];
}
let proxy,worker,tcp,udp;
let logs='';
try{
  const echo=await echoPair();tcp=echo.tcp;udp=echo.udp;const echoPort=echo.port;
  const holder=net.createServer();await new Promise(resolve=>holder.listen(0,'127.0.0.1',resolve));const proxyPort=holder.address().port;await new Promise(resolve=>holder.close(resolve));
  const password='sing-shell-fixture-password';
  const serverConfig={log:{level:'warn'},inbounds:[{type:'shadowsocks',tag:'ss-in',listen:'127.0.0.1',listen_port:proxyPort,method:'aes-128-gcm',password}],outbounds:[{type:'direct',tag:'echo'}],route:{rules:[{ip_cidr:['198.18.77.1/32','2001:db8:198::99/128'],action:'route',outbound:'echo',override_address:'127.0.0.1',override_port:echoPort}],final:'echo'}};
  const config=path.join(directory,'proxy-server.json');await fs.writeFile(config,JSON.stringify(serverConfig));
  proxy=spawn(core,['run','-c',config],{stdio:['ignore','pipe','pipe'],windowsHide:true});proxy.stdout.on('data',bytes=>{logs+=bytes;});proxy.stderr.on('data',bytes=>{logs+=bytes;});await delay(700);assert.equal(proxy.exitCode,null,'controlled proxy stays alive');
  const input={node:{id:'test-node',name:'Controlled Shadowsocks',source:'fixture',spec:{type:'shadowsocks',server:'127.0.0.1',server_port:proxyPort,method:'aes-128-gcm',password}},routing:{defaultAction:'proxy',customRules:[],siteRules:[{site:'github',action:'direct'}]},coreVersion:'1.14.4',control};
  const inputFile=path.join(directory,'worker-input.json');await fs.writeFile(inputFile,JSON.stringify(input));
  async function launch(){
    for(const file of ['started.json','stopped.json','stop'])await fs.rm(path.join(control,file),{force:true});
    worker=spawn(executable,process.env.SING_SHELL_NETWORK_EXECUTABLE?['--smoke-test','--network-fixture-input',inputFile]:[inputFile],{stdio:['ignore','pipe','pipe'],windowsHide:true});worker.stdout.on('data',bytes=>{logs=(logs+bytes).slice(-65536);});worker.stderr.on('data',bytes=>{logs=(logs+bytes).slice(-65536);});
    return Promise.race([waitFile('started.json'),new Promise((_,reject)=>{worker.once('error',reject);worker.once('exit',code=>reject(Error(`worker exited ${code}: ${logs}`)));})]);
  }
  console.log('Waiting for authorized worker to prepare the official core and TUN');
  const started=await launch();
  console.log('Authorized TUN running; verifying selected-node traffic');
  assert.equal(started.phase,'running');assert.ok(started.runtime.pid&&started.network.interface&&started.network.ipv4&&started.network.ipv6&&started.network.routes);
  report.verified.push('production network worker authenticates control channel and independently verifies official core','typed node and routing become checked private TUN config','native interface and IPv4/IPv6 routes are confirmed');
  for(const address of ['198.18.77.1','2001:db8:198::99']){await tcpEcho(address,echoPort);await udpEcho(address,echoPort);report.verified.push(`TCP and UDP ${net.isIP(address)===6?'IPv6':'IPv4'} forwarded through selected encrypted Shadowsocks node`);}
  await tcpEcho('127.0.0.1',echoPort);report.verified.push('loopback bypass remains direct');
  await fs.writeFile(path.join(control,'stop'),'stop');const stopped=await waitFile('stopped.json');assert.equal(stopped.runtime.pid,null);assert.equal(stopped.phase,'stopped');
  const exit=await new Promise(resolve=>{if(worker.exitCode!==null)resolve(worker.exitCode);else worker.once('exit',resolve);});assert.equal(exit,0);worker=null;
  assert.ok(!JSON.stringify(started).includes(password));report.verified.push('stop releases actual TUN and terminates authorization worker','worker diagnostics do not return known node credential');
  const interrupted=await launch();process.kill(interrupted.runtime.pid,'SIGKILL');await delay(2000);
  await fs.writeFile(path.join(control,'stop'),'stop');assert.equal((await waitFile('stopped.json')).phase,'stopped');
  assert.equal(await new Promise(resolve=>worker.exitCode!==null?resolve(worker.exitCode):worker.once('exit',resolve)),0);worker=null;
  report.verified.push('unexpected official core exit is cleaned without crash reconnection');
  await launch();worker.kill('SIGKILL');await new Promise(resolve=>worker.exitCode!==null?resolve():worker.once('exit',resolve));worker=null;await delay(12_000);
  const recovered=await launch();assert.equal(recovered.phase,'running');await tcpEcho('198.18.77.1',echoPort);
  await fs.writeFile(path.join(control,'stop'),'stop');assert.equal((await waitFile('stopped.json')).phase,'stopped');
  assert.equal(await new Promise(resolve=>worker.exitCode!==null?resolve(worker.exitCode):worker.once('exit',resolve)),0);worker=null;
  report.verified.push('controller crash loses authorization channel; subsequent explicit connection confirms exclusive TUN ownership and clean shutdown');
  await launch();process.kill(authorizedPid(worker.pid),'SIGKILL');worker.kill('SIGKILL');await new Promise(resolve=>worker.exitCode!==null?resolve():worker.once('exit',resolve));worker=null;await delay(1000);
  const afterWorkerCrash=await launch();assert.equal(afterWorkerCrash.phase,'running');await tcpEcho('198.18.77.1',echoPort);
  await fs.writeFile(path.join(control,'stop'),'stop');assert.equal((await waitFile('stopped.json')).phase,'stopped');assert.equal(await new Promise(resolve=>worker.exitCode!==null?resolve(worker.exitCode):worker.once('exit',resolve)),0);worker=null;
  report.verified.push('authorization worker crash recovers protected process identity and clears residual ownership before a new explicit session');report.passed=true;
}catch(error){report.error=String(error);process.exitCode=1;if(process.platform==='darwin'){report.routeDiagnostics=spawnSync('/usr/sbin/netstat',['-rn'],{encoding:'utf8'}).stdout;}if(process.platform==='linux'){report.routeDiagnostics=spawnSync('ip',['-j','route','show','table','all'],{encoding:'utf8'}).stdout;}}
finally{clearTimeout(timeout);if(worker){worker.kill();await delay(12_000);}if(proxy)proxy.kill();if(udp)udp.close();if(tcp)tcp.close();report.log=logs.slice(-32768);await fs.writeFile(path.join(directory,'integration-report.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));process.exit(report.passed?0:1);}
