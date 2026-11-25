import "./style.css";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { core, type Snapshot } from "./core";

function element<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Missing element: ${id}`);
  return node as T;
}

const buttons = {
  chooseCore: element<HTMLButtonElement>("choose-core"),
  chooseConfig: element<HTMLButtonElement>("choose-config"),
  start: element<HTMLButtonElement>("start"),
  stop: element<HTMLButtonElement>("stop"),
  check: element<HTMLButtonElement>("check"),
};
const logs = element<HTMLPreElement>("logs");
const follow = element<HTMLInputElement>("follow");
const error = element<HTMLParagraphElement>("error");
let snapshot: Snapshot | null = null;
let busy = false;
let lastLogs = "";
let operationError: string | null = null;
let generation = 0;
let ready = false;

function render() {
  if (!snapshot) return;
  const running = snapshot.status === "running";
  const ready = !!snapshot.settings.corePath && !!snapshot.settings.configPath;
  const status = element("status");
  status.textContent = { running: "核心运行中", stopped: "核心已停止", failed: "需要处理" }[snapshot.status];
  status.dataset.state = snapshot.status;
  element("pid").textContent = snapshot.pid ? `PID ${snapshot.pid}` : "";
  element("core-path").textContent = snapshot.settings.corePath ?? "选择与你的系统及架构匹配的 sing-box 程序";
  element("config-path").textContent = snapshot.settings.configPath ?? "选择现有的 JSON 或 JSONC 配置文件";
  buttons.chooseCore.disabled = busy || running;
  buttons.chooseConfig.disabled = busy || running || !snapshot.settings.corePath;
  buttons.start.disabled = busy || running || !ready;
  buttons.stop.disabled = busy || !running;
  buttons.check.disabled = busy || !ready;
  error.textContent = operationError ?? snapshot.error ?? "";
  error.hidden = !error.textContent;
  const content = snapshot.logs.map(log => `${new Date(log.receivedAt).toLocaleTimeString()}  [${log.source}] ${log.message}`).join("\n");
  if (content !== lastLogs) {
    const scroll = logs.scrollTop;
    logs.textContent = content || "选择核心和配置文件后，点击「启动 sing-box」。";
    logs.scrollTop = follow.checked ? logs.scrollHeight : scroll;
    lastLogs = content;
  }
}

async function operate(label: string, task: () => Promise<Snapshot | null>) {
  if (busy) return;
  busy = true;
  generation += 1;
  operationError = null;
  element("activity").textContent = label;
  render();
  try {
    const result = await task();
    if (result) snapshot = result;
  } catch (reason) {
    operationError = String(reason);
  } finally {
    busy = false;
    element("activity").textContent = "";
    render();
  }
}

buttons.chooseCore.onclick = () => void operate("正在选择并检查核心…", () => core.choose("core"));
buttons.chooseConfig.onclick = () => void operate("正在导入并校验配置…", () => core.choose("config"));
buttons.start.onclick = () => void operate("正在校验并启动…", () => core.action("start"));
buttons.stop.onclick = () => void operate("正在停止…", () => core.action("stop"));
buttons.check.onclick = () => void operate("正在校验配置…", () => core.action("check"));
follow.onchange = () => { if (follow.checked) logs.scrollTop = logs.scrollHeight; };

async function refresh() {
  if (!busy) {
    const requestGeneration = generation;
    try {
      const next = await core.snapshot();
      if (requestGeneration === generation && !busy) {
        snapshot = next;
        render();
        if (!ready) { await core.ready(); ready = true; }
      }
    } catch (reason) {
      if (requestGeneration === generation && !busy) {
        error.textContent = `读取核心状态失败：${String(reason)}`;
        error.hidden = false;
      }
    }
  }
  window.setTimeout(() => void refresh(), 700);
}

void listen<string>("shutdown-error", event => {
  operationError = `无法退出：${event.payload}`;
  render();
});
void getVersion().then(version => { element("version").textContent = `Sing Shell ${version}`; }).catch(() => {});
void refresh();
