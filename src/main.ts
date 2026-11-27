import "./style.css";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { core, type Snapshot } from "./core";
import { element } from "./dom";
import { Modal } from "./modal";
import { createNodes } from "./nodes";
import { createRouting } from "./routing";
import { createActivity } from "./activity";
import { createShell } from "./shell";
import { createUpdates } from "./updates";
import { createStartupSettings } from "./startup-settings";
import { createDesktop } from "./desktop";
import { createConnectionSettings } from "./connection-settings";
const modal = new Modal();
let snapshot: Snapshot | null = null,
  configurationRequest: Promise<void> | null = null,
  ready = false,
  page = "nodes",
  setting = "general",
  queuedNavigation: string | null = null,
  lastDirty = false,
  tick = 0;
const shell = createShell(refreshConfiguration),
  nodes = createNodes(modal, refreshConfiguration, shell.error),
  routing = createRouting(modal, refreshConfiguration, shell.error, shell.applyConnection),
  activity = createActivity(shell.error),
  updates = createUpdates(),
  startup = createStartupSettings();
const connectionSettings = createConnectionSettings(modal, refreshConfiguration);
function render() {
  if (!snapshot) return;
  shell.render(snapshot);
  nodes.render(snapshot);
  routing.render(snapshot);
  activity.render(snapshot);
  updates.runtime(snapshot);
  startup.render(snapshot);
  connectionSettings.render(snapshot);
  const mode = element<HTMLSelectElement>("profile-mode");
  if (document.activeElement !== mode)
    mode.value = snapshot.settings.profile.mode;
  mode.disabled = !snapshot.core.canImport;
}
async function refreshConfiguration() {
  if (configurationRequest) return configurationRequest;
  configurationRequest = core
    .snapshot()
    .then((value) => {
      snapshot = value;
      render();
    })
    .finally(() => {
      configurationRequest = null;
    });
  return configurationRequest;
}
function showSetting(id: string) {
  if (!["general", "network", "core", "advanced"].includes(id)) return;
  setting = id;
  for (const name of ["general", "network", "core", "advanced"])
    element(`settings-${name}`).hidden = name !== id;
  for (const b of element("settings-nav").querySelectorAll<HTMLButtonElement>(
    "button",
  ))
    b.setAttribute("aria-current", b.dataset.setting === id ? "page" : "false");
  element("settings-title").textContent = (
    {
      general: "通用设置",
      network: "网络设置",
      core: "核心与更新",
      advanced: "高级设置",
    } as Record<string, string>
  )[id];
  if (id === "general") void startup.refresh();
}
function navigate(target: string) {
  if (!["nodes", "routing", "activity", "settings"].includes(target)) return;
  if (modal.open) {
    queuedNavigation = target;
    return;
  }
  page = target;
  for (const name of ["nodes", "routing", "activity", "settings"]) {
    element(`${name}-panel`).hidden = name !== target;
    element(`nav-${name}`).setAttribute(
      "aria-current",
      name === target ? "page" : "false",
    );
  }
  element("settings-nav").hidden = target !== "settings";
  element("nav-settings").setAttribute(
    "aria-expanded",
    String(target === "settings"),
  );
  activity.setVisible(target === "activity");
  if (target === "settings") showSetting(setting);
  if (target === "routing") void routing.refreshSources();
}
for (const id of ["nodes", "routing", "activity", "settings"])
  element(`nav-${id}`).onclick = () => {
    if (id === "settings" && page === "settings") {
      const menu = element("settings-nav");
      menu.hidden = !menu.hidden;
      element("nav-settings").setAttribute(
        "aria-expanded",
        String(!menu.hidden),
      );
    } else navigate(id);
  };
for (const b of element("settings-nav").querySelectorAll<HTMLButtonElement>(
  "button",
))
  b.onclick = () => showSetting(b.dataset.setting!);
const desktop = createDesktop(
  navigate,
  async () =>
    (await modal.mayLeave()) &&
    (await routing.mayLeave()) &&
    (await connectionSettings.mayLeave()) &&
    (await updates.mayLeave()),
);
element<HTMLSelectElement>("profile-mode").onchange = (e) => {
  if (!snapshot) return;
  const mode = (e.target as HTMLSelectElement).value as
    | "generated"
    | "externalConfig";
  void core
    .profileMode(mode, snapshot.settings.nodesRevision)
    .then(refreshConfiguration)
    .catch(shell.error);
};
element("open-diagnostics").onclick = () => {
  navigate("activity");
  activity.openLogs();
};
void listen<string>("shutdown-error", (event) => shell.error(event.payload));
void listen("desktop-changed", () => void desktop.refresh());
void getVersion()
  .then((v) => (element("version").textContent = `Sing Shell ${v}`))
  .catch(shell.error);
async function refresh() {
  try {
    if (!snapshot) await refreshConfiguration();
    else {
      const next = await core.session();
      const known = [
        snapshot.settings.nodesRevision,
        snapshot.settings.routingRevision,
        snapshot.settings.coreRevision,
        snapshot.settings.preferencesRevision,
        snapshot.settings.connectionRevision,
      ];
      if (JSON.stringify(next.revisions) !== JSON.stringify(known))
        await refreshConfiguration();
      else {
        snapshot = {
          ...snapshot,
          ...next,
          settings: { ...snapshot.settings, startup: next.startup },
        };
        render();
      }
    }
    if (!ready) {
      await desktop.ready();
      ready = true;
    }
    await desktop.refresh(snapshot?.closing);
    if (queuedNavigation && !modal.open) {
      const target = queuedNavigation;
      queuedNavigation = null;
      navigate(target);
    }
    const dirty = modal.pending || routing.dirty() || connectionSettings.dirty() || updates.dirty();
    if (dirty !== lastDirty) {
      lastDirty = dirty;
      await core.dirty(dirty);
    }
    if (++tick % 5 === 0) {
      updates.render(await core.updates());
      if (page === "routing") await routing.refreshSources();
    }
    if (page === "activity") await activity.refresh();
  } catch (e) {
    shell.error(e);
  } finally {
    window.setTimeout(() => void refresh(), document.hidden ? 2000 : 1000);
  }
}
void routing.refreshSources();
void refresh();
