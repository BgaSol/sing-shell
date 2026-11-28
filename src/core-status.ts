import { core, type Snapshot } from "./api";
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
  let maintenance: "retry" | "update" = "update";
  retry.onclick = () => void action(() => core.maintain(maintenance));
  cancel.onclick = () => void action(core.cancelCore);
  managed.onclick = () => void action(core.managed);
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
        !p.managed || (!p.error && !m.candidate && m.phase !== "paused");
      retry.disabled = !p.retry;
      retry.textContent = p.retryLabel;
      maintenance = p.action;
      cancel.hidden = !p.cancel;
      managed.hidden = p.managed;
      managed.disabled = !s.core.canImport;
    },
  };
}
