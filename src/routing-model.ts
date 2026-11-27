import type { Policy } from "./core";
export function setExit(
  policy: Pick<Policy, "exit" | "bypassTun">,
  exit: Policy["exit"],
) {
  const cleared = exit === "proxy" && !!policy.bypassTun;
  policy.exit = exit;
  if (exit === "proxy") policy.bypassTun = false;
  return cleared;
}
// Visible rules keep their global slots; hidden rules never move during filtered sorting.
export function reorderVisible(
  policies: Policy[],
  visible: string[],
  from: string,
  to: string,
) {
  const ordered = visible.filter((id) => policies.some((p) => p.id === id));
  const a = ordered.indexOf(from),
    b = ordered.indexOf(to);
  if (a < 0 || b < 0 || a === b) return policies;
  ordered.splice(a, 1);
  ordered.splice(b, 0, from);
  const byId = new Map(policies.map((p) => [p.id, p]));
  let index = 0;
  const set = new Set(ordered);
  return policies.map((p) => (set.has(p.id) ? byId.get(ordered[index++])! : p));
}
