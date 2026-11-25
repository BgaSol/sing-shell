# 构建与分发

GitHub Actions 执行干净环境构建，GitHub Releases 分发安装包。壳与 sing-box 上游是独立组件：当前只打包壳，不包含、下载或更新 sing-box 程序；用户自行获取对应上游核心并遵守其许可证。项目没有与 sing-box 上游的官方关联。

## 平台和打包

| 平台 | 原生 runner | 包 |
| --- | --- | --- |
| Windows x86_64 | windows-2025 | NSIS |
| Windows ARM64 | windows-11-arm | NSIS |
| Linux x86_64 | ubuntu-22.04 | deb / AppImage |
| Linux ARM64 | ubuntu-22.04-arm | deb / AppImage |
| macOS ARM64 | macos-15 | dmg / app |

手动运行 `Package Sing Shell`，使用普通版本标签 `v<tauri.conf.json 中版本>` 准备同一版本的 Release 草稿。Tauri 官方 action 创建草稿并上传五个平台的附件，默认不正式发布。选择需要发行的固定提交执行，避免开发中的 main 变动。

`Check` 验证源码与 release 程序启动。打包工作流通过 `scripts/verify-package.mjs` 验收本次实际包：Windows 静默安装 NSIS 到独立临时目录、启动安装后的程序并卸载；Linux 解包 DEB 并启动包内程序，再运行 AppImage；macOS 挂载 DMG、启动包内程序，存在 app 归档时另行解包启动。每个包检查前端加载、IPC 及退出，PE / ELF / Mach-O 检查可执行文件的原生架构。

随后下载固定的官方 sing-box 1.14.4 对应平台核心，核对已登记的上游归档 SHA-256，运行真实 SOCKS5 回环转发、配置检查、启停及端口释放测试。该核心仅用于 CI 验证，不进入壳的安装包。报告包括来源提交、Actions run、包大小与摘要、实际验证和跳过范围；发布时迁入 `docs/releases/`，不能只依赖会过期的 Artifact。

## 发布

完成五平台原生包验收后，在同一个草稿检查附件、来源提交与测试结果，再正式发布。保存 `docs/releases/<版本>.md`，包含来源提交、原始 Actions run、Release 链接和实际通过 / 跳过范围；不覆盖已经正式发布的同版本附件。

当前 Windows 安装器未进行发行商代码签名，macOS 配置使用 ad-hoc 签名、未完成公证，下载后可能出现系统的未知发行商或 Gatekeeper 提示；本次验收不包含这些分发提示的交互。应用尚无在线 updater，因此不配置或复制 SoDeck 的 updater 密钥。
