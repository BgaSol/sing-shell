import { authorizationLabel, connectionReady } from "./session-model";
import { core, type Snapshot } from "./api";
import { element } from "./dom";
export function createShell(
  changed: () => Promise<void>,
  beforeNetwork: () => Promise<void>,
  navigate: (page: string) => void,
) {
  let snapshot: Snapshot | null = null,
    localError: string | null = null;
  const error = (reason: unknown) => {
    localError = String(reason);
    element("error").textContent = localError;
    element("error").hidden = false;
  };
  const action = async (task: () => Promise<unknown>) => {
    localError = null;
    try {
      await task();
      await changed();
    } catch (e) {
      error(e);
    }
  };
  const applyConnection = () =>
    action(async () => {
      await beforeNetwork();
      await core.action("apply");
    });
  element("apply-connection").onclick = () => void applyConnection();
  element("stop").onclick = () => void action(() => core.action("stop"));
  element("start").onclick = () => {
    if (!snapshot) return;
    if (
      snapshot.settings.profile.mode === "generated" &&
      !snapshot.settings.profile.selectedNode
    ) {
      navigate("nodes");
      return;
    }
    void action(async () => {
      await beforeNetwork();
      await core.action("start");
    });
  };
  element("choose-core").onclick = () => void action(() => core.choose("core"));
  element("choose-config").onclick = () =>
    void action(() => core.choose("config"));
  for (const id of ["advanced-managed", "update-core-managed"])
    element(id).onclick = () => void action(core.managed);
  element("reset-settings").onclick = () =>
    void action(async () => {
      const backup = await core.reset();
      element("reset-result").textContent = `已备份到 ${backup}`;
    });
  return {
    error,
    applyConnection,
    render(s: Snapshot) {
      snapshot = s;
      const c = s.core,
        raw = s.settings.profile.mode === "externalConfig",
        running = connectionReady(s),
        busy = s.runtimeBusy || s.closing;
      const label =
        c.phase === "authorizing"
          ? authorizationLabel(c.authorizationStage)
          : ((
              {
                running: raw ? "运行中" : "已连接",
                preparing: "正在准备",
                starting: "正在连接",
                stopping: "正在断开",
                stopped: "未连接",
                failed: "连接失败",
                cleanupRequired: "待清理",
                checking: "正在校验",
              } as Record<string, string>
            )[c.phase] ?? "正在读取");
      element("status").textContent = label;
      element("status").dataset.state = c.phase;
      element("connection-node").textContent = raw
        ? "原始配置"
        : (c.runningNode?.name ??
          c.target?.name ??
          s.settings.profile.nodes.find(
            (n) => n.id === s.settings.profile.selectedNode,
          )?.name ??
          "选择一个节点开始");
      element("connection-summary").textContent = c.pendingChanges
        ? "更改待生效"
        : c.target && running
          ? `正在准备 ${c.target.name}，当前连接继续运行`
          : running
            ? c.runningConnectionOptions?.ipv4Compatibility
              ? "TUN · IPv4 兼容"
              : "TUN 已就绪"
            : c.phase === "cleanupRequired"
              ? "网络尚未释放，请重试清理"
              : "";
      const stop = element<HTMLButtonElement>("stop");
      stop.hidden = !c.canStop;
      stop.disabled = !c.canStop;
      stop.textContent =
        c.phase === "cleanupRequired"
          ? "重试清理"
          : !running && busy && !c.runningNode && !s.runtime.pid
            ? "取消"
            : raw
              ? "停止"
              : "断开";
      const start = element<HTMLButtonElement>("start");
      start.hidden = c.canStop;
      start.disabled =
        busy ||
        !!s.storageError ||
        (raw
          ? !c.canStart
          : s.settings.profile.selectedNode
            ? !c.canConnect
            : false);
      start.textContent = raw
        ? "启动配置"
        : s.settings.profile.selectedNode
          ? "连接"
          : "选择节点";
      element("apply-connection").hidden =
        raw || !running || (!c.pendingChanges && !c.runningBypass);
      element<HTMLButtonElement>("apply-connection").disabled = busy;
      for (const id of ["choose-core", "choose-config", "advanced-managed"])
        element<HTMLButtonElement>(id).disabled = !c.canImport;
      element("external-config-row").hidden = !raw;
      element("config-path").textContent = s.settings.pendingConfigPath
        ? `${s.settings.pendingConfigPath} · 等待校验`
        : (s.settings.configPath ?? "尚未选择配置");
      element("external-core-path").textContent =
        s.settings.coreSource.mode === "external"
          ? s.settings.coreSource.path
          : "应用管理的官方 sing-box";
      const issue =
        s.storageError ?? c.configError ?? s.runtime.error ?? localError;
      element("error").textContent = issue ?? "";
      element("error").hidden = !issue;
      element("storage-recovery").hidden = !s.storageError;
    },
  };
}
