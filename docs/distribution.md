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

`Check` 验证源码与 release 程序启动；打包工作流也执行构建程序 smoke。正式发布前还必须下载实际包，在相应系统与架构安装或提取，并验证包内程序的前端加载、IPC、启动及退出；并执行实际核心集成测试。当前尚未把安装包内程序验收自动化，不可把现有 smoke 当作该项完成。

## 发布

完成五平台原生包验收后，在同一个草稿检查附件、来源提交与测试结果，再正式发布。保存 `docs/releases/<版本>.md`，包含来源提交、原始 Actions run、Release 链接和实际通过 / 跳过范围；不覆盖已经正式发布的同版本附件。

首版尚无正式 Release。当前 Windows 安装器未进行发行商代码签名，macOS 配置使用 ad-hoc 签名、未完成公证；分发前需明确处理这些目标系统上的安装体验。应用尚无在线 updater，因此不配置或复制 SoDeck 的 updater 密钥。
