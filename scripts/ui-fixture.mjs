// Isolated browser fixture: never imported by the application or included in its build.
import { createServer } from "vite";
import { readFileSync } from "node:fs";
const appVersion = JSON.parse(
  readFileSync("src-tauri/tauri.conf.json", "utf8"),
).version;
function bootstrap(appVersion) {
  window.__SING_SHELL_FIXTURE__ = true;
  const clone = (value) => structuredClone(value);
  const scenario = new URLSearchParams(location.search).get("routingScenario");
  const callbacks = new Map();
  let callbackId = 0,
    ticket = 0,
    documentId = "",
    exitId = 0;
  const definitions = [
    ["cn-domain", "国内站点", true],
    ["cn-ip", "中国大陆 IP", false],
    ["google", "Google 服务", true],
    ["github", "GitHub", true],
    ["telegram", "Telegram", true],
  ].map(([id, name, domains]) => ({
    id,
    name,
    domains,
    repository: id === "cn-ip" ? "Loyalsoldier/geoip" : "SagerNet/sing-geosite",
    branch: id === "cn-ip" ? "release" : "rule-set",
    file:
      id === "cn-ip"
        ? "srs/cn.srs"
        : `geosite-${id === "cn-domain" ? "cn" : id}.srs`,
    license: id === "cn-ip" ? "CC-BY-SA-4.0" : "GPL-3.0-or-later",
  }));
  const policies = definitions.slice(0, 2).map((d) => ({
    id: d.id,
    name: d.name,
    enabled: true,
    exit: "direct",
    match: { kind: "source", sourceId: d.id },
  }));
  policies.push({
    id: "remote",
    name: "远程控制",
    enabled: true,
    exit: "direct",
    bypassTun: true,
    match: {
      kind: "custom",
      domains: [{ value: "id.example.com", includeSubdomains: false }],
      ipCidrs: ["203.0.113.10/32", "2001:db8:71::1/128"],
    },
  });
  policies.push({
    id: "work",
    name: "工作服务",
    enabled: true,
    exit: "proxy",
    match: {
      kind: "custom",
      domains: [
        { value: "example.com", includeSubdomains: true },
        { value: "api.example.net", includeSubdomains: false },
      ],
      ipCidrs: ["192.0.2.0/24", "2001:db8::/32"],
    },
  });
  const settings = {
    schemaVersion: 5,
    nodesRevision: 0,
    routingRevision: 0,
    preferencesRevision: 0,
    coreRevision: 0,
    connectionRevision: 0,
    profile: {
      mode: "generated",
      connection: { ipv4Compatibility: false },
      selectedNode: null,
      nodes: [
        ["香港 · 主线路", ["工作", "常用"], "trojan"],
        ["东京 · 备用", ["备用"], "vless"],
        ["新加坡", [], "shadowsocks"],
        [
          "洛杉矶 · 长名称节点，用于检查列表的自动换行与紧凑布局",
          ["备用", "测试"],
          "vmess",
        ],
      ].map(([name, tags, protocol], i) => ({
        id: `node-${i}`,
        name,
        tags,
        protocol,
        server: `node-${i}.example.com`,
        port: 443,
        source: "演示数据",
      })),
      routing: { defaultExit: "proxy", policies },
    },
    rulePreferences: definitions
      .slice(0, 2)
      .map((d) => ({ sourceId: d.id, automatic: true, intervalHours: 24 })),
    coreSource: { mode: "managed" },
    managed: {
      active: {
        version: "1.14.4",
        target: "x86_64-pc-windows-msvc",
        hash: null,
      },
      previous: null,
      trial: null,
    },
    configPath: null,
    pendingConfigPath: null,
    updatePreferences: {
      automatic: true,
      automaticCore: true,
      network: {
        revision: 0,
        fallbackEnabled: true,
        disabledBuiltins: [],
        customMirrors: [],
      },
    },
    startup: { restoreCoreOnLaunch: false, resumeCore: false },
    desktop: { backgroundNoticeShown: true },
  };
  const coreState = {
    maintenance: {
      phase: "idle",
      candidate: null,
      error: null,
      downloadedBytes: 0,
      totalBytes: null,
      network: null,
      lastCheckedAt: 0,
      nextCheckAt: 0,
      failureKind: null,
      warning: null,
      preparedVersion: null,
    },
    initialized: true,
    usable: true,
    notice: null,
    configError: null,
    restore: "none",
    canStart: false,
    canSelectNode: true,
    canConnect: true,
    canStop: false,
    canImport: true,
    canRetry: true,
    authorizationStage: null,
    phase: "stopped",
    pendingChanges: false,
    runningNode: null,
    runningConnectionOptions: null,
    runningBypass: false,
    target: null,
    operation: null,
    activity: null,
    network: { interface: false, ipv4: false, ipv6: false, routes: false },
  };
  const runtime = {
    cleanupPending: false,
    status: "stopped",
    pid: null,
    version: null,
    error: null,
  };
  let exitRequest = null,
    pending = null;
  const events = [];
  const snapshot = () => ({
    settings: clone(settings),
    core: clone(coreState),
    runtime: clone(runtime),
    runtimeBusy: false,
    storageError: null,
    closing: false,
  });
  const sources = () =>
    definitions.map((definition) => ({
      definition,
      version: {
        sourceId: definition.id,
        commit: "8b8788649349cf73fcd12fc81a5cae580e60360c",
        sha256: "a".repeat(64),
        size: 100,
      },
      status: { phase: "idle", lastAttempt: 0, lastSuccess: 0, error: null },
      preference:
        settings.rulePreferences.find((p) => p.sourceId === definition.id) ??
        null,
    }));
  const connect = (id) => {
    const node = settings.profile.nodes.find((n) => n.id === id);
    if (!node) throw Error("节点不存在");
    coreState.runningNode = clone(node);
    coreState.runningConnectionOptions = clone(settings.profile.connection);
    coreState.runningBypass = settings.profile.routing.policies.some(
      (p) => p.enabled && p.bypassTun,
    );
    coreState.canStop = true;
    coreState.canImport = false;
    coreState.phase = "running";
    coreState.pendingChanges = false;
    runtime.pid = 12345;
    runtime.status = "running";
    runtime.version = "1.14.4";
    if (settings.profile.selectedNode !== id) {
      settings.profile.selectedNode = id;
      settings.nodesRevision++;
    }
    coreState.activity = {
      sessionId: "fixture-session",
      state: "ready",
      observed: 2,
      active: 2,
      upload: 123456,
      download: 987654,
      uploadRate: 8192,
      downloadRate: 49152,
      sampledAt: Date.now(),
      error: null,
    };
    events.push({
      sequence: events.length + 1,
      at: Date.now(),
      kind: "connection",
      outcome: "succeeded",
      message: `演示：${node.name} 已连接`,
    });
  };
  window.__TAURI_INTERNALS__ = {
    transformCallback(fn) {
      const id = ++callbackId;
      callbacks.set(id, fn);
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    async invoke(command, args = {}) {
      switch (command) {
        case "plugin:app|version":
          return `${appVersion} · 浏览器测试数据`;
        case "plugin:event|listen":
          return ++callbackId;
        case "plugin:event|unlisten":
          return;
        case "app_snapshot":
          return snapshot();
        case "session_snapshot":
          return {
            ...snapshot(),
            settings: undefined,
            revisions: [
              settings.nodesRevision,
              settings.routingRevision,
              settings.coreRevision,
              settings.preferencesRevision,
              settings.connectionRevision,
            ],
            startup: clone(settings.startup),
          };
        case "frontend_ready":
          documentId = args.document;
          return;
        case "desktop_snapshot":
          return { error: null, navigation: null, exitRequest };
        case "desktop_navigation_done":
        case "exit_presented":
        case "set_update_draft_dirty":
          return;
        case "request_exit":
          exitRequest = { id: ++exitId, document: documentId };
          return;
        case "exit_decision":
          exitRequest = null;
          return;
        case "update_snapshot":
          return {
            items: {},
            initialized: true,
            checking: false,
            lastCheckedAt: 0,
            nextCheckAt: 0,
          };
        case "builtin_mirrors":
          return [];
        case "login_startup_read":
          return { enabled: false, state: "disabled", reason: null };
        case "login_startup_set":
          return {
            enabled: args.enabled,
            state: args.enabled ? "enabled" : "disabled",
            reason: null,
          };
        case "set_restore_core":
          settings.startup.restoreCoreOnLaunch = args.enabled;
          return;
        case "rule_sources":
          return clone(sources());
        case "rule_preference":
          settings.rulePreferences = settings.rulePreferences
            .filter((p) => p.sourceId !== args.preference.sourceId)
            .concat(args.preference);
          return;
        case "rule_update":
        case "rule_cancel":
          return;
        case "session_connect":
          connect(args.nodeId);
          return;
        case "session_select_node":
          if (args.expectedRevision !== settings.nodesRevision)
            throw Error("节点列表已改变");
          if (
            settings.profile.mode !== "generated" ||
            !settings.profile.nodes.some((n) => n.id === args.nodeId)
          )
            throw Error("节点不可选择");
          if (settings.profile.selectedNode !== args.nodeId) {
            settings.profile.selectedNode = args.nodeId;
            settings.nodesRevision++;
          }
          return;
        case "set_connection_options":
          if (args.expectedRevision !== settings.connectionRevision)
            throw Error("连接设置修订冲突");
          settings.profile.connection = clone(args.options);
          settings.connectionRevision++;
          if (runtime.pid) coreState.pendingChanges = true;
          return clone(settings);
        case "core_action":
          if (args.action === "stop") {
            coreState.runningNode = null;
            coreState.canStop = false;
            coreState.canImport = true;
            coreState.phase = "stopped";
            coreState.activity = null;
            runtime.pid = null;
            runtime.status = "stopped";
          }
          if (args.action === "apply") {
            coreState.pendingChanges = false;
            coreState.runningConnectionOptions = clone(
              settings.profile.connection,
            );
            coreState.runningBypass = settings.profile.routing.policies.some(
              (p) => p.enabled && p.bypassTun,
            );
          }
          return;
        case "profile_rename_node": {
          const node = settings.profile.nodes.find((n) => n.id === args.id);
          node.name = args.name;
          node.tags = clone(args.tags);
          settings.nodesRevision++;
          return {
            revision: settings.nodesRevision,
            routingRevision: settings.routingRevision,
            ...clone(settings.profile),
          };
        }
        case "profile_delete_node":
          settings.profile.nodes = settings.profile.nodes.filter(
            (n) => n.id !== args.id,
          );
          settings.nodesRevision++;
          return;
        case "profile_import_preview":
          pending = {
            ticket: ++ticket,
            nodesRevision: settings.nodesRevision,
            items: args.input
              .split("\n")
              .filter(Boolean)
              .map((line, i) => ({
                line: i + 1,
                duplicate: false,
                error: line.startsWith("bad") ? "无法识别节点链接" : null,
                node: line.startsWith("bad")
                  ? null
                  : {
                      id: `import-${i}`,
                      name: line.includes("<") ? line : "新导入节点",
                      server: "import.example.com",
                      port: 443,
                      protocol: "trojan",
                      source: "测试导入",
                      tags: [],
                    },
              })),
          };
          return clone(pending);
        case "profile_import_cancel":
          pending = null;
          return;
        case "profile_import_commit":
          if (!pending || pending.ticket !== args.ticket)
            throw Error("预览已失效");
          settings.profile.nodes.push(
            ...pending.items.filter((i) => i.node).map((i) => i.node),
          );
          settings.nodesRevision++;
          pending = null;
          return;
        case "routing_change": {
          if (scenario === "slow-save")
            await new Promise((resolve) => setTimeout(resolve, 2500));
          if (scenario === "save-failure")
            throw Error("演示：设置写入失败，草稿已保留");
          if (scenario === "conflict") {
            settings.routingRevision++;
            throw Error("分流已改变，草稿已保留，请载入最新规则核对");
          }
          if (args.expectedRevision !== settings.routingRevision)
            throw Error("分流修订冲突");
          const routing = clone(settings.profile.routing),
            c = args.change;
          const index = routing.policies.findIndex((p) => p.id === c.id),
            policy = routing.policies[index];
          let removed = null;
          if (c.kind === "add")
            routing.policies.push({
              ...clone(c.policy),
              name:
                c.policy.name ||
                c.policy.match.domains[0]?.value ||
                c.policy.match.ipCidrs[0],
            });
          if (c.kind === "edit") {
            const i = routing.policies.findIndex((p) => p.id === c.policy.id);
            routing.policies[i] = clone(c.policy);
          }
          if (c.kind === "exit") {
            policy.exit = c.exit;
            if (c.exit === "proxy") policy.bypassTun = false;
          }
          if (c.kind === "enabled") policy.enabled = c.enabled;
          if (c.kind === "move")
            [routing.policies[index], routing.policies[index + c.direction]] = [
              routing.policies[index + c.direction],
              routing.policies[index],
            ];
          if (c.kind === "default") routing.defaultExit = c.exit;
          if (c.kind === "remove") {
            removed = {
              policy: clone(policy),
              index,
              preference:
                settings.rulePreferences.find(
                  (p) => p.sourceId === policy.match.sourceId,
                ) ?? null,
            };
            routing.policies.splice(index, 1);
          }
          if (c.kind === "restore") {
            routing.policies.splice(
              c.removed.index,
              0,
              clone(c.removed.policy),
            );
            if (c.removed.preference)
              settings.rulePreferences.push(clone(c.removed.preference));
          }
          if (c.kind === "addSources")
            for (const id of c.ids) {
              const d = definitions.find((d) => d.id === id);
              routing.policies.push({
                id: "source-" + id,
                name: d.name,
                enabled: true,
                exit: ["cn-domain", "cn-ip"].includes(id) ? "direct" : "proxy",
                bypassTun: false,
                match: { kind: "source", sourceId: id },
              });
            }
          if (
            routing.policies.some(
              (p) =>
                p.bypassTun &&
                p.match.kind === "custom" &&
                p.match.domains.some((d) => d.includeSubdomains),
            )
          )
            throw Error("绕过 TUN 仅支持精确域名");
          settings.profile.routing = routing;
          settings.routingRevision++;
          settings.rulePreferences = definitions
            .filter((d) =>
              routing.policies.some(
                (p) => p.match.kind === "source" && p.match.sourceId === d.id,
              ),
            )
            .map(
              (d) =>
                settings.rulePreferences.find((p) => p.sourceId === d.id) ?? {
                  sourceId: d.id,
                  automatic: true,
                  intervalHours: 24,
                },
            );
          if (runtime.pid) coreState.pendingChanges = true;
          return {
            routing: clone(routing),
            routingRevision: settings.routingRevision,
            removed,
          };
        }
        case "set_automatic_updates":
          settings.updatePreferences[
            args.component === "app" ? "automatic" : "automaticCore"
          ] = args.enabled;
          settings.preferencesRevision++;
          return clone(settings);
        case "set_download_network":
          if (
            args.expectedRevision !==
            settings.updatePreferences.network.revision
          )
            throw Error("下载网络修订冲突");
          settings.updatePreferences.network = {
            ...clone(args.network),
            revision: args.expectedRevision + 1,
          };
          settings.preferencesRevision++;
          return clone(settings);
        case "check_updates":
        case "core_maintenance":
          return;
        case "session_switch_mode":
          runtime.pid = null;
          runtime.status = "stopped";
          coreState.phase = "stopped";
          coreState.canStop = false;
          coreState.canImport = true;
        case "profile_mode":
          settings.profile.mode = args.mode;
          coreState.canSelectNode = args.mode === "generated";
          coreState.canConnect = args.mode === "generated";
          settings.nodesRevision++;
          return;
        case "activity_snapshot":
          return {
            summary: clone(
              coreState.activity ?? {
                sessionId: null,
                state: "unavailable",
                observed: 0,
                active: 0,
                upload: 0,
                download: 0,
                uploadRate: null,
                downloadRate: null,
                sampledAt: null,
                error: null,
              },
            ),
            items: runtime.pid
              ? [
                  {
                    sequence: 2,
                    id: "fixture-connection",
                    target: "api.example.com:443",
                    host: "api.example.com",
                    protocol: "tcp",
                    process: null,
                    exit: "proxy",
                    policyId: "work",
                    policyName: "工作服务",
                    rule: "rule_set=policy-work => route(proxy)",
                    started: new Date().toISOString(),
                    upload: 12345,
                    download: 54321,
                    active: true,
                    lastSeen: Date.now(),
                  },
                ]
              : [],
            next: null,
            truncated: false,
          };
        case "read_events":
          return clone(events.filter((e) => e.sequence > args.after));
        case "read_logs":
          return args.after
            ? []
            : [
                {
                  sequence: 1,
                  receivedAt: Date.now(),
                  source: "fixture",
                  message: "浏览器测试日志 <script> 只作为文本显示",
                },
              ];
        default:
          throw Error(`浏览器夹具未实现：${command}`);
      }
    },
  };
}
const server = await createServer({
  configFile: "vite.config.ts",
  server: { host: "127.0.0.1", port: 1422, strictPort: true },
  plugins: [
    {
      name: "isolated-ui-fixture",
      transformIndexHtml() {
        return [
          {
            tag: "script",
            attrs: { src: "/__fixture.js" },
            injectTo: "head-prepend",
          },
        ];
      },
      configureServer(server) {
        server.middlewares.use("/__fixture.js", (_req, res) => {
          res.setHeader("Content-Type", "application/javascript");
          res.end(`(${bootstrap.toString()})(${JSON.stringify(appVersion)});`);
        });
      },
    },
  ],
});
await server.listen();
console.log("Isolated UI fixture: http://127.0.0.1:1422");
