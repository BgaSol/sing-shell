import { authorizationLabel, connectionReady } from "./session-model";
import { core, type Snapshot } from "./core";
import { element } from "./dom";
export { element } from "./dom";
export function createShell(changed: () => Promise<void>) {
  let localError: string | null = null;
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
  element("stop").onclick = () => void action(() => core.action("stop"));
  element("start").onclick = () => void action(() => core.action("start"));
  element("check").onclick = () => void action(() => core.action("check"));
  element("apply-connection").onclick = () =>
    void action(() => core.action("apply"));
  element("choose-core").onclick = () => void action(() => core.choose("core"));
  element("choose-config").onclick = () =>
    void action(() => core.choose("config"));
  element("reset-settings").onclick = () => {
    void action(async () => {
      const backup = await core.reset();
      element("reset-result").textContent = `已备份到 ${backup}`;
    });
  };
  return {
    error,
    render(s: Snapshot) {
      const c = s.core,
        raw = s.settings.profile.mode === "externalConfig",
        running = connectionReady(s),
        busy = s.runtimeBusy || s.closing;
      const label =
        c.phase === "authorizing"
          ? authorizationLabel(c.authorizationStage)
          : ((
              {
                running: "TUN 运行中",
                preparing: "正在准备",
                starting: "正在启动",
                stopping: "正在断开",
                stopped: "尚未连接",
                failed: "连接失败",
                cleanupRequired: "需要清理网络",
              } as Record<string, string>
            )[c.phase] ?? c.phase);
      element("status").textContent = raw && running ? "核心运行中" : label;
      element("status").dataset.state = c.phase;
      document.querySelector<HTMLElement>(".connection-bar")!.dataset.state =
        running ? "running" : c.phase;
      element("connection-node").textContent = raw
        ? "原始配置模式"
        : c.target
          ? `正在连接 ${c.target.name}`
          : running
            ? (c.runningNode?.name ?? "连接运行中")
            : "尚未连接";
      element("connection-summary").textContent =
        c.operation && running
          ? `当前连接继续使用 ${c.runningNode?.name ?? "原设置"}，候选准备中`
          : c.pendingChanges
            ? "已保存内容待应用，当前连接保持原设置"
            : c.phase === "cleanupRequired"
              ? "网络资源未释放，请重试断开"
              : raw
                ? "配置由 sing-box 校验，停止后才能更换。"
                : running
                  ? "TUN 双栈、路由与临时 DNS 已确认"
                  : "点击节点播放按钮连接；本机与局域网固定直连。";
      element<HTMLButtonElement>("stop").disabled = !c.canStop;
      element("start").hidden = !raw;
      element<HTMLButtonElement>("start").disabled = !c.canStart;
      element("check").hidden = !raw;
      element<HTMLButtonElement>("check").disabled =
        busy || running || !c.usable;
      element("apply-connection").hidden = raw || !running || !c.pendingChanges;
      element<HTMLButtonElement>("apply-connection").disabled = busy;
      element<HTMLButtonElement>("choose-core").disabled = !c.canImport;
      element<HTMLButtonElement>("choose-config").disabled = !c.canImport;
      element("external-config-row").hidden = !raw;
      element("config-path").textContent = s.settings.pendingConfigPath
        ? `${s.settings.pendingConfigPath} · 等待校验`
        : (s.settings.configPath ?? "尚未导入配置");
      const issue =
        s.storageError ?? c.configError ?? s.runtime.error ?? localError;
      element("error").textContent = issue ?? "";
      element("error").hidden = !issue;
      element("storage-recovery").hidden = !s.storageError;
    },
  };
}
