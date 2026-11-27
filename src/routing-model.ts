import type { Policy, Routing } from "./core";
export function setExit(
  policy: Pick<Policy, "exit" | "bypassTun">,
  exit: Policy["exit"],
) {
  const cleared = exit === "proxy" && !!policy.bypassTun;
  policy.exit = exit;
  if (exit === "proxy") policy.bypassTun = false;
  return cleared;
}
export function canBypass(policy: Policy) {
  return policy.match.kind === "custom" && policy.exit === "direct";
}

export function bypassDomains(policy: Policy) {
  return policy.bypassTun && policy.match.kind === "custom"
    ? policy.match.domains.flatMap((domain, index) => domain.includeSubdomains ? [index] : [])
    : [];
}

function loaded(routing: Routing): Routing {
  const next = structuredClone(routing);
  for (const policy of next.policies) policy.bypassTun ??= false;
  return next;
}

// Page intent only; normalization and final validation remain native operations.
export class RoutingDraft {
  value: Routing = { defaultExit: "proxy", policies: [] };
  revision = -1;
  generation = 0;
  saving = false;
  removed: { policy: Policy; index: number } | null = null;
  private baseline = "";

  get dirty() { return this.revision >= 0 && JSON.stringify(this.value) !== this.baseline; }
  get issues() { return this.value.policies.filter(p => bypassDomains(p).length); }
  load(routing: Routing, revision: number) {
    if (this.saving) return;
    this.adopt(routing, revision);
  }
  private adopt(routing: Routing, revision: number) {
    this.value = loaded(routing);
    this.revision = revision;
    this.baseline = JSON.stringify(this.value);
    this.removed = null;
    this.generation++;
  }
  change(id: string, update: (policy: Policy) => void) {
    if (this.saving) return false;
    const policy = this.value.policies.find(p => p.id === id);
    if (!policy) return false;
    update(policy);
    this.generation++;
    return true;
  }
  replace(policy: Policy, adding: boolean, generation: number) {
    if (this.saving || generation !== this.generation) throw Error("规则草稿已改变，请返回列表重新编辑");
    const index = this.value.policies.findIndex(p => p.id === policy.id);
    if (adding) {
      if (index >= 0 || this.value.policies.length >= 100) throw Error("最多保存 100 条规则，且规则不能重复");
      this.value.policies.push(structuredClone(policy));
    } else {
      if (index < 0) throw Error("规则已不存在，请返回列表");
      this.value.policies[index] = structuredClone(policy);
    }
    this.generation++;
  }
  addSources(policies: Policy[]) {
    if (this.saving || this.value.policies.length + policies.length > 100) throw Error("最多保存 100 条规则");
    const ids = new Set(this.value.policies.flatMap(p => p.match.kind === "source" ? [p.match.sourceId] : []));
    for (const policy of policies) {
      if (policy.match.kind !== "source" || ids.has(policy.match.sourceId)) throw Error("同一来源只能添加一次");
      ids.add(policy.match.sourceId);
    }
    this.value.policies.push(...structuredClone(policies));
    this.generation++;
  }
  setDefault(exit: Routing["defaultExit"]) {
    if (this.saving) return;
    this.value.defaultExit = exit;
    this.generation++;
  }
  move(id: string, offset: -1 | 1) {
    if (this.saving) return;
    const index = this.value.policies.findIndex(p => p.id === id), target = index + offset;
    if (index < 0 || target < 0 || target >= this.value.policies.length) return;
    const [policy] = this.value.policies.splice(index, 1);
    this.value.policies.splice(target, 0, policy);
    this.generation++;
  }
  remove(id: string) {
    if (this.saving) return;
    const index = this.value.policies.findIndex(p => p.id === id);
    if (index < 0) return;
    this.removed = { policy: structuredClone(this.value.policies[index]), index };
    this.value.policies.splice(index, 1);
    this.generation++;
  }
  undo() {
    if (this.saving || !this.removed) return;
    const { policy, index } = this.removed;
    if (this.value.policies.length >= 100 || this.value.policies.some(p => p.id === policy.id ||
      (p.match.kind === "source" && policy.match.kind === "source" && p.match.sourceId === policy.match.sourceId)))
      throw Error("无法撤销：规则已重新添加，或已达到 100 条上限");
    this.value.policies.splice(Math.min(index, this.value.policies.length), 0, policy);
    this.removed = null;
    this.generation++;
  }
  async save(write: (routing: Routing, revision: number) => Promise<{ routing: Routing; routingRevision: number }>) {
    if (this.saving || !this.dirty) return false;
    if (this.issues.length) throw Error("请先修改标出的域名，或关闭绕过 TUN");
    this.saving = true;
    try {
      const saved = await write(structuredClone(this.value), this.revision);
      this.adopt(saved.routing, saved.routingRevision);
      return true;
    } finally { this.saving = false; }
  }
}
