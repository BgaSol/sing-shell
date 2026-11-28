import type { CoreState, Snapshot } from "./api";
export function authorizationLabel(stage: CoreState["authorizationStage"]) {
  return stage
    ? {
        registration: "等待系统网络授权",
        activation: "正在等待网络进程就绪",
        authentication: "正在核对网络进程身份",
        protocol: "正在确认网络控制会话",
      }[stage]
    : "正在请求系统网络授权";
}

export function connectionReady(s: Pick<Snapshot, "core" | "runtime">) {
  return s.core.phase === "running" && s.runtime.pid !== null;
}
