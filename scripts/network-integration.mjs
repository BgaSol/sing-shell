import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
import dgram from 'node:dgram';
import os from 'node:os';
import {lookup} from 'node:dns/promises';
import {spawn,spawnSync} from 'node:child_process';
import {echoPair} from './network-echo.mjs';

assert.equal(process.env.SING_SHELL_NETWORK_PREFLIGHT,'1','disposable native runner only');
const approvedMac=process.env.SING_SHELL_MACOS_AUTHORIZATION==='1';
if(approvedMac){assert.equal(process.platform,'darwin');assert.notEqual(process.getuid(),0,'system-authorized integration must run as an ordinary user');}
const target=process.argv[2],directory=path.resolve('artifacts/network-preflight',target),control=await fs.mkdtemp(path.join(os.tmpdir(),'ss-net-'));
await fs.mkdir(directory,{recursive:true});
const suffix=process.platform==='win32'?'.exe':'';
const executable=process.env.SING_SHELL_NETWORK_EXECUTABLE??path.resolve(`src-tauri/target/release/network-fixture${suffix}`);
const coreVersion=process.env.CORE_VERSION??'1.14.4';
const core=process.env.SING_SHELL_NETWORK_CORE??path.join(directory,`sing-box-${coreVersion}-${process.platform==='win32'?'windows':process.platform==='darwin'?'darwin':'linux'}-${process.arch==='arm64'?'arm64':'amd64'}`,`sing-box${suffix}`);
const report={target,passed:false,verified:[],skipped:['interactive UAC/polkit approval','macOS administrator authorization dialog','suspend and physical network switching']};
if(approvedMac)report.skipped=['interactive UAC/polkit approval (other platforms)','controller/core/helper crash injection on the user Mac; covered on disposable native runners','suspend and physical network switching'];
const timeout=setTimeout(async()=>{report.error='integration deadline exceeded';report.log=logs.slice(-32768);if(worker)worker.kill();if(proxy)proxy.kill();await fs.writeFile(path.join(directory,'integration-report.json'),JSON.stringify(report,null,2));console.error(JSON.stringify(report));process.exit(1);},240_000);
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function waitFile(name){const deadline=Date.now()+210_000;for(;;){try{return JSON.parse(await fs.readFile(path.join(control,name),'utf8'));}catch(e){if(e.code!=='ENOENT')throw e;}assert.ok(Date.now()<deadline,`timeout ${name}`);await delay(100);}}
// Publish only complete JSON commands to the native fixture's polling loop.
async function replaceNode(id,input){const staging=path.join(control,'replace.part');await fs.writeFile(staging,JSON.stringify({id,input}));await fs.rename(staging,path.join(control,'replace.json'));return waitFile(`replace-${id}.json`);}
async function tcpEcho(host,port){await new Promise((resolve,reject)=>{const socket=net.connect({host,port});socket.setTimeout(8000,()=>socket.destroy(Error('TCP tunnel timeout')));socket.on('error',reject);socket.once('connect',()=>socket.write('sing-shell-proxy'));let data='';socket.on('data',chunk=>{data+=chunk;if(data==='sing-shell-proxy'){socket.end();resolve();}});});}
async function udpEcho(host,port){const socket=dgram.createSocket(net.isIP(host)===6?'udp6':'udp4');try{await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('UDP tunnel timeout')),8000);socket.once('error',e=>{clearTimeout(timer);reject(e);});socket.once('message',message=>{clearTimeout(timer);try{assert.equal(message.toString(),'sing-shell-udp');resolve();}catch(e){reject(e);}});socket.send('sing-shell-udp',port,host);});}finally{socket.close();}}
function authorizedProcess(parent){
  const command=process.platform==='win32'?spawnSync('powershell.exe',['-NoProfile','-Command',`@(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -match '--network-worker ' -and $_.CommandLine.TrimEnd().EndsWith(' ${parent}') } | Select-Object -ExpandProperty ProcessId) | ConvertTo-Json -Compress`],{encoding:'utf8',windowsHide:true}):spawnSync('/bin/ps',['-axo','pid=,command='],{encoding:'utf8'});
  assert.equal(command.status,0);
  if(process.platform!=='win32'){
    const listing=spawnSync('/bin/ps',['-axo','pid=,ppid=,command='],{encoding:'utf8'});assert.equal(listing.status,0);
    const rows=listing.stdout.split('\n').map(line=>line.trim().match(/^(\d+)\s+(\d+)\s+(.*)$/)).filter(Boolean).map(m=>({pid:Number(m[1]),parent:Number(m[2]),command:m[3]}));
    const descendants=rows.filter(row=>{let pid=row.parent;for(let depth=0;depth<12&&pid>1;depth++){if(pid===parent)return true;pid=rows.find(p=>p.pid===pid)?.parent??0;}return false;});
    const helpers=descendants.filter(row=>row.command.includes('--network-worker '));assert.equal(helpers.length,1,'one authorized descendant of the app launcher');
    const controller=Number(helpers[0].command.trim().split(/\s+/).at(-1));assert.ok(controller===parent||descendants.some(row=>row.pid===controller));return {helper:helpers[0].pid,controller};
  }
  const matches=[JSON.parse(command.stdout)].flat();
  assert.equal(matches.length,1,'one authenticated worker owned by the fixture controller');
  assert.ok(Number.isInteger(matches[0])&&matches[0]>0&&matches[0]!==parent);return {helper:matches[0],controller:parent};
}
function macDns() {
  const result=spawnSync('/usr/sbin/scutil',['--dns'],{encoding:'utf8',timeout:5000});assert.equal(result.status,0);return result.stdout;
}
async function macDnsReleased() {
  const started=Date.now(),deadline=started+5000;
  while(macDns().includes('172.19.0.2')) {
    if(Date.now()>=deadline) {
      report.dnsReleaseDiagnostics={effective:macDns(),keys:spawnSync('/usr/sbin/scutil',[],{input:'list State:/Network/Service/sing-shell-.*/DNS\n',encoding:'utf8',timeout:3000}).stdout};
      assert.fail('temporary system DNS did not converge after worker exit');
    }
    await delay(100);
  }
  (report.dnsReleaseMs??=[]).push(Date.now()-started);
}
function macPreferences() {
  const services=spawnSync('/usr/sbin/networksetup',['-listallnetworkservices'],{encoding:'utf8',timeout:5000});assert.equal(services.status,0);
  return services.stdout.split('\n').slice(1).map(s=>s.trim()).filter(Boolean).map(service=>{
    const result=spawnSync('/usr/sbin/networksetup',['-getdnsservers',service.replace(/^\*/, '')],{encoding:'utf8',timeout:5000});assert.equal(result.status,0);return [service,result.stdout];
  });
}
const dnsBefore=process.platform==='darwin'?macPreferences():null;
let proxy,worker,tcp,udp;
let logs='';
try{
  const echo=await echoPair();tcp=echo.tcp;udp=echo.udp;const echoPort=echo.port;
  const holder=net.createServer();await new Promise(resolve=>holder.listen(0,'127.0.0.1',resolve));const proxyPort=holder.address().port;await new Promise(resolve=>holder.close(resolve));
  const secondHolder=net.createServer();await new Promise(resolve=>secondHolder.listen(0,'127.0.0.1',resolve));const secondProxyPort=secondHolder.address().port;await new Promise(resolve=>secondHolder.close(resolve));
  const password='sing-shell-fixture-password';
  const serverConfig={log:{level:'warn'},inbounds:[{type:'shadowsocks',tag:'ss-in',listen:'127.0.0.1',listen_port:proxyPort,method:'aes-128-gcm',password}],outbounds:[{type:'direct',tag:'echo'}],route:{rules:[{ip_cidr:['198.51.100.1/32','2001:db8:198::99/128'],action:'route',outbound:'echo',override_address:'127.0.0.1',override_port:echoPort}],final:'echo'}};
  if(process.platform==='darwin') {
    const route=spawnSync('/sbin/route',['-n','get','default'],{encoding:'utf8'});assert.equal(route.status,0);
    const physical=route.stdout.match(/interface:\s+(\S+)/)?.[1];assert.ok(physical&&!physical.startsWith('utun'));
    serverConfig.outbounds[0].bind_interface=physical;
    serverConfig.route.rules.push({ip_cidr:['1.1.1.1/32'],port:[443],action:'route',outbound:'echo'});
  }
  // Unrelated runner traffic must not loop from the proxy server back into the TUN.
  serverConfig.route.rules.push({port:[echoPort],action:'route',outbound:'echo',override_address:'127.0.0.1'});
  serverConfig.route.rules.push({action:'reject'});
  serverConfig.inbounds.push({...serverConfig.inbounds[0],tag:'ss-second',listen_port:secondProxyPort});
  const config=path.join(directory,'proxy-server.json');await fs.writeFile(config,JSON.stringify(serverConfig));
  proxy=spawn(core,['run','-c',config],{stdio:['ignore','pipe','pipe'],windowsHide:true});proxy.stdout.on('data',bytes=>{logs+=bytes;});proxy.stderr.on('data',bytes=>{logs+=bytes;});await delay(700);assert.equal(proxy.exitCode,null,'controlled proxy stays alive');
  const input={node:{tags:[],id:'test-node',name:'Controlled Shadowsocks',source:'fixture',spec:{type:'shadowsocks',server:'127.0.0.1',server_port:proxyPort,method:'aes-128-gcm',password}},routing:{defaultExit:'direct',policies:[{id:'github',name:'GitHub',enabled:true,exit:'direct',match:{kind:'source',sourceId:'github'}},{id:'echo',name:'Echo',enabled:true,exit:'proxy',match:{kind:'custom',domains:[],ipCidrs:['198.51.100.1/32','2001:db8:198::99/128']}}]},coreVersion,coreProgram:core,material:JSON.parse(await fs.readFile(process.env.CORE_MATERIAL??`src-tauri/resources/cores/${coreVersion}.json`)),control};
  input.routing.policies.push({id:'dns-domain',name:'DNS domain',enabled:true,exit:'proxy',match:{kind:'custom',domains:[{value:'example.com',includeSubdomains:true}],ipCidrs:[]}});
  const inputFile=path.join(directory,'worker-input.json');await fs.writeFile(inputFile,JSON.stringify(input));
  async function launch(){
    for(const file of ['started.json','stopped.json','stop','activity.json','query-activity','replace.json','replace-1.json','replace-2.json'])await fs.rm(path.join(control,file),{force:true});
    const command=approvedMac?'/usr/bin/open':executable;
    const args=approvedMac?['-W','-n','--env','SING_SHELL_NETWORK_PREFLIGHT=1','--stdout',path.join(control,'app.stdout'),'--stderr',path.join(control,'app.stderr'),path.resolve(executable,'../../..'),'--args','--smoke-test','--network-fixture-input',inputFile]:process.env.SING_SHELL_NETWORK_EXECUTABLE?['--smoke-test','--network-fixture-input',inputFile]:[inputFile];
    worker=spawn(command,args,{stdio:['ignore','pipe','pipe'],windowsHide:true});worker.stdout.on('data',bytes=>{logs=(logs+bytes).slice(-65536);});worker.stderr.on('data',bytes=>{logs=(logs+bytes).slice(-65536);});
    return Promise.race([waitFile('started.json'),new Promise((_,reject)=>{worker.once('error',reject);worker.once('exit',code=>reject(Error(`worker exited ${code}: ${logs}`)));})]);
  }
  const protectedRoot=process.platform==='win32'?path.join(process.env.ProgramData,'Sing Shell Network'):process.platform==='darwin'?'/Library/Application Support/Sing Shell Network':'/var/lib/sing-shell-network';
  if(!approvedMac){
    assert.equal(process.env.GITHUB_ACTIONS,'true','fresh install cleanup only on disposable Actions runners');
    for(const relative of [`trusted-${coreVersion}.json`,`engines/sing-box/${coreVersion}`]){
      const targetPath=path.resolve(protectedRoot,relative);assert.ok(targetPath.startsWith(path.resolve(protectedRoot)+path.sep));
      await fs.rm(targetPath,{recursive:true,force:true});
    }
    report.freshProtectedInstall=true;
  }
  console.log('Waiting for authorized worker to prepare the official core and TUN');
  const started=await launch();
  console.log('Authorized TUN running; verifying selected-node traffic');
  assert.equal(started.phase,'running');assert.ok(started.runtime.pid&&started.network.interface&&started.network.ipv4&&started.network.ipv6&&started.network.routes);
  report.coreVersion=coreVersion;
  if(!approvedMac)assert.ok((await fs.stat(path.join(protectedRoot,`trusted-${coreVersion}.json`))).isFile());
  report.verified.push('production network worker authenticates control channel and independently verifies official core','typed node and routing become checked private TUN config','native interface and IPv4/IPv6 routes are confirmed');
  for(const address of ['198.51.100.1','2001:db8:198::99']){await tcpEcho(address,echoPort);await udpEcho(address,echoPort);report.verified.push(`TCP and UDP ${net.isIP(address)===6?'IPv6':'IPv4'} forwarded through selected encrypted Shadowsocks node`);}
  const fakeName=`sing-shell-${Date.now()}.example.com`;
  const fakeAddresses=await lookup(fakeName,{all:true});
  assert.ok(fakeAddresses.length&&fakeAddresses.every(a=>/^198\.(18|19)\./.test(a.address)),'system resolver uses production Fake-IP');
  report.verified.push('native system resolver receives production IPv4 Fake-IP for a proxy domain');
  if(process.platform==='darwin') {
    const dns=macDns();assert.match(dns,/nameserver\[0\] : 172\.19\.0\.2/);
    const addresses=await Promise.race([lookup('example.com',{all:true}),delay(15000).then(()=>{throw Error('system DNS lookup timeout');})]);
    assert.ok(addresses.length&&addresses.every(a=>net.isIP(a.address)));
    const cachedFake=addresses.find(a=>/^198\.(18|19)\./.test(a.address));assert.ok(cachedFake);
    await tcpEcho(cachedFake.address,echoPort);await udpEcho(cachedFake.address,echoPort);
    report.cachedFake=cachedFake.address;
    report.verified.push('macOS system Fake-IP reaches its restored real destination through TCP and UDP TUN traffic');
    report.verified.push('macOS effective resolver selects temporary TUN DNS; system getaddrinfo resolves a domain through production DNS rules (external DoH upstream required)');
  }
  await tcpEcho('127.0.0.1',echoPort);report.verified.push('loopback bypass remains direct');
  const identity=authorizedProcess(worker.pid);
  const observer=net.connect({host:'198.51.100.1',port:echoPort});observer.on('error',()=>{});await new Promise((resolve,reject)=>{observer.once('connect',resolve);observer.once('error',reject);});observer.write('activity-observation');
  let page,observed;const sampleDeadline=Date.now()+10_000;
  do{await fs.rm(path.join(control,'activity.json'),{force:true});await fs.writeFile(path.join(control,'query-activity'),'read');page=await waitFile('activity.json');observed=page.items.some(c=>c.target===`198.51.100.1:${echoPort}`&&c.exit==='proxy'&&c.policyId==='echo');if(!observed)await delay(300);}while(!observed&&Date.now()<sampleDeadline);
  observer.destroy();assert.ok(['ready','overflow'].includes(page.summary.state));assert.ok(page.summary.sessionId);assert.ok(observed,JSON.stringify({summary:page.summary,items:page.items.length}));assert.ok(!JSON.stringify(page).includes(password));report.verified.push('worker collects authenticated core connections with session identity and real matched policy tags');
  const replacement={node:{...input.node,id:'replacement-node',name:'Replacement',spec:{...input.node.spec,server_port:secondProxyPort}},routing:input.routing,versions:[],coreVersion:input.coreVersion};
  const bundled=JSON.parse(await fs.readFile('src-tauri/resources/rules/manifest.json','utf8'));const enabled=new Set(input.routing.policies.filter(p=>p.enabled&&p.match.kind==='source').map(p=>p.match.sourceId));replacement.versions=bundled.filter(v=>enabled.has(v.sourceId));
  const invalid=structuredClone(replacement);if(invalid.versions.length)invalid.versions[0].sha256='0'.repeat(64);else{invalid.routing.policies.push({id:'invalid-source',name:'Invalid candidate',enabled:true,exit:'direct',match:{kind:'source',sourceId:'github'}});invalid.versions=bundled.filter(v=>v.sourceId==='github').map(v=>({...v,sha256:'0'.repeat(64)}));}
  const failed=await replaceNode(1,invalid);assert.equal(failed.succeeded,false);assert.equal(failed.snapshot.runtime.pid,started.runtime.pid);await tcpEcho('198.51.100.1',echoPort);report.verified.push('invalid protected candidate is rejected while the existing core PID and forwarding survive');
  const replaced=await replaceNode(2,replacement);assert.equal(replaced.succeeded,true,replaced.error);assert.equal(replaced.session.runningNode.id,replaced.settings.profile.selectedNode);assert.equal(replaced.settings.startup.resumeCore,true);assert.equal(replaced.session.pendingChanges,false);assert.ok(!JSON.stringify(replaced).includes(password));assert.notEqual(replaced.snapshot.runtime.pid,started.runtime.pid);assert.equal(authorizedProcess(worker.pid).helper,identity.helper);for(const address of ['198.51.100.1','2001:db8:198::99']){await tcpEcho(address,echoPort);await udpEcho(address,echoPort);}report.verified.push('node import and SessionService connect freeze saved routing, confirm and persist the actual selected node; replacement reuses the worker and preserves encrypted dual-stack TCP/UDP');
  const fakeAfterReplacement=await lookup(fakeName,{all:true});assert.deepEqual(fakeAfterReplacement,fakeAddresses);
  if(report.cachedFake){await tcpEcho(report.cachedFake,echoPort);await udpEcho(report.cachedFake,echoPort);}
  report.verified.push('production worker replacement preserves cached Fake-IP mappings');
  const compatible={...replacement,options:{ipv4Compatibility:true}};
  const compatibilityResult=await replaceNode(3,compatible);assert.equal(compatibilityResult.succeeded,true,compatibilityResult.error);
  assert.equal(compatibilityResult.session.runningConnectionOptions.ipv4Compatibility,true);assert.equal(compatibilityResult.session.pendingChanges,false);
  await tcpEcho('198.51.100.1',echoPort);
  await assert.rejects(()=>tcpEcho('2001:db8:198::99',echoPort));
  report.verified.push('saved compatibility option reaches the actual worker, preserves IPv4 and rejects public IPv6');

  await fs.writeFile(path.join(control,'stop'),'stop');const stopped=await waitFile('stopped.json');assert.equal(stopped.runtime.pid,null);assert.equal(stopped.phase,'stopped');
  const exit=await new Promise(resolve=>{if(worker.exitCode!==null)resolve(worker.exitCode);else worker.once('exit',resolve);});assert.equal(exit,0);worker=null;
  assert.ok(!JSON.stringify(started).includes(password));report.verified.push('stop releases actual TUN and terminates authorization worker','worker diagnostics do not return known node credential');
  if(process.platform==='darwin') {
    await macDnsReleased();
    assert.deepEqual(macPreferences(),dnsBefore,'persistent service DNS preferences remain unchanged');
    report.verified.push('temporary resolver released and all existing network-service DNS preferences preserved');
  }
  if(approvedMac){
    report.verified.push('ordinary installed app uses system administrator authorization, authenticates the root helper and confirms its exit');
  }else{
  const interrupted=await launch();process.kill(interrupted.runtime.pid,'SIGKILL');await delay(2000);
  await fs.writeFile(path.join(control,'stop'),'stop');assert.equal((await waitFile('stopped.json')).phase,'stopped');
  assert.equal(await new Promise(resolve=>worker.exitCode!==null?resolve(worker.exitCode):worker.once('exit',resolve)),0);worker=null;
  report.verified.push('unexpected official core exit is cleaned without crash reconnection');
  await launch();process.kill(authorizedProcess(worker.pid).controller,'SIGKILL');await new Promise(resolve=>worker.exitCode!==null?resolve():worker.once('exit',resolve));worker=null;await delay(12_000);
  const recovered=await launch();assert.equal(recovered.phase,'running');await tcpEcho('198.51.100.1',echoPort);
  await fs.writeFile(path.join(control,'stop'),'stop');assert.equal((await waitFile('stopped.json')).phase,'stopped');
  assert.equal(await new Promise(resolve=>worker.exitCode!==null?resolve(worker.exitCode):worker.once('exit',resolve)),0);worker=null;
  report.verified.push('controller crash loses authorization channel; subsequent explicit connection confirms exclusive TUN ownership and clean shutdown');
  await launch();const crashed=authorizedProcess(worker.pid);process.kill(crashed.helper,'SIGKILL');process.kill(crashed.controller,'SIGKILL');await new Promise(resolve=>worker.exitCode!==null?resolve():worker.once('exit',resolve));worker=null;await delay(1000);
  if(process.platform==='darwin')await macDnsReleased();
  const afterWorkerCrash=await launch();assert.equal(afterWorkerCrash.phase,'running');await tcpEcho('198.51.100.1',echoPort);
  await fs.writeFile(path.join(control,'stop'),'stop');assert.equal((await waitFile('stopped.json')).phase,'stopped');assert.equal(await new Promise(resolve=>worker.exitCode!==null?resolve(worker.exitCode):worker.once('exit',resolve)),0);worker=null;
  report.verified.push('authorization worker crash recovers protected process identity and clears residual ownership before a new explicit session');
  }
  report.passed=true;
}catch(error){report.error=String(error);process.exitCode=1;if(process.platform==='darwin'){report.routeDiagnostics=spawnSync('/usr/sbin/netstat',['-rn'],{encoding:'utf8'}).stdout;}if(process.platform==='linux'){report.routeDiagnostics=spawnSync('ip',['-j','route','show','table','all'],{encoding:'utf8'}).stdout;}}
finally{clearTimeout(timeout);if(worker){worker.kill();await delay(12_000);}if(proxy)proxy.kill();if(udp)udp.close();if(tcp)tcp.close();report.log=logs.slice(-32768);await fs.writeFile(path.join(directory,'integration-report.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));process.exit(report.passed?0:1);}
