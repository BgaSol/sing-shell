# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

需要在桌面上选择单个代理节点、启动 TUN 并设置直连例外的用户。常用流程不要求编辑 sing-box JSON。

## Product Purpose

Sing Shell 是 Tauri 桌面应用，管理官方 sing-box 核心、节点、TUN 连接和域名/IP 分流。未命中规则默认代理；用户策略依列表顺序首次匹配，一条规则可包含多个域名和 IP。

## Operating Context

Windows x86_64/ARM64、Linux x86_64/ARM64、macOS Apple Silicon。macOS 通过系统管理员授权启动短期 worker，不登记持久网络服务。关闭窗口继续运行，真正退出停止连接并清理资源。

## Capabilities and Constraints

一套运行会话、一个当前节点、全局分流。保存与应用分开；凭据和日志保留本机。保留外部核心和原始配置兼容，禁止自动覆盖外部程序。首版不做订阅自动更新、节点测速、策略组或云同步。

## Brand Commitments

保留现有贝壳标识、中文操作语言和 DESIGN.md 中的暖色平面视觉；系统字体、原生 DOM，不引入前端框架。

## Evidence on Hand

产品行为见 docs/features/nodes.md、routing.md 和 core.md；实际平台验证及跳过范围以 docs/releases/ 的对应发行记录为准。

## Product Principles

- 用户操作节点与分流意图，应用生成配置。
- 当前实际运行与下次期望选择分别呈现。
- 系统权限按需取得，退出恢复网络。
- 错误就近呈现并给出可执行的下一步。
