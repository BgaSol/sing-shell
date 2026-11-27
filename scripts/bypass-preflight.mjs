import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
import {spawn,execFileSync} from 'node:child_process';
import {randomBytes} from 'node:crypto';

assert.equal(process.env.GITHUB_ACTIONS,'true','disposable native runner only');
assert.equal(process.env.SING_SHELL_NETWORK_PREFLIGHT,'1');
const target=process.argv[2],version=process.env.CORE_VERSION??'1.14.4';
const directory=path.resolve('artifacts/network-preflight',target);
const host={win32:'windows',linux:'linux',darwin:'darwin'}[process.platform];
const core=path.join(directory,`sing-box-${version}-${host}-${process.arch==='arm64'?'arm64':'amd64'}`,`sing-box${process.platform==='win32'?'.exe':''}`);
const report={target,version,passed:false,verified:[]};
const children=new Set();
let output='';
const delay=ms=>new Promise(r=>setTimeout(r,ms));
async function port(){const s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const p=s.address().port;await new Promise(r=>s.close(r));return p;}
async function stop(child){if(child.exitCode===null){child.kill('SIGTERM');const deadline=Date.now()+10_000;while(child.exitCode===null){assert.ok(Date.now()<deadline,'core exit timeout');await delay(50);}}children.delete(child);}
async function run(name,config){
  const secret=randomBytes(24).toString('hex'),controller=await port();
  config.experimental={clash_api:{external_controller:`127.0.0.1:${controller}`,secret}};
  const file=path.join(directory,`bypass-${name}.json`);
  await fs.writeFile(file,JSON.stringify(config));
  execFileSync(core,['check','-c',file],{windowsHide:true,timeout:15000});
  const env={...process.env};for(const key of Object.keys(env))if(/token|secret|password|private.key/i.test(key))delete env[key];
  const child=spawn(core,['run','-c',file],{env,windowsHide:true,stdio:['ignore','pipe','pipe']});children.add(child);
  child.stdout.on('data',b=>{output=(output+b).slice(-32768);});child.stderr.on('data',b=>{output=(output+b).slice(-32768);});
  const url=`http://127.0.0.1:${controller}`;
  const deadline=Date.now()+15000;
  for(;;){assert.equal(child.exitCode,null,output);try{const r=await fetch(url,{signal:AbortSignal.timeout(1000)});if(r.status===401)break;}catch{}assert.ok(Date.now()<deadline,output);await delay(100);}
  return {child,async query(name,type='A'){assert.equal(child.exitCode,null,output);const r=await fetch(`${url}/dns/query?${new URLSearchParams({name,type})}`,{headers:{Authorization:`Bearer ${secret}`},signal:AbortSignal.timeout(10000)});assert.equal(r.status,200,await r.clone().text());const a=await r.json();assert.equal(a.Status,0);assert.equal(child.exitCode,null,output);return (a.Answer??[]).filter(a=>a.type===(type==='A'?1:28)).map(a=>a.data);}};
}
const resolverConfig=()=>({log:{level:'warn'},dns:{servers:[{type:'local',tag:'physical'}],final:'physical',disable_cache:true},route:{auto_detect_interface:true}});
try{
  const fresh=await run('fresh',resolverConfig());
  const before=await fresh.query('example.com');assert.ok(before.length);await stop(fresh.child);
  report.verified.push('isolated local DNS query without TUN');
  const iface=process.platform==='darwin'?'utun223':'sing-shell';
  const old=await run('old',{
    log:{level:'info'},
    inbounds:[{type:'tun',tag:'tun-in',interface_name:iface,address:['172.19.0.1/30','fdfe:dcba:9876::1/126'],auto_route:true,strict_route:true,stack:'mixed',dns_mode:'hijack',dns_address:['172.19.0.2','fdfe:dcba:9876::2'],route_exclude_address:['203.0.113.71/32','2001:db8:71::1/128']}],
    dns:{servers:[{type:'hosts',tag:'old',predefined:{'example.com':['192.0.2.71']}},{type:'local',tag:'local'}],rules:[{domain:['example.com'],action:'route',server:'old'}],final:'local'},
    outbounds:[{type:'direct',tag:'direct'}],route:{auto_detect_interface:true,rules:[{protocol:'dns',action:'hijack-dns'}],final:'direct'}
  });
  assert.deepEqual(await old.query('example.com'),['192.0.2.71']);
  const candidate=await run('candidate',resolverConfig());
  const actual=await candidate.query('example.com');
  assert.ok(actual.length&&actual.every(ip=>ip!=='192.0.2.71'&&!/^198\.(18|19)\./.test(ip)),`candidate resolved through old TUN: ${actual}`);
  report.verified.push('short-lived local DNS obtains fresh physical result while old TUN holds different pinned result');
  await stop(candidate.child);await stop(old.child);
  report.passed=true;
}catch(error){report.error=String(error);process.exitCode=1;}
finally{for(const child of children){try{await stop(child);}catch{child.kill('SIGKILL');}}report.log=output;await fs.writeFile(path.join(directory,'bypass-preflight-report.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));}
