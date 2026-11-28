import { invoke } from "@tauri-apps/api/core";
import type {
  Settings,
  Snapshot,
  Component,
  Network,
  RoutingChange,
  RoutingResult,
  RuleSource,
  RulePreference,
  ActivityQuery,
  ActivityPage,
  Log,
  SessionEvent,
  UpdateSnapshot,
  ConnectionOptions,
  Mirror,
  ProfileSnapshot,
  Profile,
  NodeSummary,
} from "./contracts";
export type * from "./contracts";
export const core = {
  saveConnection: (expectedRevision: number, options: ConnectionOptions) =>
    invoke<Settings>("set_connection_options", { expectedRevision, options }),
  snapshot: () => invoke<Snapshot>("app_snapshot"),
  session: () =>
    invoke<
      Omit<Snapshot, "settings"> & {
        revisions: number[];
        startup: Settings["startup"];
      }
    >("session_snapshot"),
  connect: (nodeId: string) => invoke("session_connect", { nodeId }),
  selectNode: (nodeId: string, expectedRevision: number) =>
    invoke("session_select_node", { nodeId, expectedRevision }),
  sources: () => invoke<RuleSource[]>("rule_sources"),
  updateSource: (sourceId: string) => invoke("rule_update", { sourceId }),
  cancelRules: () => invoke("rule_cancel"),
  sourcePreference: (preference: RulePreference) =>
    invoke("rule_preference", { preference }),
  reset: () => invoke<string>("settings_reset"),
  activity: (query: ActivityQuery) =>
    invoke<ActivityPage>("activity_snapshot", { query }),
  logs: (after: number, limit = 100) =>
    invoke<Log[]>("read_logs", { after, limit }),
  events: (after: number, limit = 100) =>
    invoke<SessionEvent[]>("read_events", { after, limit }),
  action: (action: "start" | "stop" | "check" | "apply") =>
    invoke("core_action", { action }),
  choose: (kind: "core" | "config") => invoke("choose_file", { kind }),
  ready: (document: string) => invoke("frontend_ready", { document }),
  updates: () => invoke<UpdateSnapshot>("update_snapshot"),
  check: (component: Component | null = null) =>
    invoke("check_updates", { component }),
  download: (components: Component[]) =>
    invoke("download_updates", { components }),
  cancel: (component: Component) => invoke("cancel_update", { component }),
  automatic: (component: Component, expected: boolean, enabled: boolean) =>
    invoke<Settings>("set_automatic_updates", { component, expected, enabled }),
  saveNetwork: (expectedRevision: number, network: Network) =>
    invoke<Settings>("set_download_network", { expectedRevision, network }),
  diagnose: (prefix: string | null) =>
    invoke<
      { name: string; ok: boolean; elapsedMs: number; error: string | null }[]
    >("test_update_mirror", { prefix }),
  mirrors: () => invoke<Mirror[]>("builtin_mirrors"),
  restart: () => invoke("request_restart"),
  maintain: (action: "check" | "update" | "retry") =>
    invoke("core_maintenance", { action }),
  cancelCore: () => invoke("cancel_core_maintenance"),
  managed: () => invoke("use_managed_core"),
  release: (component: Component) =>
    invoke("open_update_release", { component }),
  dirty: (dirty: boolean) => invoke("set_update_draft_dirty", { dirty }),
  exit: () => invoke("request_exit"),
  profilePreview: (input: string) =>
    invoke<{
      ticket: number;
      nodesRevision: number;
      items: {
        line: number;
        node: NodeSummary | null;
        error: string | null;
        duplicate: boolean;
      }[];
    }>("profile_import_preview", { input }),
  profileCancel: () => invoke("profile_import_cancel"),
  profileCommit: (ticket: number, expectedRevision: number) =>
    invoke<ProfileSnapshot>("profile_import_commit", {
      ticket,
      expectedRevision,
    }),
  profileRename: (
    id: string,
    name: string,
    tags: string[],
    expectedRevision: number,
  ) =>
    invoke<ProfileSnapshot>("profile_rename_node", {
      id,
      name,
      tags,
      expectedRevision,
    }),
  profileDelete: (id: string, expectedRevision: number) =>
    invoke<ProfileSnapshot>("profile_delete_node", { id, expectedRevision }),
  changeRouting: (change: RoutingChange, expectedRevision: number) =>
    invoke<RoutingResult>("routing_change", { change, expectedRevision }),
  switchMode: (mode: Profile["mode"], expectedRevision: number) =>
    invoke<ProfileSnapshot>("session_switch_mode", { mode, expectedRevision }),
};
