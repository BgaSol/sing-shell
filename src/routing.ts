import "./routing.css";
import {
  core,
  type Snapshot,
  type Routing,
  type Policy,
  type RoutingChange,
  type RemovedRule,
} from "./api";
import { element, el, button, iconButton, errorText } from "./dom";
import { editRoutingPolicy } from "./routing-editor";
import { createRoutingSources } from "./routing-sources";
import { exitControl, routingSwitch } from "./routing-controls";
import type { Modal } from "./modal";

export function createRouting(
  modal: Modal,
  changed: () => Promise<void>,
  error: (e: unknown) => void,
) {
  let snapshot: Snapshot | null = null,
    value: Routing = { defaultExit: "proxy", policies: [] },
    revision = -1;
  let task: Promise<void> | null = null,
    removed: RemovedRule | null = null,
    message = "";
  const list = element("policy-list"),
    sources = createRoutingSources(() => writable() && !modal.open, error);
  const writable = () =>
    !!snapshot &&
    snapshot.settings.profile.mode === "generated" &&
    !snapshot.storageError &&
    !snapshot.closing &&
    !task;
  const rows = new Map<string, ReturnType<typeof createRow>>();
  function write(change: RoutingChange): Promise<void> {
    if (!writable())
      return Promise.reject(Error("当前不能修改分流，请等待操作完成"));
    message = "正在保存…";
    task = (async () => {
      const saved = await core.changeRouting(change, revision);
      value = saved.routing;
      revision = saved.routingRevision;
      if (change.kind === "remove") removed = saved.removed;
      if (change.kind === "restore") removed = null;
      message = snapshot?.runtime.pid
        ? "已保存，应用并重连后生效"
        : "已保存，下次连接生效";
      await changed();
    })().finally(() => {
      task = null;
      draw();
    });
    draw();
    return task;
  }
  function action(change: RoutingChange) {
    void write(change).catch((e) => {
      message = errorText(e);
      draw();
      void changed().catch(error);
    });
  }
  function edit(id?: string) {
    if (!writable()) return;
    const policy = value.policies.find((p) => p.id === id),
      expected = revision;
    editRoutingPolicy(
      modal,
      policy,
      () => writable() && revision === expected,
      (next) => write({ kind: policy ? "edit" : "add", policy: next }),
      policy
        ? async () => {
            await write({ kind: "remove", id: policy.id });
          }
        : undefined,
    );
  }
  function createRow(p: Policy) {
    const row = el("article", "routing-rule");
    row.dataset.policy = p.id;
    const info = el("div", "row-info"),
      title = el("div", "row-title");
    title.append(el("strong", "", p.name));
    if (p.bypassTun) title.append(el("span", "tag", "绕过 TUN"));
    const summary =
      p.match.kind === "source"
        ? `站点规则 · ${sources.name(p.match.sourceId) ?? p.name}`
        : `${p.match.domains.length} 个域名 · ${p.match.ipCidrs.length} 个 IP / 网段${p.match.domains[0] ? ` · ${p.match.domains[0].value}` : ""}`;
    const description = el("p", "row-meta", summary);
    info.append(title, description);
    const exit = exitControl(
      p.exit,
      `exit-${p.id}`,
      `${p.name} 的出口`,
      (exit) => action({ kind: "exit", id: p.id, exit }),
    );
    const enabled = routingSwitch("启用", `启用 ${p.name}`, (enabled) =>
      action({ kind: "enabled", id: p.id, enabled }),
    );
    enabled.input.checked = p.enabled;
    const actions = el("div", "routing-row-actions");
    const editButton = button(
      p.match.kind === "custom" ? "编辑" : "详情",
      () => {
        if (p.match.kind === "custom") edit(p.id);
        else {
          detail!.hidden = !detail!.hidden;
          editButton.setAttribute("aria-expanded", String(!detail!.hidden));
        }
      },
      "quiet",
    );
    const up = iconButton("up", `上移 ${p.name}`, () =>
      action({ kind: "move", id: p.id, direction: -1 }),
    );
    const down = iconButton("down", `下移 ${p.name}`, () =>
      action({ kind: "move", id: p.id, direction: 1 }),
    );
    const order = el("span", "routing-index");
    actions.append(editButton, up, down);
    row.append(order, info, exit, enabled.wrapper, actions);
    const detail =
      p.match.kind === "source" ? sources.attach(p.match.sourceId) : null;
    if (detail) {
      detail.append(
        button(
          "移除此规则",
          () => action({ kind: "remove", id: p.id }),
          "danger quiet",
        ),
      );
      row.append(detail);
      editButton.setAttribute("aria-expanded", "false");
    }
    return {
      row,
      title,
      description,
      exit,
      enabled,
      editButton,
      up,
      down,
      order,
      sourceId: p.match.kind === "source" ? p.match.sourceId : null,
    };
  }
  function draw() {
    const locked = !writable();
    for (const [id, entry] of rows)
      if (!value.policies.some((p) => p.id === id)) {
        if (entry.sourceId) sources.detach(entry.sourceId);
        entry.row.remove();
        rows.delete(id);
      }
    value.policies.forEach((p, index) => {
      if (!rows.has(p.id)) rows.set(p.id, createRow(p));
      const entry = rows.get(p.id)!;
      entry.order.textContent = String(index + 1);
      entry.title.replaceChildren(el("strong", "", p.name));
      if (p.bypassTun) entry.title.append(el("span", "tag", "绕过 TUN"));
      entry.description.textContent =
        p.match.kind === "source"
          ? `站点规则 · ${sources.name(p.match.sourceId) ?? p.name}`
          : `${p.match.domains.length} 个域名 · ${p.match.ipCidrs.length} 个 IP / 网段${p.match.domains[0] ? ` · ${p.match.domains[0].value}` : ""}`;
      entry.enabled.input.checked = p.enabled;
      entry.enabled.input.setAttribute("aria-label", `启用 ${p.name}`);
      entry.up.setAttribute("aria-label", `上移 ${p.name}`);
      entry.down.setAttribute("aria-label", `下移 ${p.name}`);
      entry.editButton.setAttribute(
        "aria-label",
        `${p.match.kind === "custom" ? "编辑" : "详情"} ${p.name}`,
      );
      for (const input of entry.exit.querySelectorAll<HTMLInputElement>(
        "input",
      )) {
        input.checked = input.value === p.exit;
        input.setAttribute(
          "aria-label",
          `${p.name} 的出口：${input.value === "proxy" ? "代理" : "直连"}`,
        );
      }
      for (const b of entry.row.querySelectorAll<
        HTMLButtonElement | HTMLInputElement | HTMLSelectElement
      >("button,input,select"))
        b.disabled = locked;
      if (p.match.kind === "source") entry.editButton.disabled = false;
      entry.up.disabled = locked || index === 0;
      entry.down.disabled = locked || index === value.policies.length - 1;
      entry.row.classList.toggle("routing-disabled", !p.enabled);
      if (list.children[index] !== entry.row)
        list.insertBefore(entry.row, list.children[index] ?? null);
    });
    sources.update();
    element("routing-empty").hidden = value.policies.length > 0;
    element("routing-count").textContent = `${value.policies.length} 条`;
    element("routing-state").textContent =
      snapshot?.settings.profile.mode === "externalConfig"
        ? "原始配置模式 · 分流只读"
        : message;
    for (const id of ["add-custom", "add-source"])
      element<HTMLButtonElement>(id).disabled =
        locked || value.policies.length >= 100;
    const select = element<HTMLSelectElement>("routing-default");
    select.value = value.defaultExit;
    select.disabled = locked;
    element("undo-routing-remove").hidden = !removed;
    element<HTMLButtonElement>("undo-routing-remove").disabled = locked;
  }
  element("add-custom").onclick = () => edit();
  element("add-source").onclick = () => {
    if (!writable()) return;
    const expected = revision;
    sources.add(
      modal,
      value.policies,
      () => writable() && expected === revision,
      async (policies) => {
        await write({
          kind: "addSources",
          ids: policies.flatMap((p) =>
            p.match.kind === "source" ? [p.match.sourceId] : [],
          ),
        });
      },
    );
  };
  element("undo-routing-remove").onclick = () => {
    if (removed) action({ kind: "restore", removed });
  };
  element<HTMLSelectElement>("routing-default").onchange = (e) =>
    action({
      kind: "default",
      exit: (e.target as HTMLSelectElement).value as "proxy" | "direct",
    });
  return {
    render(next: Snapshot) {
      snapshot = next;
      if (!task && next.settings.routingRevision !== revision) {
        revision = next.settings.routingRevision;
        value = structuredClone(next.settings.profile.routing);
        draw();
      } else if (!modal.open) draw();
    },
    refreshSources: sources.refresh,
    wait: async () => {
      if (task) await task;
    },
  };
}
