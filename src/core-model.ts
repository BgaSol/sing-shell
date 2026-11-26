import type { Snapshot } from "./core";
export function corePresentation(s: Snapshot) {
  const managed = s.settings.coreSource.mode === "managed", m = s.core.maintenance;
  const busy = ["checking", "downloading", "preparing"].includes(m.phase);
  const installed = s.settings.managed.active?.version;
  const version = managed
    ? `应用管理 · sing-box ${installed ?? (m.preparedVersion ? m.preparedVersion + " 待采用" : "尚未安装")}${s.runtime.pid && s.runtime.version ? ` · 正在运行 ${s.runtime.version}` : ""}`
    : `本机核心 · ${s.settings.coreSource.mode === "external" ? s.settings.coreSource.path : ""}`;
  let state = !s.core.initialized ? "正在恢复核心状态" : !managed ? "外部核心由你手动管理" : ({
    checking: "正在查找官方核心",
    downloading: `正在下载核心 · ${(m.downloadedBytes / 1048576).toFixed(1)} MiB${m.totalBytes ? ` / ${(m.totalBytes / 1048576).toFixed(1)} MiB` : ""}`,
    preparing: "正在校验核心",
    ready: s.runtime.pid ? `sing-box ${m.preparedVersion ?? "新版"} 已准备，下次启动生效` : `sing-box ${m.preparedVersion ?? "核心"} 已准备，等待采用`,
    paused: "核心维护已暂停，可明确重试",
    error: s.core.usable ? "更新未完成，当前核心仍可使用" : "核心准备未完成",
    idle: s.core.usable ? "核心已就绪" : "正在准备核心",
  } as const)[m.phase];
  if (managed && !busy && m.phase === "idle") {
    if (m.candidate && m.candidate.version !== installed) state = `可更新至 sing-box ${m.candidate.version}`;
    else if (s.core.notice) state = s.core.notice;
  }
  const retrying = !!m.error || m.phase === "paused";
  const retryLabel = m.failureKind === "compatibility" ? "重新校验" : retrying ? "重试准备核心" : m.preparedVersion ? "采用核心" : "更新核心";
  return { version, state, busy, managed, error: managed ? m.error ?? m.warning : null,
    retry: s.core.canRetry && managed && !busy, cancel: managed && busy,
    retryLabel, action: retrying ? "retry" as const : "update" as const };
}
