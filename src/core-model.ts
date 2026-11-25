import type {Snapshot} from './core';
export function corePresentation(s:Snapshot){
  const managed=s.settings.coreSource.mode==='managed',m=s.core.maintenance;
  const busy=['checking','downloading','preparing'].includes(m.phase);
  const installed=s.settings.managed.active?.version;
  const version=managed?`应用管理 · sing-box ${installed??'尚未安装'}${s.runtime.pid&&s.runtime.version?` · 正在运行 ${s.runtime.version}`:''}`:`本机核心 · ${s.settings.coreSource.mode==='external'?s.settings.coreSource.path:''}`;
  let state=!s.core.initialized?'正在恢复核心状态':!managed?'外部核心由你手动管理':({checking:'正在查找官方核心',downloading:`正在下载核心 · ${(m.downloadedBytes/1048576).toFixed(1)} MiB${m.totalBytes?` / ${(m.totalBytes/1048576).toFixed(1)} MiB`:''}`,preparing:'正在校验核心',ready:s.settings.updatePreferences.automaticCore?'新版已准备，下次启动生效':'已准备，自动采用已暂停',paused:'核心维护已暂停',error:s.core.usable?'更新未完成，当前核心仍可使用':'核心准备未完成',idle:s.core.usable?'核心已就绪':'正在准备核心'} as const)[m.phase];
  if(managed&&!busy&&m.phase==='idle'&&s.core.notice)state=s.core.notice;
  return {version,state,busy,managed,error:managed?m.error:null,retry:s.core.canRetry&&managed&&!busy,cancel:managed&&busy};
}
