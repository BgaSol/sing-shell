import type { Policy } from "./contracts";
export function setExit(
  policy: Pick<Policy, "exit" | "bypassTun">,
  exit: Policy["exit"],
) {
  const cleared = exit === "proxy" && !!policy.bypassTun;
  policy.exit = exit;
  if (exit === "proxy") policy.bypassTun = false;
  return cleared;
}
export const canBypass = (policy: Policy) =>
  policy.match.kind === "custom" && policy.exit === "direct";
export const bypassDomains = (policy: Policy) =>
  policy.bypassTun && policy.match.kind === "custom"
    ? policy.match.domains.flatMap((d, i) => (d.includeSubdomains ? [i] : []))
    : [];
