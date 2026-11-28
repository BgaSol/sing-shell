# 构建与分发

源码、正式包及清单使用公开的 `BgaSol/sing-shell` 同一仓库，GitHub Actions 构建，GitHub Releases 分发。安装包仅包含壳；首次使用自动下载上游 sing-box，受管更新独立进行，二者独立版本。

## 平台与签名

五个原生目标统一定义在 `scripts/targets.mjs`：Windows x86_64 / ARM64（NSIS），Linux x86_64 / ARM64（DEB、AppImage），macOS ARM64（DMG、app 归档）。Check 与 Package 共用矩阵，ARM64 包验收检查真实 PE/ELF/Mach-O 架构。macOS 在原生 macOS 26 构建和验收，避免旧运行器池容量不足；实际构建/验收系统记录在发行证据中。

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

同一运行重跑可能保留多份同名验收 Artifact。汇总按各目标最新上传的报告 ID 下载，不混入旧报告；最新报告缺失、过期或未通过时拒绝发布，不回退选择旧成功。原失败证据保留，独立复验仍使用明确的 `package_run_id`，原签名包不重建。

机器清单 ID 为 `sing-shell.app`，包含唯一版本、说明及按 Rust target 区分的原始 URL、Tauri 签名、大小和 SHA-256，由验收产物生成。Windows 使用 NSIS，macOS 使用 app.tar.gz，Linux 使用 AppImage；DEB 通过系统安装方式升级。

## 实际验收

- Windows 分别以 `/AllUsers` 和 `/CurrentUser` 安装 NSIS 到隔离目录（系统级使用 Program Files 下的独立目录），核对 HKLM/HKCU 登记、启动、官方 updater 更高签名候选、实际版本、配置及自启保留，最后卸载。旧用户级基线升级仍保留 HKCU 范围；交互 UAC 和其他管理员凭据需另外人工验收。
- Linux 验 DEB 包内程序和 AppImage；验证 AppImage 官方更新，并用系统包管理安装/升级 DEB。
- macOS 挂载 DMG及解包 app 归档；复制到可写位置后经官方 updater 更新，确认版本与配置。
- 五个平台下载固定官方 sing-box 1.14.4，核对上游摘要，验证 SOCKS5 回环、启停和端口释放，以及生产用例驱动的受管准备/采用、架构/版本/配置检查和篡改拒绝；同源托管集成覆盖首次自动下载、更新、取消、一次回退及恢复。核心仅测试使用，不进入壳包。

0.6 实际包验证已发布签名 v0.5.4 → 本版的原生 updater，确认旧 schema 4 原字节保留、阻止隐式恢复并要求显式初始化；不再要求旧数据迁移。本代 → 隔离高版本候选继续验证设置、恢复和安装范围保留。

0.6.1 起同时验证 v0.6.0 → 本版保持 schema 5。旧受管核心样本额外覆盖升级后显式初始化、以及本代缺少安装记录但保留程序和准备材料的恢复；通过实际安装包调用生产恢复用例，核对备份、核心字节、真实启动/停止和不自动连接。外部核心升级样本继续保留。

测试源、测试公钥及升级夹具由 test-fixtures 限定、不发行。生产包的隔离 smoke 入口只在显式参数/环境和已提权条件下执行同一网络 worker，不接受任意管理员核心或配置路径。普通 smoke 禁止生产调度，核对实际版本、目标、前端及 IPC；新包能启动不等于升级已通过。

发行记录保留组件版本、原包源码、原始 run、Release 链接、实际通过/跳过范围，并将原始摘要报告迁入 `docs/releases/`，不能只依赖 30 天 Artifact。v0.1.0 无 updater，首个签名基线需手动安装，不以旧版手工覆盖作为在线更新验收。

## 登录项与卸载

Windows 安装器支持所有用户和当前用户，有管理员权限的新安装默认所有用户；普通用户通过管理员凭据安装时应选择所有用户。设置与自启继续属于各自用户。升级固定原范围和路径，不自动迁移；更换范围前先卸载旧安装并保留应用数据，具体行为见[桌面与启动](features/desktop.md)。

Windows 当前用户 Run 登记，保留 StartupApproved 的系统禁用状态。NSIS /UPDATE 保留登记；标准卸载只删除指向当前安装位置的 Sing Shell 值。Linux 登记 XDG autostart，DEB 使用稳定程序位置，AppImage 使用外层 APPIMAGE。macOS 13+ 使用 SMAppService.mainApp，较早系统使用无 KeepAlive 的私有 LaunchAgent；完整 bundle、安装位置与图标均需核对。macOS 新直接依赖仅为已有 Tauri 绑定 objc2 / objc2-foundation，复用其锁定版本。

移动程序后需重新登记。macOS/AppImage 直接删除没有可靠卸载回调，删除前请先关闭登录自启。真实注销/登录及系统审批不能由包启动、模拟参数或状态快照替代。

## TUN 资源与授权边界

Windows 使用 UAC 启动同一程序的受限网络 worker，Linux 使用系统 pkexec/polkit；macOS 由系统管理员授权弹窗启动同一程序的短期 worker，断开后退出。macOS 不登记新的 SMAppService 网络服务；包内旧网络 plist 仅用于迁移撤销。应用仍为 ad-hoc 签名、未公证，正式发行证据分别记录系统授权、身份认证、真实 TUN 及未执行范围。

macOS GUI 从 Applications 内的完整安装请求按需网络授权，helper 与 GUI 必须来自同一安装且运行相同构建。升级前先断开、收束 worker 与临时 DNS，再由官方 updater 替换 bundle。协议不兼容或旧副本身份失败不会自动注销/登记系统服务；系统授权、身份和网络就绪错误分别呈现。

每个实际安装包运行生产网络 worker 集成，确认独立官方核心、加密 TCP/UDP IPv4/IPv6 转发和停止后接口释放。包内规则资源包含 SRS、可编辑 JSON、manifest、上游 LICENSE、完整 GPL / CC-BY-SA 许可与 NOTICE；下载的核心不进入应用包。

卸载前停止连接并真正退出，macOS 当前不保留网络 helper 登记。授权目录保留已校验核心与设置外的残留记录，不因卸载猜测删除；确认无连接后管理员可移除 ProgramData/Sing Shell Network、/var/lib/sing-shell-network 或 /Library/Application Support/Sing Shell Network。普通应用更新不修改外部核心。

## 已验证核心渠道

手动运行 core-material.yml 并指定官方 1.14.x 版本：先从官方 API 核对五目标归档及原生程序摘要，使用仅测试构建接受的夹具签名执行五原生 TUN、Fake-IP/TLS 和空受保护目录安装验收。所有目标通过后，发布 job 才能读取 CORE_SIGNING_PRIVATE_KEY，签署生产材料并发布 core-v<版本>/material.json 与 core-channel/recommended.json。这些为 prerelease，不能成为应用最新版本；版本材料拒绝改字节，推荐不自动降级。

核心密钥独立于 TAURI_SIGNING_PRIVATE_KEY；仓库只保留公钥和明确的公开测试密钥，生产构建不包含测试密钥验证入口。签名表示 Sing Shell 已核对官方资产，不代表上游自行提供签名。运行端只需内置公钥，不接受用户公钥、PAT 或任意仓库。密钥轮换通过应用发行完成。
