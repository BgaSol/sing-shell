# 首次使用

1. 从 [sing-box 官方 Releases](https://github.com/SagerNet/sing-box/releases) 获取本机对应的命令行核心并解压。选择普通的 `sing-box-<版本>-<系统>-<架构>` 包，不是 SFW / SFM 等官方图形客户端。
2. 打开 Sing Shell，点击“选择核心”，选择 `sing-box.exe`（Windows）或 `sing-box`（Linux / macOS）。Unix 系统需有执行权限。
3. 点击“导入配置”，选择现有 JSON / JSONC 文件。导入会通过核心校验；失败原因显示在日志中。
4. 点击“启动 sing-box”，观察运行状态及日志；点击“停止”结束进程。

可使用 [`examples/direct.json`](../examples/direct.json) 测试直连模式：它在 `127.0.0.1:2080` 提供 HTTP / SOCKS5 入口，不改变系统代理。此例没有远程代理节点。

详细行为及限制见[核心与配置](features/core.md)。
