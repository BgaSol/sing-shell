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
  bypassTun?: boolean;
  match: PolicyMatch;
}
export interface Routing {
  defaultExit: RouteAction;
  policies: Policy[];
}
export interface RemovedRule {
  policy: Policy;
  index: number;
  preference: RulePreference | null;
}
export type RoutingChange =
  | { kind: "add" | "edit"; policy: Policy }
  | { kind: "addSources"; ids: string[] }
  | { kind: "exit"; id: string; exit: RouteAction }
  | { kind: "enabled"; id: string; enabled: boolean }
  | { kind: "move"; id: string; direction: -1 | 1 }
  | { kind: "default"; exit: RouteAction }
  | { kind: "remove"; id: string }
  | { kind: "restore"; removed: RemovedRule };
export interface RoutingResult {
  routing: Routing;
  routingRevision: number;
  removed: RemovedRule | null;
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
  connection: ConnectionOptions;
}
export interface ConnectionOptions {
  ipv4Compatibility: boolean;
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
  connectionRevision: number;
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
    failureKind:
      | "network"
      | "package"
      | "cache"
      | "local"
      | "compatibility"
      | "conflict"
      | null;
    warning: string | null;
    preparedVersion: string | null;
  };
  initialized: boolean;
  usable: boolean;
  notice: string | null;
  configError: string | null;
  restore: "none" | "waiting" | "starting" | "finished";
  canStart: boolean;
  canSelectNode: boolean;
  canConnect: boolean;
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
  runningConnectionOptions: ConnectionOptions | null;
  runningBypass: boolean;
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
