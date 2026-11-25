import { invoke } from "@tauri-apps/api/core";

export interface Snapshot {
  settings: { corePath: string | null; configPath: string | null };
  status: "running" | "stopped" | "failed";
  pid: number | null;
  error: string | null;
  logs: { receivedAt: number; source: string; message: string }[];
}

export const core = {
  snapshot: () => invoke<Snapshot>("core_snapshot"),
  action: (action: "start" | "stop" | "check") => invoke<Snapshot>("core_action", { action }),
  choose: (kind: "core" | "config") => invoke<Snapshot | null>("choose_file", { kind }),
  ready: () => invoke<void>("frontend_ready"),
};
