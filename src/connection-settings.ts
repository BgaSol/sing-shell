import { core, type Snapshot } from "./core";
import { element } from "./dom";
import type { Modal } from "./modal";

export function createConnectionSettings(modal: Modal, refresh: () => Promise<void>) {
  const box = element<HTMLInputElement>("ipv4-compatibility"),
    button = element<HTMLButtonElement>("save-connection-settings"),
    reload = element<HTMLButtonElement>("reload-connection-settings"),
    result = element("connection-settings-result");
  let snapshot: Snapshot | null = null, revision = -1, saved = false, saving = false, rawMode = false, savedMessage = false;
  const dirty = () => revision >= 0 && box.checked !== saved;
  function load() {
    if (!snapshot) return;
    revision = snapshot.settings.connectionRevision;
    saved = snapshot.settings.profile.connection.ipv4Compatibility;
    box.checked = saved;
  }
  function state() {
    const raw = snapshot?.settings.profile.mode === "externalConfig";
    box.disabled = saving || !!snapshot?.closing || raw;
    button.disabled = box.disabled || !dirty();
    reload.hidden = !snapshot || revision === snapshot.settings.connectionRevision;
    if (raw) result.textContent = "由原始配置控制，切回节点模式后采用已保存的设置。";
    else if (rawMode) result.textContent = "";
    else if (savedMessage && !dirty()) result.textContent = snapshot?.runtime.pid
      ? snapshot.core.runningConnectionOptions?.ipv4Compatibility === saved ? "当前连接已生效" : "已保存，应用并重连后生效"
      : "已保存，下次连接生效";
    rawMode = raw;
  }
  async function save() {
    saving = true; state();
    try {
      const value = await core.saveConnection(revision, { ipv4Compatibility: box.checked });
      revision = value.connectionRevision;
      saved = value.profile.connection.ipv4Compatibility;
      await refresh();
      savedMessage = true;
      return true;
    } catch (e) { savedMessage = false; result.textContent = String(e); return false; }
    finally { saving = false; state(); }
  }
  box.onchange = () => { savedMessage = false; result.textContent = ""; state(); };
  button.onclick = () => void save();
  reload.onclick = () => void (async () => {
    if (dirty()) {
      const choice = await modal.decision("连接设置已改变", "保存会校对修订；放弃本次草稿后载入最新设置。");
      if (choice === "save") { await save(); return; }
      if (choice !== "discard") return;
    }
    load(); state();
  })();
  return {
    render(value: Snapshot) { snapshot = value; if (revision < 0 || (!dirty() && revision !== value.settings.connectionRevision)) load(); state(); },
    dirty,
    async mayLeave() {
      while (saving) await new Promise(resolve => window.setTimeout(resolve, 30));
      if (!dirty()) return true;
      const choice = await modal.decision("连接设置尚未保存", "保存连接设置，或放弃本次草稿后继续退出。");
      if (choice === "save") return save();
      if (choice === "discard") { load(); state(); return true; }
      return false;
    },
  };
}
