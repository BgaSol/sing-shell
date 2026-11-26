import {
  core,
  type Snapshot,
  type Routing,
  type Policy,
  type RuleSource,
  type RouteAction,
  type DomainMatch,
} from "./core";
import { element, el, button, iconButton, empty } from "./dom";
import { reorderVisible } from "./routing-model";
import type { Modal } from "./modal";
export function createRouting(
  modal: Modal,
  changed: () => Promise<void>,
  error: (e: unknown) => void,
) {
  let snapshot: Snapshot | null = null,
    draft: Routing | null = null,
    baseline = "",
    revision = 0,
    filter = "all",
    saving = false,
    dragged: string | null = null,
    sources: RuleSource[] = [],
    signature = "";
  const expanded = new Set<string>();
  const cells = new Map<
    string,
    {
      version: HTMLElement;
      status: HTMLElement;
      error: HTMLElement;
      update: HTMLButtonElement;
      cancel: HTMLButtonElement;
      auto: HTMLInputElement;
      period: HTMLSelectElement;
    }
  >();
  const dirty = () => !!draft && JSON.stringify(draft) !== baseline;
  function mark() {
    element("routing-state").textContent = dirty()
      ? "未保存"
      : snapshot?.core.pendingChanges
        ? "待应用"
        : "已保存";
    element<HTMLButtonElement>("save-routing").disabled =
      !dirty() ||
      saving ||
      snapshot?.settings.profile.mode !== "generated" ||
      !!snapshot?.storageError;
    element("reload-routing").hidden =
      !snapshot || snapshot.settings.routingRevision === revision;
  }
  async function save() {
    if (!draft || saving) return false;
    saving = true;
    mark();
    try {
      const saved = await core.profileRouting(structuredClone(draft), revision);
      draft = structuredClone(saved.routing);
      revision = saved.routingRevision;
      baseline = JSON.stringify(draft);
      draw(true);
      await changed();
      return true;
    } catch (e) {
      error(e);
      return false;
    } finally {
      saving = false;
      mark();
    }
  }
  function load() {
    if (!snapshot) return;
    draft = structuredClone(snapshot.settings.profile.routing);
    baseline = JSON.stringify(draft);
    revision = snapshot.settings.routingRevision;
    draw(true);
  }
  function exitSelect(value: RouteAction, label: string) {
    const select = el("select");
    select.setAttribute("aria-label", label);
    for (const [id, text] of [
      ["proxy", "代理"],
      ["direct", "直连"],
    ] as const) {
      const option = el("option", "", text);
      option.value = id;
      select.append(option);
    }
    select.value = value;
    return select;
  }
  function edit(policy?: Policy) {
    if (!draft) return;
    const initial = policy
      ? structuredClone(policy)
      : {
          id: crypto.randomUUID(),
          name: "",
          enabled: true,
          exit: "proxy" as const,
          match: { kind: "custom" as const, domains: [], ipCidrs: [] },
        };
    if (initial.match.kind !== "custom") return;
    const name = el("input");
    name.value = initial.name;
    name.maxLength = 64;
    const exit = exitSelect(initial.exit, "规则出口");
    const domains: {
      input: HTMLInputElement;
      kind: HTMLSelectElement;
      row: HTMLElement;
    }[] = [];
    const domainFields = el("div", "domain-fields");
    const ip = el("textarea");
    ip.rows = 3;
    ip.value = initial.match.ipCidrs.join("\n");
    ip.spellcheck = false;
    ip.placeholder = "192.0.2.0/24\n2001:db8::/32";
    ip.setAttribute("aria-label", "IP 和网段，每行一个");
    const value = () => ({
      name: name.value,
      exit: exit.value,
      domains: domains
        .filter((d) => d.input.value.trim())
        .map((d) => ({
          value: d.input.value.trim(),
          includeSubdomains: d.kind.value === "suffix",
        })),
      ipCidrs: ip.value
        .split(/\n/)
        .map((s) => s.trim())
        .filter(Boolean),
    });
    let original = "";
    modal.show(policy ? "编辑规则" : "新建规则", {
      dirty: () => JSON.stringify(value()) !== original,
    });
    const pair = el("div", "field-pair");
    const nameLabel = el("label", "", "名称");
    nameLabel.append(name);
    const exitLabel = el("label", "", "统一出口");
    exitLabel.append(exit);
    pair.append(nameLabel, exitLabel);
    modal.content.append(pair);
    function addDomain(
      domain: DomainMatch = { value: "", includeSubdomains: true },
    ) {
      if (domains.length >= 1000) {
        modal.error("匹配项总数最多 1,000 个");
        return;
      }
      const row = el("div", "domain-row"),
        input = el("input"),
        kind = el("select");
      input.value = domain.value;
      input.placeholder = "example.com";
      input.spellcheck = false;
      input.setAttribute("aria-label", `域名 ${domains.length + 1}`);
      for (const [id, text] of [
        ["suffix", "包含子域名"],
        ["exact", "精确匹配"],
      ]) {
        const option = el("option", "", text);
        option.value = id;
        kind.append(option);
      }
      kind.value = domain.includeSubdomains ? "suffix" : "exact";
      kind.setAttribute("aria-label", "域名匹配方式");
      const entry = { input, kind, row };
      const remove = iconButton("remove", "删除此域名", () => {
        const index = domains.indexOf(entry);
        domains.splice(index, 1);
        row.remove();
        relabelDomains();
        (domains[index]?.input ?? domains.at(-1)?.input ?? ip).focus();
      });
      row.append(input, kind, remove);
      domains.push(entry);
      domainFields.append(row);
      relabelDomains();
      input.onkeydown = (e) => {
        if (e.key === "Enter" && !e.isComposing && !e.ctrlKey && !e.metaKey) {
          e.preventDefault();
          addDomain();
          domains.at(-1)?.input.focus();
        }
      };
    }
    function relabelDomains() {
      domains.forEach((d, index) => {
        d.input.setAttribute("aria-label", `域名 ${index + 1}`);
        d.kind.setAttribute("aria-label", `域名 ${index + 1} 的匹配方式`);
      });
    }
    const subhead = el("div", "subhead");
    subhead.append(
      el("h3", "", "域名"),
      button("添加域名", () => {
        addDomain();
        domains.at(-1)?.input.focus();
      }),
    );
    modal.content.append(subhead, domainFields);
    for (const d of initial.match.domains) addDomain(d);
    if (!domains.length) addDomain();
    const ipLabel = el("label", "", "IP / 网段");
    ipLabel.append(ip);
    modal.content.append(
      ipLabel,
      el(
        "p",
        "hint",
        "域名和 IP 任意一项命中即使用本规则出口。组间按列表顺序匹配；IP 规则不会在解析前决定 DNS 出口。",
      ),
    );
    original = JSON.stringify(value());
    const complete = () => {
      const v = value();
      if (!v.name.trim()) {
        modal.error("请填写规则名称");
        name.focus();
        return;
      }
      if (!v.domains.length && !v.ipCidrs.length) {
        modal.error("至少填写一个域名或 IP");
        domains[0]?.input.focus();
        return;
      }
      const next: Policy = {
        ...initial,
        name: v.name.trim(),
        exit: v.exit as RouteAction,
        match: { kind: "custom", domains: v.domains, ipCidrs: v.ipCidrs },
      };
      const proposed = structuredClone(draft!);
      if (policy) {
        const index = proposed.policies.findIndex((p) => p.id === policy.id);
        if (index < 0) {
          modal.error("规则已不存在，请返回列表");
          return;
        }
        proposed.policies[index] = next;
      } else proposed.policies.push(next);
      void modal
        .run(async () => {
          draft = await core.validateRouting(proposed);
        })
        .then((ok) => {
          if (ok) {
            draw(true);
            modal.close();
          } else {
            const message = element("modal-error").textContent ?? "";
            const domain = message.match(/域名 (\d+)/);
            if (domain)
              domains
                .filter((d) => d.input.value.trim())
                [Number(domain[1]) - 1]?.input.focus();
            else if (message.includes("IP ")) ip.focus();
          }
        });
    };
    modal.actions.append(
      button("取消", () => void modal.mayLeave()),
      button("完成", complete, "primary"),
    );
    const keydown = (e: KeyboardEvent) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !e.isComposing) {
        e.preventDefault();
        complete();
      }
    };
    modal.content.onkeydown = keydown;
    name.focus();
  }
  function addSource() {
    if (!draft) return;
    const existing = new Set(
      draft.policies.flatMap((p) =>
        p.match.kind === "source" ? [p.match.sourceId] : [],
      ),
    );
    const available = sources.filter((s) => !existing.has(s.definition.id));
    modal.show("添加开源规则");
    const checked = new Set<string>();
    if (!available.length)
      modal.content.append(
        el(
          "p",
          "hint",
          "固定来源已全部添加。可以在列表中调整出口、顺序和更新设置。",
        ),
      );
    for (const s of available) {
      const label = el("label", "source-choice"),
        box = el("input");
      box.type = "checkbox";
      box.onchange = () => {
        if (box.checked) checked.add(s.definition.id);
        else checked.delete(s.definition.id);
      };
      const text = el("div");
      text.append(
        el("strong", "", s.definition.name),
        el(
          "p",
          "",
          `${s.definition.repository} · ${s.definition.domains ? "域名" : "IP"}`,
        ),
      );
      label.append(box, text);
      modal.content.append(label);
    }
    const exit = exitSelect("proxy", "新增开源规则出口");
    const label = el("label", "", "新增规则的统一出口");
    label.append(exit);
    modal.content.append(
      label,
      el(
        "p",
        "hint",
        "添加后可分别修改。固定来源支持自动更新，保存规则后开始调度。",
      ),
    );
    modal.actions.append(
      button("取消", () => modal.close()),
      button(
        "添加规则",
        () => {
          if (!checked.size) {
            modal.error("请选择至少一个来源");
            return;
          }
          for (const id of checked) {
            const source = sources.find((s) => s.definition.id === id)!;
            draft!.policies.push({
              id: crypto.randomUUID(),
              name: source.definition.name,
              enabled: true,
              exit: exit.value as RouteAction,
              match: { kind: "source", sourceId: id },
            });
          }
          modal.close();
          draw(true);
        },
        "primary",
      ),
    );
  }
  function visible() {
    return (
      draft?.policies.filter(
        (p) => filter === "all" || p.match.kind === filter,
      ) ?? []
    );
  }
  function reorder(from: string, to: string) {
    if (!draft) return;
    draft.policies = reorderVisible(
      draft.policies,
      visible().map((p) => p.id),
      from,
      to,
    );
    draw(true);
    element("policy-list")
      .querySelector<HTMLButtonElement>(`[data-policy="${from}"] button`)
      ?.focus();
  }
  function draw(force = false) {
    if (!draft || !snapshot) return;
    const key = JSON.stringify([
      draft,
      filter,
      snapshot.settings.profile.mode,
      saving,
    ]);
    if (!force && key === signature) {
      mark();
      return;
    }
    signature = key;
    const list = element("policy-list");
    list.replaceChildren();
    cells.clear();
    const raw = snapshot.settings.profile.mode !== "generated";
    for (const b of ["add-source", "add-custom", "routing-default"])
      element<HTMLButtonElement>(b).disabled = raw;
    const rules = visible();
    for (const p of rules) {
      const block = el("div", "policy-block");
      block.dataset.policy = p.id;
      const row = el("div", "list-row");
      const drag = iconButton("grip", "拖拽调整规则顺序", () => {});
      drag.draggable = !raw;
      drag.ondragstart = (e) => {
        dragged = p.id;
        e.dataTransfer?.setData("text/plain", p.id);
        block.classList.add("dragging");
      };
      drag.ondragend = () => {
        dragged = null;
        block.classList.remove("dragging");
        for (const r of list.children) r.classList.remove("drag-over");
      };
      block.ondragover = (e) => {
        if (dragged && dragged !== p.id) {
          e.preventDefault();
          block.classList.add("drag-over");
        }
      };
      block.ondragleave = () => block.classList.remove("drag-over");
      block.ondrop = (e) => {
        e.preventDefault();
        if (dragged) reorder(dragged, p.id);
        dragged = null;
      };
      const info = el("div", "row-info");
      info.append(
        el("strong", "", p.name),
        el(
          "p",
          "",
          p.match.kind === "custom"
            ? `${p.match.domains.length} 个域名 · ${p.match.ipCidrs.length} 个 IP / 网段`
            : "固定开源规则",
        ),
      );
      const enabled = el("input");
      enabled.type = "checkbox";
      enabled.checked = p.enabled;
      enabled.disabled = raw;
      enabled.onchange = () => {
        p.enabled = enabled.checked;
        draw(true);
      };
      const enabledLabel = el("label", "policy-toggle");
      enabledLabel.append(
        enabled,
        document.createTextNode(p.enabled ? "启用" : "关闭"),
      );
      const exit = exitSelect(p.exit, `${p.name} 的出口`);
      exit.disabled = raw;
      exit.onchange = () => {
        p.exit = exit.value as RouteAction;
        draw(true);
      };
      const order = el("div", "order-actions");
      const index = rules.indexOf(p);
      for (const [type, offset, label] of [
        ["up", -1, "上移"],
        ["down", 1, "下移"],
      ] as const) {
        const move = iconButton(type, `${label} ${p.name}`, () =>
          reorder(p.id, rules[index + offset].id),
        );
        move.disabled = raw || !rules[index + offset];
        order.append(move);
      }
      row.append(
        drag,
        el("span", "policy-index", String(draft.policies.indexOf(p) + 1)),
        info,
        enabledLabel,
        exit,
        order,
      );
      if (p.match.kind === "custom")
        row.append(iconButton("edit", `编辑 ${p.name}`, () => edit(p)));
      row.append(
        iconButton("remove", `移除 ${p.name}`, () => {
          draft!.policies = draft!.policies.filter((item) => item.id !== p.id);
          draw(true);
        }),
      );
      block.append(row);
      if (p.match.kind === "source") {
        const id = p.match.sourceId;
        const detail = el("details", "policy-detail");
        detail.open = expanded.has(p.id);
        detail.ontoggle = () => {
          if (detail.open) expanded.add(p.id);
          else expanded.delete(p.id);
        };
        detail.append(el("summary", "", "来源与更新"));
        const source = sources.find((s) => s.definition.id === id);
        const body = el("div", "source-body");
        if (source) {
          body.append(
            el(
              "code",
              "",
              `https://raw.githubusercontent.com/${source.definition.repository}/${source.definition.branch}/${source.definition.file}`,
            ),
          );
        }
        const version = el("p"),
          status = el("p"),
          issue = el("p");
        issue.setAttribute("role", "alert");
        const auto = el("input");
        auto.type = "checkbox";
        auto.setAttribute("aria-label", `${p.name} 自动更新`);
        const period = el("select");
        period.setAttribute("aria-label", `${p.name} 更新周期`);
        for (const [hours, text] of [
          [6, "每 6 小时"],
          [24, "每天"],
          [168, "每周"],
        ] as const) {
          const opt = el("option", "", text);
          opt.value = String(hours);
          period.append(opt);
        }
        const preference = () => {
          void core
            .sourcePreference({
              sourceId: id,
              automatic: auto.checked,
              intervalHours: Number(period.value) as 6 | 24 | 168,
            })
            .then(refreshSources)
            .catch(error);
        };
        auto.onchange = preference;
        period.onchange = preference;
        const autoLabel = el("label");
        autoLabel.append(auto, document.createTextNode("自动更新"), period);
        const update = button("立即更新", () => {
          void core.updateSource(id).then(refreshSources).catch(error);
        });
        const cancel = button("取消更新", () => {
          void core.cancelRules().then(refreshSources).catch(error);
        });
        body.append(version, status, issue, autoLabel);
        const actions = el("div", "actions");
        actions.append(update, cancel);
        body.append(actions);
        detail.append(body);
        block.append(detail);
        cells.set(id, {
          version,
          status,
          error: issue,
          update,
          cancel,
          auto,
          period,
        });
      }
      if (raw) {
        for (const b of block.querySelectorAll<HTMLButtonElement>("button"))
          b.disabled = true;
      }
      list.append(block);
    }
    if (!rules.length && filter !== "default")
      list.append(
        empty("没有此类规则", "新建自定义规则，或从固定目录添加开源规则。"),
      );
    list.hidden = filter === "default";
    element("default-policy").hidden = !["all", "default"].includes(filter);
    element<HTMLSelectElement>("routing-default").value = draft.defaultExit;
    updateSources();
    mark();
  }
  function updateSources() {
    for (const [id, c] of cells) {
      const s = sources.find((s) => s.definition.id === id);
      if (!s) continue;
      const available = !!s.preference;
      c.version.textContent = `已准备版本：${s.version?.commit.slice(0, 12) ?? "暂无"} · ${s.definition.license}`;
      c.status.textContent = !available
        ? "保存规则后可更新"
        : s.status.phase === "preparing"
          ? "正在下载并校验，当前连接保持原版本"
          : s.status.lastSuccess
            ? `最近更新 ${new Date(s.status.lastSuccess).toLocaleString()} · 重连后采用`
            : "随包版本可离线使用";
      c.error.textContent = s.status.error ?? "";
      c.error.hidden = !s.status.error;
      c.update.disabled =
        !available || sources.some((s) => s.status.phase === "preparing");
      c.cancel.hidden = s.status.phase !== "preparing";
      c.auto.disabled = !available;
      c.period.disabled = !available;
      if (document.activeElement !== c.period)
        c.period.value = String(s.preference?.intervalHours ?? 24);
      c.auto.checked = s.preference?.automatic ?? true;
    }
  }
  async function refreshSources() {
    try {
      sources = await core.sources();
      if (cells.size) updateSources();
    } catch (e) {
      error(e);
    }
  }
  element("add-custom").onclick = () => edit();
  element("add-source").onclick = addSource;
  element<HTMLSelectElement>("routing-default").onchange = (e) => {
    if (draft) {
      draft.defaultExit = (e.target as HTMLSelectElement).value as RouteAction;
      mark();
    }
  };
  element("save-routing").onclick = () => void save();
  element("reload-routing").onclick = () => {
    void modal
      .decision(
        "载入最新规则",
        "本地草稿尚未合并。保存会先校对修订；放弃将载入后端最新规则。",
      )
      .then(async (choice) => {
        if (choice === "save") await save();
        else if (choice === "discard") load();
      });
  };
  for (const b of element("routing-filter").querySelectorAll<HTMLButtonElement>(
    "button",
  ))
    b.onclick = () => {
      filter = b.dataset.filter!;
      for (const item of element("routing-filter").querySelectorAll("button"))
        item.setAttribute("aria-pressed", String(item === b));
      draw(true);
    };
  return {
    render(s: Snapshot) {
      snapshot = s;
      if (!draft || (!dirty() && revision !== s.settings.routingRevision))
        load();
      else draw();
    },
    refreshSources,
    dirty,
    async mayLeave() {
      if (!dirty()) return true;
      const choice = await modal.decision(
        "分流规则尚未保存",
        "保存整张规则表，或放弃本次草稿后继续退出。",
      );
      if (choice === "save") return save();
      if (choice === "discard") {
        load();
        return true;
      }
      return false;
    },
  };
}
