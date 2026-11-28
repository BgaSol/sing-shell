import { core, type Snapshot } from "./api";
import { element } from "./dom";
export function createConnectionSettings(refresh: () => Promise<void>) {
  const box = element<HTMLInputElement>("ipv4-compatibility"),
    result = element("connection-settings-result");
  let snapshot: Snapshot | null = null,
    task: Promise<void> | null = null;
  box.onchange = () => {
    if (!snapshot || task) return;
    const expected = snapshot.settings.connectionRevision,
      desired = box.checked;
    box.disabled = true;
    result.textContent = "正在保存…";
    task = (async () => {
      try {
        await core.saveConnection(expected, { ipv4Compatibility: desired });
        result.textContent = "已保存，下次连接或应用并重连后生效";
      } catch (e) {
        result.textContent = String(e);
      } finally {
        task = null;
        await refresh();
      }
    })();
  };
  return {
    render(value: Snapshot) {
      snapshot = value;
      if (!task)
        box.checked = value.settings.profile.connection.ipv4Compatibility;
      box.disabled =
        !!task ||
        value.closing ||
        !!value.storageError ||
        value.settings.profile.mode !== "generated";
    },
    wait: async () => {
      if (task) await task;
    },
  };
}
