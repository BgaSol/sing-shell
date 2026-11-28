import { getCurrentWindow } from "@tauri-apps/api/window";
import { element } from "./dom";
export async function initializeWindow(error: (e: unknown) => void) {
  if (!("__TAURI_INTERNALS__" in window) || "__SING_SHELL_FIXTURE__" in window)
    return;
  const win = getCurrentWindow();
  const mac = /Mac/.test(navigator.platform);
  document.documentElement.classList.toggle("macos", mac);
  element("window-close").onclick = () => void win.close().catch(error);
  element("window-minimize").onclick = () => void win.minimize().catch(error);
  element("window-maximize").onclick = () =>
    void win.toggleMaximize().catch(error);
  if (!mac) {
    const update = async () => {
      element("window-maximize").setAttribute(
        "aria-label",
        (await win.isMaximized()) ? "还原窗口" : "最大化",
      );
    };
    await win.onResized(() => void update().catch(error));
    await update();
  }
}
