import {createCoreStatus} from './core-status';
import {core,type Snapshot} from './core';
export {element} from './dom';
import {element} from './dom';
export function createRuntime(){
  const coreStatus=createCoreStatus('home-core');
  const buttons={chooseCore:element<HTMLButtonElement>('choose-core'),chooseConfig:element<HTMLButtonElement>('choose-config'),start:element<HTMLButtonElement>('start'),stop:element<HTMLButtonElement>('stop'),check:element<HTMLButtonElement>('check')};
  const logs=element<HTMLPreElement>('logs'),follow=element<HTMLInputElement>('follow'),error=element('error');let lastLogs='';let operationError:string|null=null;
  const invoke=async(task:()=>Promise<unknown>)=>{operationError=null;try{await task();error.hidden=true;}catch(e){operationError=String(e);error.textContent=operationError;error.hidden=false;}};
  element('apply-connection').onclick=()=>void invoke(()=>core.action('apply'));buttons.chooseCore.onclick=()=>void invoke(()=>core.choose('core'));buttons.chooseConfig.onclick=()=>void invoke(()=>core.choose('config'));buttons.start.onclick=()=>void invoke(()=>core.action('start'));buttons.stop.onclick=()=>void invoke(()=>core.action('stop'));buttons.check.onclick=()=>void invoke(()=>core.action('check'));follow.onchange=()=>{if(follow.checked)logs.scrollTop=logs.scrollHeight;};
  return {render(snapshot:Snapshot){coreStatus.render(snapshot);const r=snapshot.runtime,s=snapshot.settings,c=snapshot.core;const running=!!r.pid,busy=snapshot.runtimeBusy||snapshot.closing;const state=element('status');state.textContent=c.restore==='waiting'?'等待核心自动恢复':({running:'核心运行中',starting:'正在启动',stopping:'正在停止',checking:'正在校验',stopped:'核心已停止',failed:'需要处理'} as Record<string,string>)[r.status]??r.status;if(generatedMode(snapshot)&&r.status==='running')state.textContent='TUN 运行中';state.dataset.state=r.status;
    const generated=s.profile.mode==='generated';element('external-config-row').hidden=generated;buttons.start.lastChild!.textContent=generated?'启动 TUN':'启动 sing-box';const selected=s.profile.nodes.find(n=>n.id===s.profile.selectedNode);element('connection-node').textContent=generated?(running?c.runningNode?.name??'正在运行':selected?.name??'请先到节点页导入并选择节点'):'原始配置模式';element('connection-summary').textContent=generated?(c.phase==='authorizing'?'正在请求系统网络授权':c.pendingChanges?'已保存的新设置尚未生效，当前连接继续使用旧设置':c.phase==='running'?'TUN 接口及 IPv4/IPv6 路由已确认；远端可用性请查看日志':c.phase==='cleanupRequired'?'网络资源尚未释放，请重试停止':'未命中规则走'+(s.profile.routing.defaultAction==='proxy'?'代理':'直连')+'；本机与局域网直连。'):'网络设置由配置文件控制。';element('apply-connection').hidden=!generated||!running||!c.pendingChanges;element<HTMLButtonElement>('apply-connection').disabled=busy;
    element('pid').textContent=r.pid?`PID ${r.pid}`:'';element('config-path').textContent=s.pendingConfigPath?`${s.pendingConfigPath} · 等待核心校验`:s.configPath??'选择现有的 JSON 或 JSONC 配置文件';buttons.chooseCore.disabled=!c.canImport;buttons.chooseConfig.disabled=!c.canImport;buttons.start.disabled=!c.canStart;buttons.stop.disabled=!c.canStop;buttons.check.disabled=busy||running||!c.usable||!(s.configPath||s.pendingConfigPath);element('activity').textContent=busy?'正在处理…':'';
    const issue=snapshot.storageError??c.configError??r.error??operationError;error.textContent=issue??'';error.hidden=!issue;
    const text=snapshot.logs.map(l=>`${new Date(l.receivedAt).toLocaleTimeString()}  [${l.source}] ${l.message}`).join('\n');if(text!==lastLogs){const position=logs.scrollTop;logs.textContent=text||'选择核心和配置后，点击「启动 sing-box」。';logs.scrollTop=follow.checked?logs.scrollHeight:position;lastLogs=text;}
  },error(reason:unknown){error.textContent=`读取运行状态失败：${String(reason)}`;error.hidden=false;}};
}

function generatedMode(snapshot:Snapshot){return snapshot.settings.profile.mode==='generated';}
