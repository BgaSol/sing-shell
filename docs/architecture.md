# 架构

Sing Shell 使用 Tauri 2 / Rust 与 TypeScript 原生 DOM / Vite。功能合同见[核心与配置](features/core.md)和[软件更新](features/updates.md)。

## 职责与链路

`页面 → Tauri IPC → CoreService / UpdateService → 接纳与持久化 → 进程或平台安装器`。

- `main.rs` 只装配命令、文件框和原生窗口事件；文件框回调返回后才接纳选择，系统文件框不占核心变更票据。
- `app.rs` 组合服务并拥有一次启动的后台唤醒循环。应用和核心分别到期判断，不因一项网络操作等待另一项。
- `core/mod.rs` 的 CoreService 是来源、配置、受管选择、启停、采用和回退的唯一用例入口；它消费准备结果，不把采用交给下载回调。
- `core/managed.rs` 的 ManagedCore 拥有候选、下载准备、核心维护周期和缓存，不自行改当前选择或运行代理。
- `core/package.rs` 核对/解包核心文件和受管路径，提供可取消的散列与清理，不读取当前配置或决定来源。
- `runtime.rs` 拥有临时校验进程、运行 Child、日志 reader、启动观察和真实退出结果；使用固定 StartSpec，不保存用户设置。supervisor 独占运行 Child，启动调用等待其有界结果。
- `settings.rs` 是设置的唯一串行写入者，分别校对核心与偏好修订；`platform.rs` 适配 Windows Job / Unix 进程组，`logs.rs` 保存有界内存日志。
- `updates/service.rs` 只负责应用检查、下载和准备；`application.rs` 调用 Tauri 官方 updater。`release.rs/network.rs` 复用两个具体发行适配与传输，不提供通用更新框架。
- `lifecycle.rs` 封闭接纳、取消维护、等待所有者、停止核心，再处理退出或应用安装；不采用核心版本。

前端只呈现快照、编辑偏好和发起意图，不维护自动更新计时器。仍只有运行与软件更新两页，不引入框架、数据库、插件协议或升级守护进程。

## 设置、缓存与恢复

settings schema 2 的 `coreSource` 区分 managed 和 external；managed 保存 active/previous、首次启动 trial 与至多一个 blocked 候选。受管引用绑定版本、target、程序摘要和可用的上游证据，路径从本机应用目录推导。configPath 是已检查路径，pendingConfigPath 是缺核心时先选择的路径，不上传配置正文。

coreRevision 与 preferencesRevision 分开。每次局部提交从最新设置合并对应字段，不用后台旧快照覆写整个文件。采用时在同一串行提交内检查核心上下文、关闭/取消及自动采用偏好。磁盘写入临时文件同步后替换；设置损坏或未知 schema 保留原件，阻止覆盖。

v0/v1 先备份再迁移；外部选择不变，v1 受管继承旧自动检查开关且不猜测上一版本。legacy 证据与官方新下载证据明确区分。迁移不联网，不在设置读取时运行外部程序。

核心准备记录最后提交，绑定来源请求和选择代次；恢复不相信上次进度文案，重新核对本地字节。active、previous 和 trial 一次保存，确认启动才清 trial，未确认中断按恢复规则处理。进程 PID/版本来自真实进程快照，不从预期选择伪造。

## 并发边界

短接纳锁管理具体操作：核心变更互斥、单个核心下载准备、单个应用检查/下载、偏好写入和诊断。下载与当前代理可并行。采用和清理另获取核心文件票据，保护 staging 与版本目录；网络 worker 释放维护票据后再请求采用，避免循环持锁。

进度/错误带任务所有权，来源变更使旧操作失效；线路设置变化不污染新健康结果。配置变化使兼容结论失效，每次启动重验。散列、解包和检查接受取消，票据在实际资源收尾后释放。退出不把取消请求当作完成，失败时保留实际状态、允许重试。

本地版本切换无需停止正在运行的进程；只在已停止或用户下次启动前采用。应用更新与核心各自完成，不提供跨组件事务。Unix 宿主被 SIGKILL 无法执行退出清理；Windows Job 使用 KILL_ON_JOB_CLOSE。停止并非平滑关闭。

## 依赖

沿用已固定的 Tauri updater/opener、reqwest/rustls、Tokio、semver、sha2、zip/tar/flate2，不新增生产依赖或另一套运行时。应用验签与安装交给官方 updater，配置、网络协议、DNS 和转发语义交给 sing-box。工程命令和平台验证见[开发](development.md)。
