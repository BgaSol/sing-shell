import "./routing.css";
import { core, type Snapshot, type Policy } from "./core";
import { element, el, button, icon } from "./dom";
import { RoutingDraft, bypassDomains, canBypass, setExit } from "./routing-model";
import { editRoutingPolicy } from "./routing-editor";
import { createRoutingSources } from "./routing-sources";
import { exitControl, routingSwitch } from "./routing-controls";
import { connectionReady } from "./session-model";
import type { Modal } from "./modal";

export function createRouting(modal: Modal, changed: () => Promise<void>, error: (e: unknown) => void, apply: () => Promise<void>) {
  const draft = new RoutingDraft(), rows = new Map<string, ReturnType<typeof createRow>>();
  let snapshot: Snapshot | null = null, message = "", savingTask: Promise<boolean> | null = null, awaitingSnapshot = false, editorMode = 0;
  const list = element("policy-list"), result = element("routing-result"), panel = element("routing-panel");
  const writable = () => !!snapshot && snapshot.settings.profile.mode === "generated" && !snapshot.storageError && !snapshot.closing && !draft.saving;
  const editable = () => writable() && !modal.open;
  const sources = createRoutingSources(editable, error);
  const shape = (p: Policy) => p.match.kind === "source" ? `source:${p.match.sourceId}` : "custom";
  function state() {
    const raw = snapshot?.settings.profile.mode !== "generated", locked = !writable();
    const conflict = !!snapshot && snapshot.settings.routingRevision !== draft.revision;
    element("routing-state").textContent = raw ? "原始配置模式 · 分流只读" : draft.saving ? "正在保存" : draft.issues.length ? "请先处理标出的域名问题" :
      draft.dirty ? conflict ? "未保存 · 远端规则已改变" : "未保存" : awaitingSnapshot ? "已保存，正在确认连接状态" : snapshot?.core.pendingChanges ? "已保存 · 待应用" : "已保存";
    element<HTMLButtonElement>("save-routing").disabled = locked || !draft.dirty || draft.issues.length > 0;
    element<HTMLButtonElement>("reload-routing").hidden = !conflict;
    element<HTMLButtonElement>("reload-routing").disabled = locked;
    for (const id of ["add-custom", "add-source"]) element<HTMLButtonElement>(id).disabled = locked || draft.value.policies.length >= 100;
    element<HTMLSelectElement>("routing-default").disabled = locked;
    element<HTMLButtonElement>("undo-routing-remove").hidden = !draft.removed;
    element<HTMLButtonElement>("undo-routing-remove").disabled = locked;
    result.textContent = message;
    result.hidden = !message;
    const running = !!snapshot && connectionReady(snapshot), applyButton = element<HTMLButtonElement>("apply-routing");
    applyButton.hidden = raw || !running || (!snapshot?.core.pendingChanges && !snapshot?.core.runningBypass);
    applyButton.disabled = locked || draft.dirty || !!snapshot?.runtimeBusy || awaitingSnapshot;
    element("routing-apply-hint").textContent = draft.dirty ? "保存后再应用，当前连接保持原设置。" : !running ? "保存后，下次连接采用。" : "仅应用已保存配置；重新连接会刷新绕过地址。";
    element("routing-count").textContent = `${draft.value.policies.length} 条规则`;
    element("routing-bypass-note").hidden = !draft.value.policies.some(p => p.bypassTun);
    for (const [id, row] of rows) row.update(draft.value.policies.find(p => p.id === id)!, locked);
    sources.update();
  }
  function mutate(change: () => void) {
    if (!editable()) return;
    message = "";
    try { change(); reconcile(); } catch (reason) { message = String(reason); state(); }
  }
  function edit(id?: string) {
    if (!editable()) return;
    const policy = draft.value.policies.find(p => p.id === id), generation = draft.generation, mode = editorMode;
    editRoutingPolicy(modal, policy, () => writable() && generation === draft.generation && mode === editorMode, next => {
      draft.replace(next, !policy, generation); message = ""; reconcile();
    });
    state();
  }
  function createRow(policy: Policy) {
    const block = el("article", "routing-rule"), index = el("span", "routing-index"), info = el("div", "routing-info"),
      name = el("h3"), summary = el("p", "routing-summary"), head = el("div", "routing-rule-head"), settings = el("div", "routing-rule-settings");
    block.dataset.policy = policy.id;
    const enabled = routingSwitch("已启用", `启用 ${policy.name}`, checked => mutate(() => { draft.change(policy.id, p => { p.enabled = checked; }); }));
    head.append(name, enabled.wrapper); info.append(head, summary);
    const connection = el("div", "routing-connection");
    const exit = exitControl(policy.exit, `exit-${policy.id}`, `${policy.name} 的连接方式`, choice => mutate(() => { draft.change(policy.id, p => setExit(p, choice)); }));
    const bypass = routingSwitch("绕过 TUN", `${policy.name} 绕过 TUN`, checked => mutate(() => { draft.change(policy.id, p => { if (canBypass(p)) p.bypassTun = checked; }); }));
    connection.append(exit, bypass.wrapper);
    const actions = el("div", "routing-row-actions"), editButton = button(policy.match.kind === "custom" ? "编辑" : "来源与更新", () => {
      if (policy.match.kind === "custom") edit(policy.id);
      else if (detail) { detail.hidden = !detail.hidden; editButton.setAttribute("aria-expanded", String(!detail.hidden)); }
    }, "routing-text");
    editButton.dataset.action = "edit";
    const order = el("div", "routing-order");
    const up = button("上移", () => mutate(() => draft.move(policy.id, -1)), "routing-text"), down = button("下移", () => mutate(() => draft.move(policy.id, 1)), "routing-text");
    up.prepend(icon("up")); down.prepend(icon("down"));
    up.dataset.action = "up"; down.dataset.action = "down"; order.append(up, down);
    const remove = button("移除", () => {
      const position = draft.value.policies.findIndex(p => p.id === policy.id);
      mutate(() => draft.remove(policy.id));
      if (!rows.has(policy.id)) (rows.get(draft.value.policies[Math.min(position, draft.value.policies.length - 1)]?.id)?.editButton ?? element("add-custom")).focus();
    }, "routing-text routing-remove");
    remove.dataset.action = "remove"; actions.append(editButton, order, remove); settings.append(connection, actions);
    const issue = el("div", "routing-row-error"), issueText = el("span"), fix = button("修改域名", () => edit(policy.id), "routing-text");
    issue.setAttribute("role", "alert"); issue.append(issueText, fix); info.append(settings, issue); block.append(index, info);
    const detail = policy.match.kind === "source" ? sources.attach(policy.match.sourceId) : null;
    if (detail) { block.append(detail); editButton.setAttribute("aria-controls", detail.id); editButton.setAttribute("aria-expanded", "false"); }
    return { block, editButton, shape: shape(policy), update(p: Policy, locked: boolean) {
      const position = draft.value.policies.findIndex(item => item.id === p.id);
      index.textContent = String(position + 1); name.textContent = p.name;
      summary.textContent = p.match.kind === "custom" ? `${p.match.domains.length} 个域名 · ${p.match.ipCidrs.length} 个 IP / 网段${p.match.domains[0] ? ` · ${p.match.domains[0].value}` : ""}` : `站点规则 · ${sources.name(p.match.sourceId) ?? p.name}`;
      enabled.input.checked = p.enabled; enabled.caption.textContent = p.enabled ? "已启用" : "已停用";
      enabled.input.setAttribute("aria-label", `启用 ${p.name}`);
      block.classList.toggle("routing-disabled", !p.enabled);
      for (const input of exit.querySelectorAll<HTMLInputElement>("input")) {
        input.checked = input.value === p.exit;
        input.setAttribute("aria-label", `${p.name} 的连接方式：${input.value === "proxy" ? "代理" : "直连"}`);
      }
      exit.setAttribute("aria-label", `${p.name} 的连接方式`);
      bypass.wrapper.hidden = !canBypass(p); bypass.input.checked = !!p.bypassTun;
      bypass.input.setAttribute("aria-label", `${p.name} 绕过 TUN`);
      const invalid = bypassDomains(p).length > 0;
      issue.hidden = !invalid; issueText.textContent = invalid ? "绕过 TUN 需要精确域名，请修改“包含子域名”的目标。" : "";
      for (const control of block.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button")) control.disabled = locked;
      up.disabled = locked || position === 0; down.disabled = locked || position === draft.value.policies.length - 1;
      for (const [b, action] of [[editButton, p.match.kind === "custom" ? "编辑" : "来源与更新"], [up, "上移"], [down, "下移"], [remove, "移除"], [fix, "修改域名"]] as const) b.setAttribute("aria-label", `${action} ${p.name}`);
    } };
  }
  function reconcile() {
    for (const [id, row] of rows) if (!draft.value.policies.some(p => p.id === id && shape(p) === row.shape)) {
      const sourceId = row.block.querySelector<HTMLElement>(".routing-source")?.id.replace("routing-source-", "");
      if (sourceId) sources.detach(sourceId);
      row.block.remove(); rows.delete(id);
    }
    // Move existing nodes only when their order changed; ordinary toggles keep native focus.
    draft.value.policies.forEach((p, index) => {
      if (!rows.has(p.id)) rows.set(p.id, createRow(p));
      const block = rows.get(p.id)!.block;
      if (list.children[index] !== block) list.insertBefore(block, list.children[index] ?? null);
    });
    element("routing-empty").hidden = draft.value.policies.length > 0;
    element<HTMLSelectElement>("routing-default").value = draft.value.defaultExit;
    state();
  }
  function load() {
    if (!snapshot || draft.saving) return;
    draft.load(snapshot.settings.profile.routing, snapshot.settings.routingRevision); message = ""; reconcile();
  }
  function save(): Promise<boolean> {
    if (savingTask) return savingTask;
    if (!writable() || modal.open || !draft.dirty || draft.issues.length) return Promise.resolve(false);
    const pending = draft.save(core.profileRouting); message = ""; state();
    savingTask = (async () => {
      try {
        const ok = await pending; awaitingSnapshot = ok; reconcile();
        try { await changed(); awaitingSnapshot = !snapshot || snapshot.settings.routingRevision < draft.revision; }
        catch (reason) { message = `已保存，读取连接状态失败：${reason}`; }
        return ok;
      } catch (reason) { message = String(reason); return false; }
      finally { savingTask = null; state(); }
    })();
    return savingTask;
  }
  element("add-custom").onclick = () => edit();
  element("add-source").onclick = () => {
    if (!editable()) return;
    const generation = draft.generation, mode = editorMode;
    sources.add(modal, draft.value.policies, () => writable() && draft.generation === generation && editorMode === mode, policies => { draft.addSources(policies); reconcile(); });
    state();
  };
  element("save-routing").onclick = () => void save();
  element("apply-routing").onclick = () => { if (editable() && !draft.dirty && !snapshot?.runtimeBusy) void apply(); };
  element("undo-routing-remove").onclick = () => mutate(() => draft.undo());
  element<HTMLSelectElement>("routing-default").onchange = event => mutate(() => draft.setDefault((event.target as HTMLSelectElement).value as "proxy" | "direct"));
  element("reload-routing").onclick = () => void (async () => {
    if (!editable()) return;
    if (draft.dirty) {
      const decision = await modal.decision("分流规则已改变", "保存会校对修订；放弃本次草稿后载入最新规则。");
      if (decision === "save") { await save(); return; }
      if (decision !== "discard") return;
    }
    load();
  })();
  panel.addEventListener("focusin", () => state());
  return {
    render(next: Snapshot) {
      if (snapshot?.settings.profile.mode !== next.settings.profile.mode || snapshot?.storageError !== next.storageError || snapshot?.closing !== next.closing) editorMode++;
      snapshot = next;
      if (awaitingSnapshot && next.settings.routingRevision >= draft.revision) awaitingSnapshot = false;
      if (draft.revision < 0 || (!awaitingSnapshot && !draft.dirty && !draft.saving && !savingTask && !modal.open && draft.revision !== next.settings.routingRevision)) load();
      else state();
    },
    refreshSources: sources.refresh,
    dirty: () => draft.dirty,
    async mayLeave() {
      if (savingTask) await savingTask;
      if (!draft.dirty) return true;
      const choice = await modal.decision("分流规则尚未保存", "保存整张规则表，或放弃本次草稿后继续退出。");
      if (choice === "save") return save();
      if (choice === "discard") { load(); return true; }
      return false;
    },
  };
}
