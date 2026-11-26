import { invoke } from "@tauri-apps/api/core";
export type Component = "app" | "engine";
export interface Network {
  revision: number;
  fallbackEnabled: boolean;
  disabledBuiltins: string[];
  customMirrors: { name: string; prefix: string; enabled: boolean }[];
}
export interface Preferences {
  automatic: boolean;
  automaticCore: boolean;
  network: Network;
}
export interface ManagedRef {
  version: string;
  target: string;
  hash: string | null;
}
export type RouteAction = "proxy" | "direct";
export interface DomainMatch {
  value: string;
  includeSubdomains: boolean;
}
export type PolicyMatch =
  | { kind: "custom"; domains: DomainMatch[]; ipCidrs: string[] }
  | { kind: "source"; sourceId: string };
export interface Policy {
  id: string;
  name: string;
  enabled: boolean;
  exit: RouteAction;
  match: PolicyMatch;
}
export interface Routing {
  defaultExit: RouteAction;
  policies: Policy[];
}
export interface NodeSummary {
  id: string;
  name: string;
  source: string;
  protocol: string;
  server: string;
  port: number;
  tags: string[];
}
export interface Profile {
  mode: "generated" | "externalConfig";
  nodes: NodeSummary[];
  selectedNode: string | null;
  routing: Routing;
}
export interface ProfileSnapshot extends Profile {
  revision: number;
  routingRevision: number;
}
export interface Settings {
  schemaVersion: number;
  coreRevision: number;
  nodesRevision: number;
  routingRevision: number;
  preferencesRevision: number;
  profile: Profile;
  rulePreferences: RulePreference[];
  coreSource: { mode: "managed" } | { mode: "external"; path: string };
  managed: {
    active: ManagedRef | null;
    previous: ManagedRef | null;
    trial: "awaitingStart" | "starting" | null;
  };
  configPath: string | null;
  pendingConfigPath: string | null;
  updatePreferences: Preferences;
  startup: { restoreCoreOnLaunch: boolean; resumeCore: boolean };
  desktop: { backgroundNoticeShown: boolean };
}
export interface RulePreference {
  sourceId: string;
  automatic: boolean;
  intervalHours: 6 | 24 | 168;
}
export interface SourceVersion {
  sourceId: string;
  commit: string;
  sha256: string;
  size: number;
}
export interface RuleSource {
  definition: {
    id: string;
    name: string;
    repository: string;
    branch: string;
    file: string;
    domains: boolean;
    license: string;
  };
  version: SourceVersion | null;
  status: {
    phase: string;
    lastAttempt: number;
    lastSuccess: number;
    error: string | null;
  };
  preference: RulePreference | null;
}
export interface ActivitySummary {
  sessionId: string | null;
  state: string;
  observed: number;
  active: number;
  upload: number;
  download: number;
  uploadRate: number | null;
  downloadRate: number | null;
  sampledAt: number | null;
  error: string | null;
}
export interface Connection {
  sequence: number;
  id: string;
  target: string;
  host: string | null;
  protocol: string;
  process: string | null;
  exit: RouteAction | null;
  policyId: string | null;
  policyName: string | null;
  rule: string;
  started: string;
  upload: number;
  download: number;
  active: boolean;
  lastSeen: number;
}
export interface ActivityPage {
  summary: ActivitySummary;
  items: Connection[];
  next: number | null;
  truncated: boolean;
}
export interface ActivityQuery {
  sessionId: string | null;
  after: number | null;
  state: "all" | "active" | "ended";
  search: string;
  limit: number;
}
export interface Log {
  sequence: number;
  receivedAt: number;
  source: string;
  message: string;
}
export interface SessionEvent {
  sequence: number;
  at: number;
  kind: string;
  outcome: string;
  message: string;
}
export type SessionPhase =
  | "stopped"
  | "checking"
  | "authorizing"
  | "preparing"
  | "starting"
  | "running"
  | "stopping"
  | "failed"
  | "cleanupRequired";
export interface CoreState {
  maintenance: {
    phase:
      | "idle"
      | "checking"
      | "downloading"
      | "preparing"
      | "ready"
      | "paused"
      | "error";
    candidate: Candidate | null;
    error: string | null;
    downloadedBytes: number;
    totalBytes: number | null;
    network: { endpoint: string; attempt: number; error: string | null } | null;
    lastCheckedAt: number;
    nextCheckAt: number;
  };
  initialized: boolean;
  usable: boolean;
  notice: string | null;
  configError: string | null;
  restore: "none" | "waiting" | "starting" | "finished";
  canStart: boolean;
  canStop: boolean;
  canImport: boolean;
  canRetry: boolean;
  authorizationStage:
    | "registration"
    | "activation"
    | "authentication"
    | "protocol"
    | null;
  phase: SessionPhase;
  pendingChanges: boolean;
  runningNode: NodeSummary | null;
  sessionId: string | null;
  target: NodeSummary | null;
  operation: string | null;
  activity: ActivitySummary | null;
  network: {
    interface: boolean;
    ipv4: boolean;
    ipv6: boolean;
    routes: boolean;
  };
}
export interface Snapshot {
  settings: Settings;
  core: CoreState;
  runtime: {
    cleanupPending: boolean;
    status: string;
    pid: number | null;
    version: string | null;
    error: string | null;
  };
  runtimeBusy: boolean;
  storageError: string | null;
  closing: boolean;
}
export interface Candidate {
  component: Component;
  version: string;
  target: string;
  notes: string;
  releaseUrl: string;
}
export interface UpdateItem {
  phase: string;
  candidate: Candidate | null;
  error: string | null;
  errorStage: string | null;
  downloadedBytes: number;
  totalBytes: number | null;
  network: { endpoint: string; attempt: number; error: string | null } | null;
}
export interface UpdateSnapshot {
  items: Partial<Record<Component, UpdateItem>>;
  lastCheckedAt: number;
  nextCheckAt: number;
  checking: boolean;
  initialized: boolean;
}
export interface Mirror {
  id: string;
  prefix: string;
  supportsApi: boolean;
}
export const core = {
  snapshot: () => invoke<Snapshot>("app_snapshot"),
  session: () =>
    invoke<
      Omit<Snapshot, "settings"> & {
        revisions: number[];
        startup: Settings["startup"];
      }
    >("session_snapshot"),
  connect: (nodeId: string) => invoke("session_connect", { nodeId }),
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
  save: (expectedRevision: number, preferences: Preferences) =>
    invoke<Settings>("set_update_preferences", {
      expectedRevision,
      preferences,
    }),
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
  validateRouting: (routing: Routing) =>
    invoke<Routing>("routing_validate", { routing }),
  profileRouting: (routing: Routing, expectedRevision: number) =>
    invoke<ProfileSnapshot>("profile_set_routing", {
      routing,
      expectedRevision,
    }),
  profileMode: (mode: Profile["mode"], expectedRevision: number) =>
    invoke<ProfileSnapshot>("profile_mode", { mode, expectedRevision }),
};
