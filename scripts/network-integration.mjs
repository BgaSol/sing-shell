import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
import dgram from 'node:dgram';
import {spawn} from 'node:child_process';

assert.equal(process.env.SING_SHELL_NETWORK_PREFLIGHT,'1','disposable native runner only');
const target=process.argv[2],directory=path.resolve('artifacts/network-preflight',target),control=path.join(directory,'integration-control');
await fs.mkdir(control,{recursive:true});
const suffix=process.platform==='win32'?'.exe':'';
const executable=path.resolve(`src-tauri/target/release/network-fixture${suffix}`);
const core=path.join(directory,`sing-box-1.14.4-${process.platform==='win32'?'windows':process.platform==='darwin'?'darwin':'linux'}-${process.arch==='arm64'?'arm64':'amd64'}`,`sing-box${suffix}`);
const report={target,passed:false,verified:[],skipped:['interactive UAC/polkit approval','macOS launchd activation after user approval','suspend and physical network switching']};
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function waitFile(name){const deadline=Date.now()+210_000;for(;;){try{return JSON.parse(await fs.readFile(path.join(control,name),'utf8'));}catch(e){if(e.code!=='ENOENT')throw e;}assert.ok(Date.now()<deadline,`timeout ${name}`);await delay(100);}}
async function tcpEcho(host,port){await new Promise((resolve,reject)=>{const socket=net.connect({host,port});socket.setTimeout(8000,()=>socket.destroy(Error('TCP tunnel timeout')));socket.on('error',reject);socket.once('connect',()=>socket.write('sing-shell-proxy'));let data='';socket.on('data',chunk=>{data+=chunk;if(data==='sing-shell-proxy'){socket.end();resolve();}});});}
async function udpEcho(host,port){const socket=dgram.createSocket(net.isIP(host)===6?'udp6':'udp4');try{await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('UDP tunnel timeout')),8000);socket.once('error',e=>{clearTimeout(timer);reject(e);});socket.once('message',message=>{clearTimeout(timer);try{assert.equal(message.toString(),'sing-shell-udp');resolve();}catch(e){reject(e);}});socket.send('sing-shell-udp',port,host);});}finally{socket.close();}}
let proxy,worker,tcp,udp;
let logs='';
try{
  tcp=net.createServer(socket=>socket.pipe(socket));await new Promise(resolve=>tcp.listen(0,'127.0.0.1',resolve));const echoPort=tcp.address().port;
  udp=dgram.createSocket('udp4');udp.on('message',(message,remote)=>udp.send(message,remote.port,remote.address));await new Promise(resolve=>udp.bind(echoPort,'127.0.0.1',resolve));
  const holder=net.createServer();await new Promise(resolve=>holder.listen(0,'127.0.0.1',resolve));const proxyPort=holder.address().port;await new Promise(resolve=>holder.close(resolve));
  const password='sing-shell-fixture-password';
  const serverConfig={log:{level:'warn'},inbounds:[{type:'shadowsocks',tag:'ss-in',listen:'127.0.0.1',listen_port:proxyPort,method:'aes-128-gcm',password}],outbounds:[{type:'direct',tag:'echo'}],route:{rules:[{ip_cidr:['198.18.77.1/32','2001:db8:198::99/128'],action:'route',outbound:'echo',override_address:'127.0.0.1',override_port:echoPort}],final:'echo'}};
  const config=path.join(directory,'proxy-server.json');await fs.writeFile(config,JSON.stringify(serverConfig));
  proxy=spawn(core,['run','-c',config],{stdio:['ignore','pipe','pipe'],windowsHide:true});proxy.stdout.on('data',bytes=>{logs+=bytes;});proxy.stderr.on('data',bytes=>{logs+=bytes;});await delay(700);assert.equal(proxy.exitCode,null,'controlled proxy stays alive');
  const input={node:{id:'test-node',name:'Controlled Shadowsocks',source:'fixture',spec:{type:'shadowsocks',server:'127.0.0.1',server_port:proxyPort,method:'aes-128-gcm',password}},routing:{defaultAction:'proxy',customRules:[],siteRules:[{site:'github',action:'direct'}]},coreVersion:'1.14.4',control};
  const inputFile=path.join(directory,'worker-input.json');await fs.writeFile(inputFile,JSON.stringify(input));
  worker=spawn(executable,[inputFile],{stdio:['ignore','pipe','pipe'],windowsHide:true});worker.stdout.on('data',bytes=>{logs+=bytes;});worker.stderr.on('data',bytes=>{logs+=bytes;});
  const started=await Promise.race([waitFile('started.json'),new Promise((_,reject)=>worker.once('exit',code=>reject(Error(`worker exited ${code}: ${logs}`))))]);
  assert.equal(started.phase,'running');assert.ok(started.runtime.pid&&started.network.interface&&started.network.ipv4&&started.network.ipv6&&started.network.routes);
  report.verified.push('production network worker authenticates control channel and independently verifies official core','typed node and routing become checked private TUN config','native interface and IPv4/IPv6 routes are confirmed');
  for(const address of ['198.18.77.1','2001:db8:198::99']){await tcpEcho(address,echoPort);await udpEcho(address,echoPort);report.verified.push(`TCP and UDP ${net.isIP(address)===6?'IPv6':'IPv4'} forwarded through selected encrypted Shadowsocks node`);}
  await tcpEcho('127.0.0.1',echoPort);report.verified.push('loopback bypass remains direct');
  await fs.writeFile(path.join(control,'stop'),'stop');const stopped=await waitFile('stopped.json');assert.equal(stopped.runtime.pid,null);assert.equal(stopped.phase,'stopped');
  const exit=await new Promise(resolve=>{if(worker.exitCode!==null)resolve(worker.exitCode);else worker.once('exit',resolve);});assert.equal(exit,0);worker=null;
  assert.ok(!JSON.stringify(started).includes(password));report.verified.push('stop releases actual TUN and terminates authorization worker','worker diagnostics do not return known node credential');report.passed=true;
}catch(error){report.error=String(error);process.exitCode=1;}
finally{if(worker){worker.kill();await delay(12_000);}if(proxy)proxy.kill();if(udp)udp.close();if(tcp)tcp.close();report.log=logs.slice(-32768);await fs.writeFile(path.join(directory,'integration-report.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));}
