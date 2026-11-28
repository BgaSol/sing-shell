import {
  core,
  type Snapshot,
  type UpdateSnapshot,
  type Component,
} from "./api";
import { element } from "./dom";
import { label, downloadable, working } from "./update-model";
import { createCoreStatus } from "./core-status";
import { editDownloadNetwork } from "./download-network";
import type { Modal } from "./modal";
export function createUpdates(modal: Modal, refresh: () => Promise<void>) {
  const coreStatus = createCoreStatus("update-core"),
    jobs = new Set<Promise<unknown>>();
  let snapshot: Snapshot | null = null,
    state: UpdateSnapshot | null = null;
  const pending = new Set<Component>(),
    issue = element("update-error");
  function run(task: () => Promise<unknown>) {
    const p = task()
      .catch((e) => {
        issue.textContent = String(e);
        issue.hidden = false;
      })
      .finally(() => jobs.delete(p));
    jobs.add(p);
    return p;
  }
  for (const [id, component] of [
    ["automatic", "app"],
    ["automatic-core", "engine"],
  ] as const) {
    const box = element<HTMLInputElement>(id);
    box.onchange = () => {
      if (!snapshot || pending.has(component)) return;
      const expected =
        component === "app"
          ? snapshot.settings.updatePreferences.automatic
          : snapshot.settings.updatePreferences.automaticCore;
      pending.add(component);
      box.disabled = true;
      void run(async () => {
        try {
          await core.automatic(component, expected, box.checked);
        } finally {
          pending.delete(component);
          await refresh();
        }
      });
    };
  }
  element("edit-network").onclick = () => {
    if (snapshot)
      void run(() =>
        editDownloadNetwork(
          modal,
          snapshot!.settings.updatePreferences.network,
          refresh,
        ),
      );
  };
  element("check-updates").onclick = () =>
    void run(async () => {
      issue.hidden = true;
      element<HTMLButtonElement>("check-updates").disabled = true;
      const checks: Promise<unknown>[] = [];
      if (state?.initialized && !state.checking && !working(state.items.app))
        checks.push(core.check("app"));
      if (
        snapshot?.settings.coreSource.mode === "managed" &&
        !["checking", "downloading", "preparing"].includes(
          snapshot.core.maintenance.phase,
        )
      )
        checks.push(core.maintain("check"));
      const results = await Promise.allSettled(checks);
      const failures = results.flatMap((r) =>
        r.status === "rejected" ? [String(r.reason)] : [],
      );
      if (failures.length) {
        issue.textContent = failures.join("\n");
        issue.hidden = false;
      }
      await refresh();
      render(await core.updates());
    });
  element("download-selected").onclick = () =>
    void run(() => core.download(["app"]));
  element("cancel-app-update").onclick = () =>
    void run(() => core.cancel("app"));
  element("apply-updates").onclick = () => void run(core.restart);
  element("app-release").onclick = () => void run(() => core.release("app"));
  function render(value: UpdateSnapshot) {
    state = value;
    const app = value.items.app;
    element("app-update-state").textContent = label(app);
    element("app-update-version").textContent =
      `${element("version").textContent}${app?.candidate ? ` → ${app.candidate.version}` : ""}`;
    element("app-update-error").textContent = app?.error ?? "";
    element("app-update-error").hidden = !app?.error;
    element("app-update-notes").textContent =
      app?.candidate?.notes ?? "检查到新版后显示版本说明。";
    const progress = element<HTMLProgressElement>("app-update-progress");
    progress.hidden = !working(app) || app?.phase === "checking";
    if (app?.totalBytes) {
      progress.max = app.totalBytes;
      progress.value = app.downloadedBytes;
    } else progress.removeAttribute("value");
    element("download-selected").hidden = !downloadable(app);
    element("apply-updates").hidden = app?.phase !== "ready";
    element("cancel-app-update").hidden = ![
      "downloading",
      "preparing",
    ].includes(app?.phase ?? "");
    element("app-release").hidden = app?.phase !== "unsupported";
    element("pending-updates").textContent =
      app?.phase === "ready"
        ? "重启更新会断开当前连接。"
        : app?.phase === "unsupported"
          ? "此安装形式请通过系统安装方式更新。"
          : "";
    element("checked-at").textContent = value.lastCheckedAt
      ? `应用最近检查：${new Date(value.lastCheckedAt).toLocaleString()}`
      : "";
    element<HTMLButtonElement>("check-updates").disabled =
      !value.initialized || value.checking;
    element("update-notice").textContent =
      app?.phase === "ready" ? "设置 · 更新" : "设置";
  }
  return {
    render,
    runtime(s: Snapshot) {
      snapshot = s;
      coreStatus.render(s);
      for (const [id, c] of [
        ["automatic", "app"],
        ["automatic-core", "engine"],
      ] as const) {
        const box = element<HTMLInputElement>(id);
        if (!pending.has(c))
          box.checked =
            c === "app"
              ? s.settings.updatePreferences.automatic
              : s.settings.updatePreferences.automaticCore;
        box.disabled =
          pending.has(c) ||
          s.closing ||
          !!s.storageError ||
          (c === "engine" && s.settings.coreSource.mode !== "managed");
      }
    },
    wait: async () => {
      await Promise.all([...jobs]);
    },
  };
}
