import { core, type Snapshot } from "./core";
import { element } from "./dom";
import { corePresentation } from "./core-model";
export function createCoreStatus(prefix: string) {
  const version = element(`${prefix}-version`),
    state = element(`${prefix}-state`),
    error = element(`${prefix}-error`),
    progress = element<HTMLProgressElement>(`${prefix}-progress`),
    retry = element<HTMLButtonElement>(`${prefix}-retry`),
    cancel = element<HTMLButtonElement>(`${prefix}-cancel`),
    managed = element<HTMLButtonElement>(`${prefix}-managed`);
  const action = async (task: () => Promise<unknown>) => {
    try {
      await task();
    } catch (e) {
      error.textContent = String(e);
      error.hidden = false;
    }
  };
  retry.onclick = () => void action(() => core.maintain("retry"));
  cancel.onclick = () => void action(core.cancelCore);
  managed.onclick = () => void action(core.managed);
  if (prefix === "update-core")
    element("core-check").onclick = () =>
      void action(() => core.maintain("check"));
  return {
    render(s: Snapshot) {
      const p = corePresentation(s),
        m = s.core.maintenance;
      version.textContent = p.version;
      state.textContent = p.state;
      error.textContent = p.error ?? "";
      error.hidden = !p.error;
      progress.hidden =
        !p.managed || !["downloading", "preparing"].includes(m.phase);
      if (m.totalBytes) {
        progress.max = m.totalBytes;
        progress.value = m.downloadedBytes;
      } else progress.removeAttribute("value");
      retry.hidden =
        prefix === "home-core" ? !p.retry || s.core.usable : !p.managed;
      retry.disabled = !p.retry;
      cancel.hidden = !p.cancel;
      managed.hidden = p.managed;
      managed.disabled = !s.core.canImport;
      if (prefix === "update-core")
        element<HTMLButtonElement>("core-check").disabled = !p.retry;
    },
  };
}
