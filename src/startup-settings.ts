import {invoke} from '@tauri-apps/api/core';
import {core,type Snapshot} from './core';
import {element} from './runtime';
interface Login {enabled:boolean;state:string;reason:string|null}
export function createStartupSettings(){
  const login=element<HTMLInputElement>('login-startup'),restore=element<HTMLInputElement>('restore-core');
  const error=element('startup-error'),status=element('login-status');
  let loginBusy=false,restoreBusy=false;
  const showError=(e:unknown)=>{error.textContent=String(e);error.hidden=false;};
  function renderLogin(value:Login){
    login.checked=value.enabled;
    login.disabled=loginBusy||['unavailable','unknown'].includes(value.state);
    login.indeterminate=value.state==='unknown';
    const labels:Record<string,string>={enabled:'已在系统登记',disabled:'未开启',blocked:'已被系统禁用',stale:'登记位置需要更新',unavailable:'当前安装无法使用',unknown:'无法确认系统状态'};
    status.textContent=[labels[value.state]??value.state,value.reason].filter(Boolean).join('。');
  }
  async function refresh(){
    if(loginBusy)return;
    try{renderLogin(await invoke<Login>('login_startup_read'));}catch(e){renderLogin({enabled:false,state:'unknown',reason:String(e)});}
  }
  login.onchange=()=>{loginBusy=true;login.disabled=true;error.hidden=true;
    void invoke<Login>('login_startup_set',{enabled:login.checked}).then(renderLogin).catch(showError).finally(()=>{loginBusy=false;void refresh();});
  };
  restore.onchange=()=>{restoreBusy=true;restore.disabled=true;error.hidden=true;
    void invoke('set_restore_core',{enabled:restore.checked}).catch(showError).finally(()=>{restoreBusy=false;void core.snapshot().then(render).catch(showError);});
  };
  function render(s:Snapshot){if(!restoreBusy){restore.checked=s.settings.startup.restoreCoreOnLaunch;restore.disabled=s.closing||!!s.storageError;}}
  element('refresh-login').onclick=()=>void refresh();
  element('quit-app').onclick=()=>void core.exit().catch(showError);
  window.addEventListener('focus',()=>void refresh());
  void refresh();
  return {render,refresh};
}
