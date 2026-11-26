import { invoke } from '@tauri-apps/api/core';
export type Component = 'app'|'engine';
export interface Network { revision:number; fallbackEnabled:boolean; disabledBuiltins:string[]; customMirrors:{name:string;prefix:string;enabled:boolean}[] }
export interface Preferences {automatic:boolean;automaticCore:boolean;network:Network}
export interface ManagedRef {version:string;target:string;hash:string|null}
export type RouteAction='proxy'|'direct';
export interface RouteRule {kind:'domain'|'ip';value:string;includeSubdomains?:boolean;action:RouteAction}
export interface SiteRule {site:string;action:RouteAction}
export interface Routing {defaultAction:RouteAction;customRules:RouteRule[];siteRules:SiteRule[]}
export interface NodeSummary {id:string;name:string;source:string;protocol:string;server:string;port:number}
export interface Profile {mode:'generated'|'externalConfig';nodes:NodeSummary[];selectedNode:string|null;routing:Routing;ruleCommit:string}
export interface ProfileSnapshot extends Profile {revision:number}
export interface Settings {schemaVersion:number;coreRevision:number;profileRevision:number;preferencesRevision:number;profile:Profile;coreSource:{mode:'managed'}|{mode:'external';path:string};managed:{active:ManagedRef|null;previous:ManagedRef|null;trial:'awaitingStart'|'starting'|null};configPath:string|null;pendingConfigPath:string|null;updatePreferences:Preferences;startup:{restoreCoreOnLaunch:boolean;resumeCore:boolean};desktop:{backgroundNoticeShown:boolean}}
export interface CoreState {maintenance:{phase:'idle'|'checking'|'downloading'|'preparing'|'ready'|'paused'|'error';candidate:Candidate|null;error:string|null;downloadedBytes:number;totalBytes:number|null;network:{endpoint:string;attempt:number;error:string|null}|null;lastCheckedAt:number;nextCheckAt:number};initialized:boolean;usable:boolean;notice:string|null;configError:string|null;restore:'none'|'waiting'|'starting'|'finished';canStart:boolean;canStop:boolean;canImport:boolean;canRetry:boolean;phase:string;pendingChanges:boolean;runningNode:NodeSummary|null;network:{interface:boolean;ipv4:boolean;ipv6:boolean;routes:boolean}}
export interface Snapshot {settings:Settings;core:CoreState;runtime:{cleanupPending:boolean;status:string;pid:number|null;version:string|null;error:string|null};runtimeBusy:boolean;storageError:string|null;closing:boolean;logs:{receivedAt:number;source:string;message:string}[]}
export interface Candidate {component:Component;version:string;target:string;notes:string;releaseUrl:string}
export interface UpdateItem {phase:string;candidate:Candidate|null;error:string|null;errorStage:string|null;downloadedBytes:number;totalBytes:number|null;network:{endpoint:string;attempt:number;error:string|null}|null}
export interface UpdateSnapshot {items:Partial<Record<Component,UpdateItem>>;lastCheckedAt:number;nextCheckAt:number;checking:boolean;initialized:boolean}
export interface Mirror {id:string;prefix:string;supportsApi:boolean}
export const core = {
  snapshot:()=>invoke<Snapshot>('core_snapshot'),
  action:(action:'start'|'stop'|'check'|'apply')=>invoke('core_action',{action}),
  choose:(kind:'core'|'config')=>invoke('choose_file',{kind}),
  ready:(document:string)=>invoke('frontend_ready',{document}),
  updates:()=>invoke<UpdateSnapshot>('update_snapshot'),
  check:(component:Component|null=null)=>invoke('check_updates',{component}),
  download:(components:Component[])=>invoke('download_updates',{components}),
  cancel:(component:Component)=>invoke('cancel_update',{component}),
  save:(expectedRevision:number,preferences:Preferences)=>invoke<Settings>('set_update_preferences',{expectedRevision,preferences}),
  diagnose:(prefix:string|null)=>invoke<{name:string;ok:boolean;elapsedMs:number;error:string|null}[]>('test_update_mirror',{prefix}),
  mirrors:()=>invoke<Mirror[]>('builtin_mirrors'),
  restart:()=>invoke('request_restart'),
  maintain:(action:'check'|'update'|'retry')=>invoke('core_maintenance',{action}),
  cancelCore:()=>invoke('cancel_core_maintenance'),
  managed:()=>invoke('use_managed_core'),
  release:(component:Component)=>invoke('open_update_release',{component}),
  dirty:(dirty:boolean)=>invoke('set_update_draft_dirty',{dirty}),
  exit:()=>invoke('request_exit'),
  profilePreview:(input:string)=>invoke<{ticket:number;items:{line:number;node:NodeSummary|null;error:string|null;duplicate:boolean}[]}>('profile_import_preview',{input}),
  profileCancel:()=>invoke('profile_import_cancel'),
  profileCommit:(ticket:number,expectedRevision:number)=>invoke<ProfileSnapshot>('profile_import_commit',{ticket,expectedRevision}),
  profileSelect:(id:string,expectedRevision:number)=>invoke<ProfileSnapshot>('profile_select_node',{id,expectedRevision}),
  profileRename:(id:string,name:string,expectedRevision:number)=>invoke<ProfileSnapshot>('profile_rename_node',{id,name,expectedRevision}),
  profileDelete:(id:string,expectedRevision:number)=>invoke<ProfileSnapshot>('profile_delete_node',{id,expectedRevision}),
  profileRouting:(routing:Routing,expectedRevision:number)=>invoke<ProfileSnapshot>('profile_set_routing',{routing,expectedRevision}),
  profileMode:(mode:Profile['mode'],expectedRevision:number)=>invoke<ProfileSnapshot>('profile_mode',{mode,expectedRevision}),
  profileUpdateRules:(expectedRevision:number)=>invoke<ProfileSnapshot>('profile_update_rules',{expectedRevision}),
  networkSettings:()=>invoke('open_network_settings'),
};
