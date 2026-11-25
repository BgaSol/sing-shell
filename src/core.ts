import { invoke } from '@tauri-apps/api/core';
export type Component = 'app'|'engine';
export interface Network { revision:number; fallbackEnabled:boolean; disabledBuiltins:string[]; customMirrors:{name:string;prefix:string;enabled:boolean}[] }
export interface Preferences {automatic:boolean;network:Network}
export interface Settings {schemaVersion:number;revision:number;engine:null|{mode:'external';path:string}|{mode:'managed';version:string;target:string};configPath:string|null;updatePreferences:Preferences}
export interface Snapshot {settings:Settings;runtime:{status:string;pid:number|null;version:string|null;error:string|null};runtimeBusy:boolean;storageError:string|null;closing:boolean;logs:{receivedAt:number;source:string;message:string}[]}
export interface Candidate {component:Component;version:string;target:string;notes:string;releaseUrl:string}
export interface UpdateItem {phase:string;candidate:Candidate|null;error:string|null;errorStage:string|null;downloadedBytes:number;totalBytes:number|null;network:{endpoint:string;attempt:number;error:string|null}|null}
export interface UpdateSnapshot {items:Partial<Record<Component,UpdateItem>>;lastCheckedAt:number;nextCheckAt:number;checking:boolean;initialized:boolean}
export interface Mirror {id:string;prefix:string;supportsApi:boolean}
export const core = {
  snapshot:()=>invoke<Snapshot>('core_snapshot'),
  action:(action:'start'|'stop'|'check')=>invoke('core_action',{action}),
  choose:(kind:'core'|'config')=>invoke('choose_file',{kind}),
  ready:()=>invoke('frontend_ready'),
  updates:()=>invoke<UpdateSnapshot>('update_snapshot'),
  check:(component:Component|null=null)=>invoke('check_updates',{component}),
  download:(components:Component[])=>invoke('download_updates',{components}),
  cancel:(component:Component)=>invoke('cancel_update',{component}),
  save:(expectedRevision:number,preferences:Preferences)=>invoke<Settings>('set_update_preferences',{expectedRevision,preferences}),
  diagnose:(prefix:string|null)=>invoke<{name:string;ok:boolean;elapsedMs:number;error:string|null}[]>('test_update_mirror',{prefix}),
  mirrors:()=>invoke<Mirror[]>('builtin_mirrors'),
  restart:(components:Component[])=>invoke('request_restart',{components}),
  release:(component:Component)=>invoke('open_update_release',{component}),
  dirty:(dirty:boolean)=>invoke('set_update_draft_dirty',{dirty}),
  exit:()=>invoke('request_exit'),
};
