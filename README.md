# Sing Shell

sing-box 的轻量桌面壳，采用与 [SoDeck](https://github.com/BgaSol/soDeck) 相同的 Tauri 2 / Rust + TypeScript / Vite 技术架构。

- 选择本机 sing-box 程序，导入并校验 JSON / JSONC 配置。
- 启动、停止核心，查看运行状态和最近日志。
- 保存核心和配置文件位置；关闭应用时停止受管进程。
- 每日检查应用和受管官方核心的新版本，手动下载并重启应用；外部核心始终手动管理。

支持 Windows x86_64 / ARM64、Linux x86_64 / ARM64 和 macOS Apple Silicon，各平台验证范围见[开发文档](docs/development.md)。

## 安装

在 [GitHub Latest Release](https://github.com/BgaSol/sing-shell/releases/latest) 下载对应平台的 Windows 安装器、Linux DEB / AppImage 或 macOS DMG。安装包仅包含桌面壳，sing-box 核心单独获取。首次使用及系统安装提示见[使用指南](docs/usage.md)和[分发说明](docs/distribution.md)。

## 开发运行

安装 Node.js（版本见 `.node-version`）、Rust（版本见 `rust-toolchain.toml`）和 [Tauri 系统依赖](https://v2.tauri.app/start/prerequisites/)，然后运行：

```sh
npm ci
npm run dev
```

首次运行选择从 [sing-box 官方 Releases](https://github.com/SagerNet/sing-box/releases) 下载并解压的、与本机系统及架构匹配的 `sing-box` / `sing-box.exe`。再导入自己的配置，或使用 [`examples/direct.json`](examples/direct.json) 测试本机直连代理。

当前安装包仅包含桌面壳，sing-box 核心单独获取。[文档导航](docs/README.md)包含使用方式、架构、开发与分发说明。
