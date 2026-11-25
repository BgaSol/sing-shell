# 开发与验证

工具链由 `.node-version`（Node.js）和 `rust-toolchain.toml`（Rust / rustfmt）固定。系统依赖按 [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) 安装，不依赖 SoDeck 目录或开发者的绝对路径。

| 命令 | 内容 |
| --- | --- |
| `npm ci` | 按 npm 锁文件安装 |
| `npm run dev` | Tauri 开发应用，Vite 使用 1421 端口 |
| `npm run check` | TypeScript 类型及 Rust 格式 |
| `npm run scripts:test` | 工程入口一致性检查 |
| `npm run web:build` | 前端生产构建 |
| `npm run test` | Rust 核心生命周期集成测试 |
| `npm run build` | Tauri 生产编译与平台安装包 |
| `node scripts/smoke.mjs` | 构建后启动 release 程序，等待前端就绪、IPC 完成并验证退出 |

主版本只修改 `src-tauri/tauri.conf.json`；Cargo 使用内部版本 `0.0.0`。提交 `package-lock.json` 和 `src-tauri/Cargo.lock`。

## 回归范围

`src-tauri/tests/lifecycle.rs` 使用本仓库编译的真实夹具进程，验证：

- 配置引用、保存恢复、空格及非 ASCII 路径、相对文件路径。
- 已运行时拒绝重复启动及更换配置。
- 无效配置导入不覆盖原选择，启动时重新校验被修改的文件。
- 异常退出识别及后续重新启动。
- 停止和应用销毁后的进程 / 监听端口回收。
- stdout / stderr 采集、长日志拆分、损坏设置保护、同目录实例锁。

实际核心集成测试独立执行。下载并解压官方核心，将绝对路径写入 `SING_SHELL_TEST_CORE` 后运行：

```sh
cargo test --manifest-path src-tauri/Cargo.toml --locked --test real_core -- --ignored
```

此测试以混合入口的 SOCKS5 协议转发一条本机 TCP 往返，验证配置导入、真实核心启动、转发、停止和端口释放；不修改系统代理，不访问第三方站点。默认测试明确跳过此项，不能把默认通过当作真实核心已验证。

## 持续集成与平台

`.github/workflows/check.yml` 在五个原生 runner 上执行类型、格式、脚本、Rust 测试、生产构建和启动 smoke。ARM64 通过 Rust host 检查拒绝意外使用 x86_64；Linux WebView smoke 在独立 D-Bus / Xvfb 会话中运行，并设置 `WEBKIT_DISABLE_DMABUF_RENDERER=1`，避免无 GPU 桌面的渲染等待。打包工作流使用相同环境。

并行测试目录使用进程 ID、时间戳及进程内原子序号，并以 `create_dir` 拒绝复用已有目录，避免时钟精度不足造成不同测试争用同一实例锁。

初始本机验证环境为 Windows x86_64。其余平台仅在对应 CI / 实机执行成功后报告通过；矩阵存在不等于已验收。构建程序 smoke 不替代实际安装包提取或安装后的验收；打包工作流在每个平台运行 `scripts/verify-package.mjs`，实际范围见[分发](distribution.md)。
