# 架构

Sing Shell 使用 Tauri 2 / Rust 与 TypeScript 原生 DOM / Vite。具体行为见[核心与配置](features/core.md)和[软件更新](features/updates.md)。

## 职责与链路

`运行页/更新页 → Tauri IPC → Application 接纳 → SettingsStore / RuntimeService / UpdateService → Lifecycle 收尾 → 原生退出或重启`。

- `main.rs` 负责 Tauri 装配、原生文件框和命令转接，不在原生事件回调同步等待进程。
- `app.rs` 组合服务，以短接纳锁维护 closing 和具体在途操作；票据结束释放占用，取消与真实结束分别处理。
- `settings.rs` 拥有唯一设置、修订、串行写入与实例锁，磁盘操作不持应用全局状态锁。
- `runtime.rs` 负责命令校验、启动、监督与停止，独立观察真实进程退出；快照只读。`logs.rs` 管理有界日志和输出 reader。
- `platform.rs` 负责 Windows Job Object / Unix 进程组，不拼 shell 启动程序。
- `lifecycle.rs` 是退出、重启及更新重启的唯一入口；先验证本地更新，再停止核心、切换受管选择及调用安装器。
- `updates/service.rs` 拥有初始化、调度、两个组件的候选/进度/准备状态；`release.rs` 解释发行，`network.rs` 传输，`application.rs` 调用官方 updater，`engine.rs` 管理受管程序。

只维护应用和引擎两个具体更新对象，不建立插件协议、通用工作流引擎或动态服务注册。后台任务属于应用，事件仅通知重读，快照和已提交结果是事实来源。

## 存储

设置 schema 1 为 `schemaVersion/revision/engine/configPath/updatePreferences`；engine 是未配置、`external {path}` 或 `managed {version,target}`。受管路径由本机应用目录和校验后的版本/目标推导，不由 UI 提供。

v0.1.0 设置迁移时保留 `settings.v0.backup.json`，旧 corePath 成为 external。未知 schema 或损坏文件保留原件并阻止覆盖。保存检查 expectedRevision，临时文件同步后替换。配置正文仍属于用户，使用原绝对路径及父目录工作路径，不复制上传。

更新缓存独立于设置，准备记录绑定 schema、来源、目标、版本和摘要。恢复不相信上次进度文案，重新核对本地字节。稳定状态单独串行保存，避免双组件覆盖同一缓存文件。

实例锁由应用持有直到收尾完成，新实例有界等待重启交接，真正第二实例被拒绝。

## 并发和平台边界

状态锁只维护短状态；网络、解包、校验进程、签名和安装交给有限 worker。supervisor 独占 Child，reader 只写日志。收尾超时保留实际状态、允许重试，不把取消信号当作资源已回收。

Windows Job 使用 KILL_ON_JOB_CLOSE；Unix 正常退出清理进程组，宿主被 SIGKILL 时无法执行收尾。应用与核心各自提交，不提供跨组件原子事务或应用内任意降级。

Tauri 官方 updater 负责应用验签及安装，sing-box 自己负责配置语义、协议、DNS 和路由。前后端同包发行，不增加独立 RPC 主版本。

## 依赖

清单和锁文件固定版本。更新依赖沿用 SoDeck 已使用的 Tauri updater/opener、reqwest/rustls、tokio、semver、sha2、zip/tar/flate2，分别用于官方安装、系统打开、HTTPS、调度、版本、摘要及 Windows/Unix 两种实际归档。由 Tauri 与对应 Rust 上游维护，升级前复核维护状态、安全修复及原生兼容。

替代方案为自建安装器/加密实现，或依赖机器上的 curl、解包命令；本项目使用成熟库减少平台分支，不要求运行机器装开发工具。未增加前端框架、数据库、Go FFI 或升级守护进程。
