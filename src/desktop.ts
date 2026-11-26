import {invoke} from '@tauri-apps/api/core';
import {core} from './core';
import {element} from './runtime';
interface DesktopSnapshot {error:string|null;navigation:{id:number;target:string}|null;exitRequest:{id:number;document:string|null}|null}
export function createDesktop(navigate:(target:string)=>void,mayLeave:()=>Promise<boolean>){
  const documentId=crypto.randomUUID();
  let processing=false,lastRequest=0;
  const error=element('desktop-error');
  function freeze(value:boolean){for(const id of ['runtime-panel','nodes-panel','routing-panel','updates-panel'])element(id).inert=value;}
  async function refresh(closing=false){
    try{
      const s=await invoke<DesktopSnapshot>('desktop_snapshot');
      error.textContent=s.error??'';error.hidden=!s.error;
      if(s.navigation){navigate(s.navigation.target);await invoke('desktop_navigation_done',{id:s.navigation.id});}
      if(!processing&&!closing&&!s.exitRequest)freeze(false);
      const request=s.exitRequest;
      if(!request||request.document!==documentId||processing||lastRequest===request.id)return;
      processing=true;lastRequest=request.id;
      try{
        await invoke('exit_presented',{id:request.id,document:documentId});
        freeze(true);
        const proceed=await mayLeave();
        await invoke('exit_decision',{id:request.id,document:documentId,proceed});
        if(!proceed)freeze(false);
      }catch(e){
        freeze(false);error.textContent=String(e);error.hidden=false;
        await invoke('exit_decision',{id:request.id,document:documentId,proceed:false}).catch(()=>{});
      }finally{processing=false;}
    }catch(e){error.textContent=String(e);error.hidden=false;}
  }
  return {ready:()=>core.ready(documentId),refresh};
}
