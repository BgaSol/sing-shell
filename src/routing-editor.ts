import { core, type Policy, type DomainMatch } from "./core";
import { el, button, iconButton } from "./dom";
import { bypassDomains, setExit } from "./routing-model";
import { exitControl, routingSwitch } from "./routing-controls";
import type { Modal } from "./modal";

export function editRoutingPolicy(modal: Modal, policy: Policy | undefined, valid: () => boolean, commit: (policy: Policy) => void) {
  const initial: Policy = policy ? structuredClone(policy) : {
    id: crypto.randomUUID(), name: "", exit: "proxy", enabled: true, bypassTun: false,
    match: { kind: "custom", domains: [], ipCidrs: [] },
  };
  if (initial.match.kind !== "custom") return;
  const name = el("input"), ip = el("textarea"), fields = el("div", "routing-domains");
  name.value = initial.name;
  name.maxLength = 64;
  ip.value = initial.match.ipCidrs.join("\n");
  ip.rows = 3;
  ip.spellcheck = false;
  ip.placeholder = "每行一个 IPv4、IPv6 或 CIDR";
  const domains: { input: HTMLInputElement; kind: HTMLSelectElement; row: HTMLElement; issue: HTMLElement }[] = [];
  function value(): Policy {
    return { ...initial, name: name.value.trim(), match: {
      kind: "custom",
      domains: domains.filter(d => d.input.value.trim()).map(d => ({ value: d.input.value.trim(), includeSubdomains: d.kind.value === "suffix" })),
      ipCidrs: ip.value.split(/\n/).map(s => s.trim()).filter(Boolean),
    } };
  }
  let original = "";
  modal.show(policy ? "编辑规则" : "添加规则", {
    dirty: () => JSON.stringify(value()) !== original,
    cleanup: () => { modal.dialog.classList.remove("routing-modal"); },
  });
  modal.dialog.classList.add("routing-modal");
  const token = modal.token;
  const nameLabel = el("label", "", "规则名称"); nameLabel.append(name);
  modal.content.append(nameLabel);
  const bypass = routingSwitch("绕过 TUN", "绕过 TUN", checked => { initial.bypassTun = checked; update(); });
  bypass.input.checked = !!initial.bypassTun;
  const help = el("p", "hint", "连接时解析域名，重连后刷新；共享 IP 的其他服务也可能一起绕过。");
  if (!policy) {
    const exitLabel = el("div", "routing-field");
    exitLabel.append(el("strong", "", "连接方式"), exitControl(initial.exit, "new-policy-exit", "连接方式", exit => {
      setExit(initial, exit); bypass.input.checked = !!initial.bypassTun; update();
    }));
    modal.content.append(exitLabel, bypass.wrapper, help);
  } else modal.content.append(el("p", "routing-current", `${initial.exit === "proxy" ? "代理" : "直连"}${initial.bypassTun ? " · 绕过 TUN" : ""} · 可在列表中调整`));
  const complete = button("完成", () => void finish(), "primary");
  function update() {
    bypass.wrapper.hidden = initial.exit !== "direct";
    help.hidden = initial.exit !== "direct";
    domains.forEach((d, index) => {
      const conflict = !!initial.bypassTun && !!d.input.value.trim() && d.kind.value === "suffix";
      d.input.setAttribute("aria-label", `域名 ${index + 1}`);
      d.kind.setAttribute("aria-label", `域名 ${index + 1} 的匹配方式`);
      d.kind.setAttribute("aria-invalid", String(conflict));
      d.issue.hidden = !conflict;
      d.issue.textContent = conflict ? "绕过 TUN 需要精确域名，请修改匹配方式。" : "";
    });
    complete.disabled = bypassDomains(value()).length > 0;
  }
  function addDomain(domain: DomainMatch = { value: "", includeSubdomains: false }) {
    if (domains.length >= 1000) { modal.error("匹配项总数最多 1,000 个"); return; }
    const row = el("div", "routing-domain"), input = el("input"), kind = el("select"), issue = el("p", "routing-field-error");
    input.value = domain.value; input.placeholder = "example.com"; input.spellcheck = false;
    for (const [value, text] of [["exact", "精确域名"], ["suffix", "包含子域名"]]) {
      const option = el("option", "", text); option.value = value; kind.append(option);
    }
    kind.value = domain.includeSubdomains ? "suffix" : "exact";
    issue.id = `routing-domain-${crypto.randomUUID()}`;
    kind.setAttribute("aria-describedby", issue.id); issue.setAttribute("role", "alert");
    const entry = { row, input, kind, issue };
    input.oninput = update; kind.onchange = update;
    const remove = iconButton("remove", "移除此域名", () => {
      const index = domains.indexOf(entry); domains.splice(index, 1); row.remove(); update();
      (domains[index]?.input ?? domains.at(-1)?.input ?? ip).focus();
    });
    row.append(input, kind, remove, issue); domains.push(entry); fields.append(row); update();
    input.onkeydown = event => {
      if (event.key === "Enter" && !event.isComposing && !event.ctrlKey && !event.metaKey) {
        event.preventDefault(); addDomain(); domains.at(-1)?.input.focus();
      }
    };
  }
  const subhead = el("div", "routing-subhead");
  subhead.append(el("h3", "", "域名"), button("添加域名", () => { addDomain(); domains.at(-1)?.input.focus(); }, "routing-text"));
  modal.content.append(subhead, fields);
  initial.match.domains.forEach(addDomain);
  if (!domains.length) addDomain();
  const ipLabel = el("label", "", "IP 与网段"); ipLabel.append(ip);
  modal.content.append(ipLabel, el("p", "hint", "域名与 IP 中任一项匹配，就使用本规则。"));
  async function finish() {
    if (!valid() || !modal.valid(token)) { modal.error("页面状态已改变，请返回列表重新编辑"); return; }
    const next = value();
    if (!next.name) { modal.error("请填写规则名称"); name.focus(); return; }
    if (next.match.kind !== "custom") return;
    if (!next.match.domains.length && !next.match.ipCidrs.length) { modal.error("至少填写一个域名或 IP"); domains[0]?.input.focus(); return; }
    if (bypassDomains(next).length) { update(); return; }
    const ok = await modal.run(async () => {
      // Validate this candidate only: another row may intentionally have an unfinished bypass choice.
      const result = await core.validateRouting({ defaultExit: "proxy", policies: [next] });
      if (!valid() || !modal.valid(token)) throw Error("页面状态已改变，请返回列表重新编辑");
      commit(result.policies[0]);
    });
    if (ok && modal.valid(token)) modal.close();
    else update();
  }
  modal.actions.append(button("取消", () => void modal.mayLeave()), complete);
  modal.content.onkeydown = event => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && !event.isComposing) { event.preventDefault(); void finish(); }
  };
  update(); original = JSON.stringify(value()); name.focus();
}
