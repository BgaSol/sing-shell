import { core, type Policy, type RuleSource } from "./api";
import { el, button } from "./dom";
import type { Modal } from "./modal";
import { routingSwitch } from "./routing-controls";

export function createRoutingSources(
  editable: () => boolean,
  error: (reason: unknown) => void,
) {
  let sources: RuleSource[] = [],
    request: Promise<void> | null = null;
  const pending = new Set<string>();
  const cells = new Map<string, ReturnType<typeof createDetail>>();
  function createDetail(id: string) {
    let localError = "";
    const body = el("div", "routing-source"),
      origin = el("p", "hint"),
      version = el("p", "hint"),
      status = el("p", "hint"),
      issue = el("p", "routing-field-error"),
      period = el("select");
    const autoControl = routingSwitch("自动更新", `${id} 自动更新`, () =>
        preference(),
      ),
      auto = autoControl.input;
    body.id = `routing-source-${id}`;
    body.hidden = true;
    issue.setAttribute("role", "alert");
    auto.type = "checkbox";
    auto.setAttribute("aria-label", `${id} 自动更新`);
    period.setAttribute("aria-label", `${id} 更新周期`);
    for (const [hours, text] of [
      [6, "每 6 小时"],
      [24, "每天"],
      [168, "每周"],
    ] as const) {
      const option = el("option", "", text);
      option.value = String(hours);
      period.append(option);
    }
    async function run(task: () => Promise<unknown>) {
      if (
        !editable() ||
        pending.has(id) ||
        !sources.find((s) => s.definition.id === id)?.preference
      )
        return;
      pending.add(id);
      localError = "";
      update();
      try {
        await task();
        if (request) await request;
        await refresh();
      } catch (reason) {
        localError = String(reason);
      } finally {
        pending.delete(id);
        update();
      }
    }
    const preference = () => {
      const next = {
        sourceId: id,
        automatic: auto.checked,
        intervalHours: Number(period.value) as 6 | 24 | 168,
      };
      void run(() => core.sourcePreference(next));
    };
    auto.onchange = preference;
    period.onchange = preference;
    const label = el("div", "routing-source-preference");
    label.append(autoControl.wrapper, period);
    const updateButton = button(
      "立即更新",
      () => void run(() => core.updateSource(id)),
    );
    const cancel = button("取消更新", () => void run(() => core.cancelRules()));
    const actions = el("div", "actions");
    actions.append(updateButton, cancel);
    body.append(
      origin,
      version,
      status,
      issue,
      label,
      el("p", "hint", "更新设置自动保存，不改变当前连接；新版本在重连后采用。"),
      actions,
    );
    return {
      body,
      origin,
      version,
      status,
      issue,
      auto,
      period,
      updateButton,
      cancel,
      get error() {
        return localError;
      },
    };
  }
  function update() {
    for (const [id, c] of cells) {
      const s = sources.find((s) => s.definition.id === id);
      const available = !!s?.preference,
        locked = !editable() || !available || pending.has(id);
      c.origin.textContent = s
        ? `${s.definition.repository} · ${s.definition.license}`
        : "正在读取来源信息";
      c.version.textContent = `已准备版本：${s?.version?.commit.slice(0, 12) ?? "随包版本"}`;
      c.status.textContent = !available
        ? "保存规则后可设置更新"
        : s?.status.phase === "preparing"
          ? "正在下载并校验，当前连接保持原版本"
          : s?.status.lastSuccess
            ? `最近更新 ${new Date(s.status.lastSuccess).toLocaleString()}`
            : "随包规则可离线使用";
      c.issue.textContent = c.error || s?.status.error || "";
      c.issue.hidden = !c.issue.textContent;
      c.auto.disabled = locked;
      c.auto.setAttribute("aria-label", `${s?.definition.name ?? id} 自动更新`);
      c.period.setAttribute(
        "aria-label",
        `${s?.definition.name ?? id} 更新周期`,
      );
      c.period.disabled = locked;
      c.updateButton.disabled =
        locked || sources.some((s) => s.status.phase === "preparing");
      c.cancel.hidden = s?.status.phase !== "preparing";
      c.cancel.disabled = locked;
      if (!pending.has(id)) {
        c.auto.checked = s?.preference?.automatic ?? true;
        if (document.activeElement !== c.period)
          c.period.value = String(s?.preference?.intervalHours ?? 24);
      }
    }
  }
  async function refresh() {
    if (request) return request;
    request = core
      .sources()
      .then((next) => {
        sources = next;
        update();
      })
      .catch(error)
      .finally(() => {
        request = null;
      });
    return request;
  }
  function attach(sourceId: string) {
    const cell = createDetail(sourceId);
    cells.set(sourceId, cell);
    update();
    return cell.body;
  }
  function add(
    modal: Modal,
    policies: Policy[],
    valid: () => boolean,
    commit: (policies: Policy[]) => Promise<void>,
  ) {
    const existing = new Set(
      policies.flatMap((p) =>
        p.match.kind === "source" ? [p.match.sourceId] : [],
      ),
    );
    const available = sources.filter((s) => !existing.has(s.definition.id));
    const chosen = new Set<string>();
    modal.show("添加站点规则", {
      dirty: () => chosen.size > 0,
      cleanup: () => modal.dialog.classList.remove("routing-modal"),
    });
    modal.dialog.classList.add("routing-modal");
    if (!available.length)
      modal.content.append(
        el(
          "p",
          "hint",
          sources.length
            ? "固定来源已全部添加，可在列表调整连接方式和更新设置。"
            : "正在读取固定来源，请返回后重试。",
        ),
      );
    for (const s of available) {
      const label = el("label", "source-choice"),
        check = el("input"),
        info = el("div");
      check.type = "checkbox";
      check.onchange = () => {
        if (check.checked) chosen.add(s.definition.id);
        else chosen.delete(s.definition.id);
      };
      info.append(
        el("strong", "", s.definition.name),
        el(
          "p",
          "hint",
          `${s.definition.repository} · ${s.definition.domains ? "域名" : "IP"}`,
        ),
      );
      label.append(check, info);
      modal.content.append(label);
    }
    modal.content.append(
      el("p", "hint", "国内来源默认直连，其他来源默认代理；添加后自动保存。"),
    );
    const save = async () => {
      if (!chosen.size) {
        modal.error("请选择至少一个来源");
        return false;
      }
      if (!valid()) {
        modal.error("页面状态已改变，请返回列表");
        return false;
      }
      const ok = await modal.run(async () => {
        await commit(
          available
            .filter((s) => chosen.has(s.definition.id))
            .map((s) => ({
              id: crypto.randomUUID(),
              name: s.definition.name,
              enabled: true,
              exit: "proxy",
              bypassTun: false,
              match: { kind: "source", sourceId: s.definition.id },
            })),
        );
      });
      if (ok) modal.close();
      return ok;
    };
    modal.onSubmit(save, "添加并继续");
    modal.actions.append(
      button("取消", () => void modal.mayLeave()),
      button("添加规则", () => void save(), "primary"),
    );
  }
  return {
    refresh,
    update,
    attach,
    add,
    detach: (id: string) => cells.delete(id),
    name: (id: string) =>
      sources.find((s) => s.definition.id === id)?.definition.name,
  };
}
