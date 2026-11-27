import {
  core,
  type Snapshot,
  type Connection,
  type ActivitySummary,
  type SessionEvent,
  type Log,
} from "./core";
import { element, el, button, empty, bytes } from "./dom";
export function createActivity(error: (e: unknown) => void) {
  let visible = false,
    inflight = false,
    generation = 0,
    sessionId: string | null = null,
    after: number | null = null,
    next: number | null = null,
    state: "all" | "active" | "ended" = "all",
    search = "",
    logCursor = 0,
    eventCursor = 0;
  const rows = new Map<string, HTMLDetailsElement>();
  const logs: Log[] = [],
    events: SessionEvent[] = [];
  const list = element("connection-list");
  const raw = element<HTMLDetailsElement>("raw-logs");
  const latest = button("返回最新", () => {
    after = null;
    void refresh();
  });
  latest.hidden = true;
  element("connection-filter").parentElement!.append(latest);
  function summary(s: ActivitySummary) {
    const stats = element("activity-stats");
    stats.replaceChildren();
    for (const [title, value] of [
      ["当前连接", String(s.active)],
      [
        "上传速率",
        s.uploadRate === null ? "采样中" : bytes(s.uploadRate) + "/s",
      ],
      [
        "下载速率",
        s.downloadRate === null ? "采样中" : bytes(s.downloadRate) + "/s",
      ],
      ["本次流量", `${bytes(s.upload)} / ${bytes(s.download)}`],
    ]) {
      const group = el("div");
      group.append(el("span", "", title), el("strong", "", value));
      stats.append(group);
    }
    element("activity-state").textContent =
      s.state === "unavailable"
        ? "当前没有可读取的核心连接明细。原始配置模式提供会话事件和诊断日志。"
        : s.state === "overflow"
          ? "连接或响应超过采样上限，部分连接未展示；不会把缺失项标为失败。"
          : s.state === "stale"
            ? (s.error ?? "等待活动采样…")
            : `${s.observed} 条已观测连接 · 每秒采样，短连接可能遗漏${s.sampledAt ? " · " + new Date(s.sampledAt).toLocaleTimeString() : ""}`;
  }
  function connectionRow(c: Connection) {
    let row = rows.get(c.id);
    if (!row) {
      row = el("details", "connection-detail");
      row.dataset.connection = c.id;
      row.append(el("summary"), el("dl"));
      rows.set(c.id, row);
    }
    const heading = row.querySelector("summary")!,
      detail = row.querySelector("dl")!;
    const info = el("div", "row-info");
    info.append(
      el("strong", "", c.target || "未知目标"),
      el(
        "p",
        "row-meta",
        `${c.protocol.toUpperCase() || "未知协议"} · ${c.exit === "proxy" ? "代理" : c.exit === "direct" ? "直连" : "出口未知"} · ${c.policyName ?? "未知规则"} · ${c.active ? "连接中" : "已结束"}`,
      ),
    );
    heading.replaceChildren(
      info,
      el(
        "span",
        "traffic",
        `上传 ${bytes(c.upload)}\n下载 ${bytes(c.download)}`,
      ),
    );
    detail.replaceChildren();
    for (const [key, value] of [
      ["命中规则", c.policyName ?? "未知"],
      ["开始时间", c.started ? new Date(c.started).toLocaleString() : "未知"],
      ["进程", c.process ?? "不可用"],
      ["核心描述", c.rule || "不可用"],
      ["连接 ID", c.id],
      [
        "状态说明",
        c.active
          ? "当前采样可见"
          : "已从完整采样中消失；结束原因与最终字节可能未知",
      ],
    ])
      detail.append(el("dt", "", key), el("dd", "", value));
    return row;
  }
  async function refresh() {
    if (!visible || document.hidden || inflight) return;
    inflight = true;
    const token = generation,
      requested = sessionId;
    try {
      const page = await core.activity({
        sessionId,
        after,
        state,
        search,
        limit: 100,
      });
      if (token !== generation || !visible || requested !== sessionId) return;
      summary(page.summary);
      if (page.summary.sessionId && page.summary.sessionId !== sessionId) {
        sessionId = page.summary.sessionId;
        rows.clear();
      }
      next = page.next;
      const keep = new Set(page.items.map((c) => c.id));
      for (const [id, row] of rows)
        if (!keep.has(id)) {
          row.remove();
          rows.delete(id);
        }
      if (!page.items.length) {
        rows.clear();
        list.replaceChildren(
          empty(
            "暂无连接记录",
            page.summary.state === "unavailable"
              ? "连接节点后，可在这里查看已观测连接。"
              : "尝试访问网站，或更换筛选条件。",
          ),
        );
      } else {
        for (const child of list.querySelectorAll(".empty")) child.remove();
        for (const c of page.items) list.append(connectionRow(c));
      }
      element("more-connections").hidden = next === null;
      latest.hidden = after === null;
      const incoming = await core.events(eventCursor);
      if (token !== generation) return;
      for (const event of incoming) {
        eventCursor = Math.max(eventCursor, event.sequence);
        events.push(event);
      }
      events.splice(0, Math.max(0, events.length - 1000));
      const eventList = element("event-list");
      eventList.replaceChildren();
      for (const event of events.slice(-50).reverse()) {
        const row = el("div", "event");
        row.append(
          el("span", "", new Date(event.at).toLocaleTimeString()),
          document.createTextNode(
            `${event.outcome === "failed" ? "失败 · " : ""}${event.message}`,
          ),
        );
        eventList.append(row);
      }
      if (!events.length) eventList.append(el("p", "hint", "尚无会话事件。"));
      if (raw.open) {
        const incoming = await core.logs(logCursor);
        if (token !== generation) return;
        for (const log of incoming) {
          if (log.sequence > logCursor) {
            logs.push(log);
            logCursor = log.sequence;
          }
        }
        logs.splice(0, Math.max(0, logs.length - 1000));
        const output = element("logs"),
          position = output.scrollTop;
        output.textContent =
          logs
            .map(
              (l) =>
                `${new Date(l.receivedAt).toLocaleTimeString()} [${l.source}] ${l.message}`,
            )
            .join("\n") || "暂无日志";
        output.scrollTop = element<HTMLInputElement>("follow").checked
          ? output.scrollHeight
          : position;
      }
    } catch (e) {
      if (token === generation)
        element("activity-state").textContent = String(e);
    } finally {
      inflight = false;
    }
  }
  element("more-connections").onclick = () => {
    after = next;
    void refresh();
  };
  element<HTMLSelectElement>("connection-filter").onchange = (e) => {
    state = (e.target as HTMLSelectElement).value as typeof state;
    after = null;
    generation++;
    void refresh();
  };
  let timer: number | undefined;
  element<HTMLInputElement>("connection-search").oninput = (e) => {
    search = (e.target as HTMLInputElement).value.trim();
    after = null;
    generation++;
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void refresh(), 200);
  };
  const openLogs = () => {
    raw.open = true;
    raw.scrollIntoView({ block: "nearest" });
    void refresh();
  };
  element("show-logs").onclick = openLogs;
  raw.ontoggle = () => {
    if (raw.open) void refresh();
  };
  return {
    render(s: Snapshot) {
      element("activity-bypass").hidden = !s.core.runningBypass;
      const id = s.core.sessionId ?? s.core.activity?.sessionId ?? null;
      if (id !== sessionId) {
        sessionId = id;
        generation++;
        after = null;
        rows.clear();
        list.replaceChildren();
        logs.length = 0;
        logCursor = 0;
      }
    },
    setVisible(value: boolean) {
      if (visible !== value) {
        visible = value;
        generation++;
      }
      if (visible) void refresh();
    },
    refresh,
    openLogs,
    error,
  };
}
