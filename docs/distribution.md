# 构建与分发

源码、正式包及清单使用公开的 `BgaSol/sing-shell` 同一仓库，GitHub Actions 构建，GitHub Releases 分发。安装包仅包含壳；首次使用自动下载上游 sing-box，受管更新独立进行，二者独立版本。

## 平台与签名

五个原生目标统一定义在 `scripts/targets.mjs`：Windows x86_64 / ARM64（NSIS），Linux x86_64 / ARM64（DEB、AppImage），macOS ARM64（DMG、app 归档）。Check 与 Package 共用矩阵，ARM64 包验收检查真实 PE/ELF/Mach-O 架构。macOS 在原生 macOS 15 构建、macOS 26 验收，避免旧运行器池容量不足阻断原包复验；实际验收系统记录在报告中。

应用版本只修改 `src-tauri/tauri.conf.json`；标签为普通 `v<major.minor.patch>`。验收可用临时配置构建更高测试候选，不修改源码版本，不把候选上传正式 Release。

Sing Shell 使用独立 Tauri updater 密钥。公钥入应用配置，发行配置 `tauri.release.conf.json` 启用签名；私钥备份位于维护者 `~/.sing-shell-signing/updater.key`，仅由 Actions Secret `TAURI_SIGNING_PRIVATE_KEY` 提供，不提交或发行、不复用 SoDeck 身份。普通开发无需密钥，正式包缺签名即失败。更换公钥需兼容迁移，不能令既有客户端无法更新。

updater 签名不等于 Windows 发行商证书或 macOS 公证。目前 Windows 未做发行商代码签名、macOS 仅 ad-hoc 签名，未公证；互联网下载后的 SmartScreen/Gatekeeper 交互另行记录。

## 一次构建、独立验收

手动运行 `Package Sing Shell`：

1. 五平台原生 package 构建签名包，收集版本、源码、目标、文件名、大小和 SHA-256；同源码另构建更高签名候选，供隔离原生升级测试。
2. 上传明确平台 Artifact，独立 verify 下载同一份原包，验签、实际安装/解包、启动、IPC 和退出，并验证官方核心及从签名基线到更高候选的原生安装器更替、实际版本与设置保留。
3. metadata 核对五份成功报告与原包摘要，生成 `core-update.json`，将验证过的字节上传为同一固定标签的 Release 草稿。
4. 确认包来源提交的相关 Check 结果，正式发布同一草稿并设 Latest；不再推相同版本标签重复构建，不覆盖正式同版本附件。

失败优先重跑失败的平台/verify/metadata。有效 Artifact 可通过明确原始 `package_run_id` 复验和汇总，构建跳过；不能填只有汇总的复用运行或猜最近一次运行。过期/缺失时重建固定源码，不能换来源覆盖同版本。

机器清单 ID 为 `sing-shell.app`，包含唯一版本、说明及按 Rust target 区分的原始 URL、Tauri 签名、大小和 SHA-256，由验收产物生成。Windows 使用 NSIS，macOS 使用 app.tar.gz，Linux 使用 AppImage；DEB 通过系统安装方式升级。

## 实际验收

- Windows 安装 NSIS 到隔离目录，启动安装后程序，再由官方 updater 安装更高签名候选，确认实际版本与配置，最后卸载。
- Linux 验 DEB 包内程序和 AppImage；验证 AppImage 官方更新，并用系统包管理安装/升级 DEB。
- macOS 挂载 DMG及解包 app 归档；复制到可写位置后经官方 updater 更新，确认版本与配置。
- 五个平台下载固定官方 sing-box 1.14.4，核对上游摘要，验证 SOCKS5 回环、启停和端口释放，以及生产用例驱动的受管准备/采用、架构/版本/配置检查和篡改拒绝；同源托管集成覆盖首次自动下载、更新、取消、一次回退及恢复。核心仅测试使用，不进入壳包。

实际包同时验证已发布签名 v0.2.0 → 本版的原生 updater 和 schema 1→2 迁移，并验证本版→隔离高版本候选；旧版外部选择和关闭的核心更新偏好必须保留。

夹具由 test-fixtures 限定，不发行，生产不提供测试源、公钥或任意安装路径入口。smoke 禁止生产调度，核对实际应用版本、目标、前端及 IPC；新包能启动不等于升级已通过。

发行记录保留组件版本、原包源码、原始 run、Release 链接、实际通过/跳过范围，并将原始摘要报告迁入 `docs/releases/`，不能只依赖 30 天 Artifact。v0.1.0 无 updater，首个签名基线需手动安装，不以旧版手工覆盖作为在线更新验收。
