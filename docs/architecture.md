# 架构

Sing Shell 复用 SoDeck 的技术分层，以最小桌面壳管理一个本地 sing-box 进程。仓库、标识、数据目录、版本和构建均独立于 SoDeck。

```text
TypeScript / 原生 DOM / Vite
            │ Tauri IPC（操作与快照）
            ▼
Tauri 2 桌面适配（main.rs）
            │
            ▼
Rust 核心（core.rs）── 设置 / 校验 / 状态 / 有界日志
            │
            ▼
平台适配（platform.rs）── 原生路径 / 进程组
            │ argv + stdout/stderr
            ▼
独立 sing-box 程序 ── 代理协议 / DNS / 路由 / 网络转发
```

## 代码与契约

- `src/main.ts`：页面状态、操作、防重复提交、日志与失败展示。快照是事实来源；刷新期间过期结果不能覆盖新操作。
- `src/core.ts`：前端调用及类型。`core_snapshot` 读取真实状态；`core_action` 接受 `check` / `start` / `stop`；`choose_file` 通过原生文件框选择核心或配置。
- `src-tauri/src/main.rs`：Tauri 装配、原生文件框、应用目录、IPC 和退出收尾。耗时操作放在阻塞工作线程，不阻塞 WebView 事件循环。
- `src-tauri/src/core.rs`：核心选择、配置校验、设置保存、进程真实状态及有界日志。操作由同一实例互斥串行处理；日志 reader 只持日志锁，不反向获取操作锁。
- `src-tauri/src/platform.rs`：Windows 无控制台挂起启动、Job Object 归属及恢复；Unix 独立进程组。停止后等待真实退出并回收输出线程。

前后端随同一应用版本发行，当前无需独立 RPC 主版本。sing-box 通过命令行调用，不套 SoDeck 的插件协议；以后若引入 Clash API，应由 Rust 适配并校验本机地址与凭据，前端不直接持有控制接口密钥。

## 数据和生命周期

设置使用临时文件写入后替换；读取失败保留原文件。应用数据目录持文件锁。配置正文属于用户文件，壳只记路径；核心和配置均以独立原生参数传递，不通过 shell 拼接。

核心与检查进程均加入受管进程组。Windows Job 设置 `KILL_ON_JOB_CLOSE`，应用进程消失时操作系统会回收作业；Unix 正常退出由应用清理，应用被 SIGKILL 等强制终止时不能执行清理，因此不能保证这种情况下核心被回收。

状态刷新通过 `try_wait` 观察实际子进程；日志不决定连接状态。具体用户行为、失败和限制统一见[核心与配置](features/core.md)。

## 依赖

仅保留 SoDeck 已使用的 Tauri API / CLI、Tauri dialog、serde / serde_json、Unix libc 和 Windows windows-sys；开发端使用同一 TypeScript / Vite。Tauri 维护桌面适配和原生文件框，serde 处理设置，libc / windows-sys 处理系统进程原语。版本固定在清单及锁文件。

不引入前端框架、数据库、插件 SDK 或 Go 绑定。替代方案是手写平台文件框与 FFI，但会增加维护；嵌入 libbox 会把 Go 编译和 FFI 带入壳的构建，当前独立核心用例不需要。
