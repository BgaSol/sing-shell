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
| `node scripts/smoke.mjs` | 构建后启动 release 程序，等待前端、IPC 并验证版本/目标和退出 |

主版本只修改 `src-tauri/tauri.conf.json`；Cargo 使用内部版本 `0.0.0`。提交 `package-lock.json` 和 `src-tauri/Cargo.lock`。

## 回归范围

`src-tauri/tests/lifecycle.rs` 经 SessionService 驱动本仓库编译的真实夹具进程，验证：

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

测试以混合入口的 SOCKS5 协议转发本机 TCP 往返。受管核心专项还需将原始归档的绝对路径写入 `SING_SHELL_TEST_ARCHIVE`，验证准备不覆盖外部程序、配置兼容和准备后篡改拒绝。不修改系统代理或访问第三方站点；默认测试明确跳过真实核心项，不能当作已验证。

`src-tauri/tests/managed_core.rs` 使用两个不同版本的真实夹具进程、本地固定发行源与归档，覆盖无前端首次自动下载、pending 配置、运行中准备不切换、偏好修订、取消、兼容失败、启动观察、一次回退、启动中断恢复及缓存修复。测试端点只在 test-fixtures 构建中可用，不进入生产客户端；不操作桌面或用户真实代理。

## 持续集成与平台

`.github/workflows/check.yml` 从 `scripts/targets.mjs` 读取五原生目标，执行类型、格式、脚本、Rust 测试、生产构建和 smoke。ARM64 通过 Rust host 检查；Linux smoke 使用独立 D-Bus / Xvfb，设置 `WEBKIT_DISABLE_DMABUF_RENDERER=1`。

并行测试目录使用进程 ID、时间戳及进程内原子序号，并以 `create_dir` 拒绝复用已有目录，避免时钟精度不足造成不同测试争用同一实例锁。

原有生命周期回归外，测试覆盖旧设置迁移、修订冲突、慢校验取消/快照读取、关闭接纳、正式版本选择、坏签名与版本篡改、原站/镜像 404、网络限额和线路重试。smoke 在隔离数据目录中模拟未保存草稿并请求关闭，确认窗口仍保留到统一退出入口，防止窗口先销毁、进程却停留后台；不需要桌面输入。真实升级由不发行的 `upgrade-fixture` 驱动官方 updater；更高版本仅用临时配置生成，正式版本来源仍只有 Tauri 配置。

矩阵存在不等于验收通过。打包工作流通过独立 `verify` 下载原始 Artifact，运行 `scripts/verify-package.mjs`，检查原生实际包、签名、设置保留、原生升级及真实核心。具体范围见[分发](distribution.md)。

## 桌面与恢复回归

startup_restore 覆盖无前端的一次恢复、主动停止/来源选择/关闭恢复开关撤销、启动与诊断并行、失败不重试、迁移备份和字段级持久化、意图保存失败仍停止。instance 单元测试覆盖真实管道/socket 激活和退出拒绝；登录项测试只操作测试注册表或可丢弃文件。

Windows 的 `scripts/windows-installer.test.mjs` 使用 Tauri 下载的 NSIS 编译器实际编译并执行安装 hook 夹具，覆盖旧用户级更新、自定义中文/空格路径、跨范围重复安装拦截、保留数据卸载后的残留路径以及卸载范围校正，只写独立测试注册表。编译器不存在时明确跳过；Check 在 Windows NSIS 打包后执行此回归。系统级实际安装/更新/卸载由 Package 的两种范围验收负责，不能用夹具或构建成功替代。

## 节点、分流与授权网络验收

profile 回归覆盖 schema 3→4 原字节备份、独立修订、原子导入、票据重放拒绝和快照凭据保护；单元覆盖协议解析、未知字段、网段规范化和固定规则摘要。设置 `SING_SHELL_TEST_CORE` 后执行 `cargo test --manifest-path src-tauri/Cargo.toml --locked --test profile -- --include-ignored`，以官方 1.14.4 检查全部支持协议配置，再运行两个受控加密出口实测域名、CIDR、集合和默认动作。出口探针只替换传输和入口，不重写生成的路由或 SRS；不改变系统网络。

`import_links` 覆盖 VLESS `encryption=none` 的等价转换、严格单次解码、TLS 别名冲突、传输字段边界、行号与安全错误。profile 集成覆盖预览/提交/选择、存储重开、生成配置保真，以及新预览整体失败、取消、过期、修订冲突和原子写入失败；过期与迟到完成使用确定性状态测试。官方配置检查包含完整 VLESS + TLS + WebSocket 分享链接和显式 SNI 的 gRPC 样例。Windows 手动执行这些集成测试也须加 `--features test-fixtures`，使测试程序包含原生对话框 manifest；`npm run test` 已包含该参数。

工程脚本回归还覆盖“导入节点 → 选择节点 → 保存分流”的连续交互状态，防止忙状态未释放或干净表单未跟随节点修订。协议检查包含 VMess、WebSocket、gRPC、TLS 和 Reality；配置通过不能代替各远端协议的真实服务器互通。

文件读取/严格 UTF-8 解码失败、预览异常、全部无效和部分有效时的票据/按钮状态，以及恶意名称按文本显示，由 `scripts/profiles.test.mjs` 回归。合法链接预览不启动核心；未提供 SNI 的链接能导入不代表远端握手已验证。

真实 TUN 仅在可丢弃的原生运行器运行 `.github/workflows/network-preflight.yml`。基础探测限制为两个测试 IP；生产 worker 集成使用正式授权通道、独立官方核心验证、生成配置、TCP/UDP IPv4/IPv6 和接口退出观测。实际包复用该链路，需环境 `SING_SHELL_NETWORK_PREFLIGHT=1`，并已由系统提权；不会在维护者日常桌面运行。

macOS 网络 CI 使用独立 root 入口验证同一生产 worker；它不能代表普通用户的系统管理员授权弹窗已验证。完整安装包可通过 `SING_SHELL_NETWORK_PREFLIGHT=1 "/Applications/Sing Shell.app/Contents/MacOS/sing-shell" --smoke-test --network-authorization` 验证系统弹窗、双方身份与握手及 worker 退出，不传节点、不启动核心，不能以 root 运行该探针。系统允许仍由用户完成。

实机使用 `SING_SHELL_MACOS_AUTHORIZATION=1` 的网络集成模式，经普通安装应用的系统授权启动 worker，验证真实 TUN、加密 IPv4/IPv6 TCP/UDP、停止与 helper 退出；不在用户机器注入核心或控制端崩溃。该变量只选择验收范围，不改变生产授权路径。崩溃清理由五个可丢弃原生运行器验证。

授权协议回归覆盖延迟就绪、错误身份不发送确认、取消、超时、超限、错误 nonce 和旧协议。macOS 原生回归覆盖运行映像被替换后的拒绝、套接字身份和含引号、命令替换及换行路径的授权参数传递，不实际提权。旧服务迁移回归使用用户 EX_CONFIG 样本，未知格式、其他归属、有 PID 或活动数不为零的作业不得停用。隧道回归覆盖关闭超时保留同一线程、重试回收及只读状态；纯节点恢复不依赖原始配置路径。

原生 smoke 分别断言收起与真正退出，读取登录项适配测试 IO。实际包额外用官方 sing-box 执行启动恢复、收起后的 SOCKS5 回环、第二实例唤回同一窗口、PID 保留和真正退出后的端口释放。Linux Xvfb 配合 Openbox 验证实际最小化。合成登录参数不代表真实注销/登录；物理托盘展示、macOS 系统允许提示和实际登录会话未执行时记录为跳过。
