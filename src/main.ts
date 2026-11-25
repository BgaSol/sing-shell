import './style.css';
import {getVersion} from '@tauri-apps/api/app';
import {listen} from '@tauri-apps/api/event';
import {core} from './core';
import {createRuntime,element} from './runtime';
import {createUpdates} from './updates';
const runtime=createRuntime(),updates=createUpdates();let ready=false;
for(const id of ['runtime','updates'])element(`nav-${id}`).onclick=()=>{element('runtime-panel').hidden=id!=='runtime';element('updates-panel').hidden=id!=='updates';for(const name of ['runtime','updates'])element(`nav-${name}`).setAttribute('aria-current',name===id?'page':'false');};
void core.dirty(false);
void listen<string>('shutdown-error',event=>runtime.error(event.payload));void listen('close-requested',()=>void updates.close());
void getVersion().then(v=>element('version').textContent=`Sing Shell ${v}`);
async function refresh(){await Promise.allSettled([
  core.snapshot().then(s=>{runtime.render(s);updates.runtime(s);if(!ready){ready=true;void core.ready();}}).catch(e=>runtime.error(e)),
  core.updates().then(s=>updates.render(s)).catch(e=>updates.error(e)),
]);window.setTimeout(()=>void refresh(),700);}
void refresh();
