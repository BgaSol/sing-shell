import './style.css';
import {getVersion} from '@tauri-apps/api/app';
import {listen} from '@tauri-apps/api/event';
import {core} from './core';
import {createRuntime,element} from './runtime';
import {createUpdates} from './updates';
import {createStartupSettings} from './startup-settings';
import {createDesktop} from './desktop';
const runtime=createRuntime(),updates=createUpdates();let ready=false;
const startup=createStartupSettings();
function navigate(id:string){element('runtime-panel').hidden=id!=='runtime';element('updates-panel').hidden=id!=='updates';for(const name of ['runtime','updates'])element(`nav-${name}`).setAttribute('aria-current',name===id?'page':'false');if(id==='updates')void startup.refresh();}
for(const id of ['runtime','updates'])element(`nav-${id}`).onclick=()=>navigate(id);
const desktop=createDesktop(navigate,updates.mayLeave);
void listen<string>('shutdown-error',event=>runtime.error(event.payload));
void listen('desktop-changed',()=>void desktop.refresh());
void getVersion().then(v=>element('version').textContent=`Sing Shell ${v}`);
async function refresh(){await Promise.allSettled([
  core.snapshot().then(async s=>{runtime.render(s);updates.runtime(s);startup.render(s);if(!ready){await desktop.ready();ready=true;}await desktop.refresh(s.closing);}).catch(e=>runtime.error(e)),
  core.updates().then(s=>updates.render(s)).catch(e=>updates.error(e)),
]);window.setTimeout(()=>void refresh(),700);}
void refresh();
