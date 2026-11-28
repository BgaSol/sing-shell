import "./style.css";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { core, type Snapshot } from "./api";
import { ConfigurationReader } from "./app-state";
import { element } from "./dom";
import { Modal } from "./modal";
import { createShell } from "./shell";
import { createNodes } from "./nodes";
import { createRouting } from "./routing";
import { createActivity } from "./activity";
import { createUpdates } from "./updates";
import { createStartupSettings } from "./startup-settings";
import { createConnectionSettings } from "./connection-settings";
import { createDesktop } from "./desktop";
import { initializeWindow } from "./window";
const modal = new Modal();
let page = "nodes",
  setting = "general",
  ready = false,
  tick = 0,
  lastDirty = false,
  modeTask: Promise<void> | null = null;
const reader = new ConfigurationReader(core.snapshot, render);
const changed = () => reader.refresh();
const beforeNetwork = async () => {
  await Promise.all([routing.wait(), connection.wait()]);
};
const shell = createShell(
  changed,
  beforeNetwork,
  (target) => void navigate(target),
);
const nodes = createNodes(modal, changed, shell.error, beforeNetwork);
const routing = createRouting(modal, changed, shell.error);
const activity = createActivity(shell.error);
const updates = createUpdates(modal, changed);
const startup = createStartupSettings();
const connection = createConnectionSettings(changed);
const desktop = createDesktop(navigate, async () => {
  await Promise.all([
    routing.wait(),
    connection.wait(),
    startup.wait(),
    updates.wait(),
    modeTask,
  ]);
  return modal.mayLeave(true);
});
function render(s: Snapshot) {
  shell.render(s);
  nodes.render(s);
  routing.render(s);
  activity.render(s);
  updates.runtime(s);
  startup.render(s);
  connection.render(s);
  const mode = element<HTMLSelectElement>("profile-mode");
  if (!modeTask) mode.value = s.settings.profile.mode;
  mode.disabled = !!modeTask || s.runtimeBusy || s.closing || !!s.storageError;
}
function showSetting(id: string) {
  if (!["general", "updates", "advanced"].includes(id)) return;
  setting = id;
  for (const name of ["general", "updates", "advanced"])
    element(`settings-${name}`).hidden = name !== id;
  for (const b of element("settings-nav").querySelectorAll<HTMLButtonElement>(
    "button",
  ))
    b.setAttribute("aria-pressed", String(b.dataset.setting === id));
  if (id === "general") void startup.refresh();
}
async function navigate(target: string) {
  if (
    !["nodes", "routing", "activity", "settings"].includes(target) ||
    target === page
  )
    return;
  if (!(await modal.mayLeave())) return;
  page = target;
  for (const name of ["nodes", "routing", "activity", "settings"]) {
    element(`${name}-panel`).hidden = name !== target;
    element(`nav-${name}`).setAttribute(
      "aria-current",
      name === target ? "page" : "false",
    );
  }
  activity.setVisible(target === "activity");
  if (target === "settings") showSetting(setting);
  if (target === "routing") await routing.refreshSources();
}
for (const id of ["nodes", "routing", "activity", "settings"])
  element(`nav-${id}`).onclick = () => void navigate(id).catch(shell.error);
for (const b of element("settings-nav").querySelectorAll<HTMLButtonElement>(
  "button",
))
  b.onclick = () => showSetting(b.dataset.setting!);
element<HTMLSelectElement>("profile-mode").onchange = (e) => {
  const snapshot = reader.value;
  if (!snapshot || modeTask) return;
  const mode = (e.target as HTMLSelectElement).value as
    | "generated"
    | "externalConfig";
  (e.target as HTMLSelectElement).value = snapshot.settings.profile.mode;
  modeTask = (async () => {
    if (
      snapshot.runtime.pid ||
      (mode === "generated" && snapshot.settings.coreSource.mode === "external")
    ) {
      const answer = await modal.decision(
        "切换运行方式",
        mode === "generated" && snapshot.settings.coreSource.mode === "external"
          ? "当前连接将断开，并改用官方受管核心。外部核心文件保持原样。"
          : "当前连接将断开，切换后由你重新启动。",
      );
      if (answer !== "save") return;
    }
    element("mode-result").textContent = "正在切换…";
    try {
      await core.switchMode(mode, snapshot.settings.nodesRevision);
      element("mode-result").textContent = "已切换，选择节点或配置后启动。";
    } catch (e) {
      element("mode-result").textContent = String(e);
    }
  })().finally(() => {
    modeTask = null;
    void changed().catch(shell.error);
  });
};
element("quit-app").onclick = () => void core.exit().catch(shell.error);
void getVersion()
  .then((v) => (element("version").textContent = `Sing Shell ${v}`))
  .catch(shell.error);
void initializeWindow(shell.error).catch(shell.error);
void listen<string>("shutdown-error", (e) => shell.error(e.payload));
void listen(
  "desktop-changed",
  () => void desktop.refresh(reader.value?.closing),
);
async function refresh() {
  const session = async () => {
    if (!reader.value) await changed();
    else if (!reader.updateSession(await core.session())) await changed();
    if (!ready) {
      await desktop.ready();
      ready = true;
    }
  };
  const results = await Promise.allSettled([
    session(),
    desktop.refresh(reader.value?.closing),
  ]);
  for (const result of results)
    if (result.status === "rejected") shell.error(result.reason);
  const dirty = modal.pending;
  if (dirty !== lastDirty) {
    lastDirty = dirty;
    await core.dirty(dirty).catch(shell.error);
  }
  if (++tick % 3 === 0) {
    await core
      .updates()
      .then(updates.render)
      .catch((e) => {
        element("update-error").textContent = String(e);
        element("update-error").hidden = false;
      });
    if (page === "routing") await routing.refreshSources();
  }
  if (page === "activity") await activity.refresh();
  window.setTimeout(
    () => void refresh().catch(shell.error),
    document.hidden ? 2000 : 1000,
  );
}
void routing.refreshSources();
void refresh().catch(shell.error);
