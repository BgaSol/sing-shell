# 托盘呈现与 TUN 授权生命周期整改方案

状态：实施中，等待最终五原生/实际包及用户 Mac 验收。2026-11-26 基于 `c151dd15295b0077e27f9b7bade9b572fa89c62a` 核对。起因是托盘标识异常，以及 macOS 首次使用 TUN 时提示“网络控制端与已授权程序身份不符”。用户刚升级应用，但此前没有使用 TUN，不能据此认定是旧 helper 残留。

本方案解决同一条链路上的问题：用户操作 → 会话状态 → 系统授权与对端认证 → 核心运行 → 停止/退出/更新 → 页面和托盘呈现。允许在这些职责内重构，不重写客户端、不建立通用后台框架。用户已授权实施及最终发布 Latest；随后明确先完成代码/CI，稍后提供 Mac 实际验收。验收前保留候选草稿，不发布 Latest。

实施进度：协议 v2 已采用 Ready/Confirm/Accepted；macOS 运行 audit token/CDHash 及动态签名核验、共享 bundle 检查、授权阶段 DTO、控制线程/worker 所有权、只读采样/轮询、节点恢复、托盘投影和 1×/2×资源已落地。macOS ARM64 原生 `465c031` 的继承 socket 身份回归和隔离真实 launchd 激活测试通过，用户 SMAppService/真实 TUN 尚待执行。停止阶段按原有资源停止上限校准为 55 秒，完整 disconnect 等待 65 秒，接纳收尾外层 125 秒；调用超时仍保留所有者。

固定实现 `1b76707` 的五原生 Check、网络集成、签名原包/实际安装升级均已通过；原包及 16 个草稿附件摘要已核对，证据见 [v0.5.2 候选记录](../releases/v0.5.2.md)。剩余仅用户 Mac 的系统允许后真实 TUN/断开退出及实际托盘视觉验收，验收完成后更新正式证据、发布同一草稿并删除本文。期间 Latest 保持 v0.5.1，不将自动化通过冒充人工允许通过。

## 1. 判断：设计缺口与程序错误同时存在

| 发现 | 性质与证据强度 | 整改结论 |
| --- | --- | --- |
| 托盘由 `desktop::icon` 手画蓝色圆环、实心圆和竖条，未使用贝壳标识，无抗锯齿 | 已确认的资源设计缺口；固定 32 像素，未覆盖小尺寸和平台呈现 | 建立少量可审查的内置托盘资源，保留品牌主体及文字状态 |
| 初建托盘设置 template，刷新调用 `set_icon` | 已确认的 macOS API 使用错误；锁定的 tray-icon 0.25.1 在该调用中传入 `false` | 图像及模板属性一次更新，并只在图像状态变化时更新 |
| `Tray.projection` 仅含文案及两个动作布尔值，实际图像还依赖 PID/error | 已确认的缓存输入不完整 | 缓存完整的类型化呈现，不以文案代替状态身份 |
| `connect` 完成后立即读 `LOCAL_PEERPID` 并散列其文件 | 已确认的连接设计缺口；launchd socket 连接完成不保证 daemon 已 accept | 以 helper 主动就绪及双向认证建立会话，不增加固定 sleep 或放宽校验 |
| 此次 macOS 错误是否恰好因读到 launchd 而产生 | 高可信假设，尚无用户机器上的 PID/阶段证据 | 先用原生延迟 accept 场景复现；同时排除旧 helper、不同安装副本和签名变化 |
| `same_executable` 比较当前路径上的磁盘字节 | 已确认的身份模型限制；无法证明进程正在执行的映像，更新替换相同路径后尤其不能据此判定新旧程序相同 | macOS 认证运行进程的代码身份，路径用于安装归属，应用版本只用于兼容判断 |
| `ensure_registered` 将 enabled 直接当作可连接；错误统一成重装提示，丢掉 NSError | 已确认的服务状态和就绪状态混用 | 分开系统登记、激活、认证、协议和资源状态，并保留有界错误原因 |
| `Tunnel::close` 先 take Client，超时直接返回 | 已确认的所有权错误；JoinHandle 随局部值丢弃，线程仍可存活而不再被 Tunnel 管理 | 未完成时保留所有者，禁止新会话和更新交接，允许对同一所有者重试收尾 |
| `Tunnel::stop` 只停核心，`close` 才结束通道 | 当前内部应用/回退会复用通道，不能简单当成“用户断开必然泄漏”的证据 | 明确区分内部停止核心和用户断开，保留内部复用的必要性 |
| `Poll` 遇 cleanupRequired 会执行 `worker.stop`；Worker::snapshot 还写状态 | 已确认的查询与副作用耦合 | 状态采集只读；实际清理由显式请求或 worker 自身的退出事件触发 |
| Session::snapshot_for 忽略传入 `_r`，重读多个快照，外层 IPC 又使用另一次 runtime | 已确认的一次响应内部不一致风险 | 每次响应只采集一份事实，运行页、动作和托盘从同一份会话视图派生 |
| `restore_tick` 在节点模式授权通过后仍落入 config_path 必需判断 | 已确认的模式条件错误；纯节点设置通常没有原始配置路径 | 恢复前置条件按模式分支，TUN 恢复仍只尝试一次且不重新弹授权 |
| macOS CI 以 root 路径启动 network worker，SMAppService 探针另用 Swift 程序 | 已确认的测试边界缺口；实际包网络测试也以 sudo 进入 direct_connect | 分开 worker、launchd 通道、SMAppService 用户授权三种证据，不能互相替代 |

“已确认”指代码及 API 语义可确定，并不表示已在五个平台复现。此次用户报错仍需原生诊断闭环，不将假设写成已证实的用户机器根因。

## 2. 已追踪的入口及影响范围

| 链路 | 当前实现 | 必须到达的整改点 |
| --- | --- | --- |
| 页面/托盘启动、停止、应用更改 | [SessionService](../../src-tauri/src/session/mod.rs) → [Tunnel](../../src-tauri/src/session/tunnel.rs) | 一个会话所有者、一个认证入口；成功保存及取消代次继续由 SessionService 决定 |
| macOS 普通用户连接 | [authorization::connect](../../src-tauri/src/platform/authorization.rs) → SMAppService → 全局 Unix socket → `verify` | bundle 前置检查、激活等待、helper 就绪、运行进程认证、协议确认 |
| helper 入口 | [main::main](../../src-tauri/src/main.rs) → `tunnel::dispatch` → prepare_root/Lease → daemon accept → Worker | 拒绝未认证连接不能占住机器租约；有效会话持有租约直至真实清理完成 |
| Windows / Linux 授权 | 同一 authorization 模块的 UAC/管道或 pkexec/私有 socket | 保留平台权限及 PID 校验；共享协议、取消和子进程所有权变更必须一并回归 |
| 运行和异常退出 | Worker → [RuntimeService](../../src-tauri/src/runtime.rs) / [platform::network](../../src-tauri/src/platform/network.rs) | 保留真实 Child、官方核心校验、接口观测及残留身份核对，不另造进程管理器 |
| 退出及安装 | [lifecycle::finish](../../src-tauri/src/lifecycle.rs) → Admission.wait → Session::stop_internal → updater | 完成包括 helper 在内的收尾之后才能 handed_off/install；失败保留可重试所有者 |
| 恢复 | Session::restore_tick → start_tunnel_owned | enabled 只是允许尝试的前置条件；仍要完整认证、观察和确认，不能依赖 config_path |
| 托盘 | main 的原生 1 秒循环 → [Desktop::refresh](../../src-tauri/src/desktop.rs) | 读取紧凑会话视图；主线程仅应用变化；失败保留可返回窗口 |
| 页面 | core_snapshot → [src/core.ts](../../src/core.ts) / [src/runtime.ts](../../src/runtime.ts) | 明确授权阶段和错误操作建议，不根据错误文字猜行为 |
| 构建及验证 | [network-integration](../../scripts/network-integration.mjs)、[macOS 探针](../../scripts/macos-service-preflight.mjs)、[verify-package](../../scripts/verify-package.mjs)、[CI](../../.github/workflows/network-preflight.yml) | 使用生产 Rust 认证与实际包；故障用例独立，不伪装成真实用户审批 |

现有平台边界、Admission、SettingsStore、核心下载校验和进程组继续复用。不要把平台提权、桌面呈现或系统登记放进 RuntimeService，也不要让 Desktop 直接操作 Tunnel/Child。

## 3. 可验收合同及非目标

1. 正常停止、运行和异常时，用户均能辨认 Sing Shell 的托盘入口；浅/深菜单栏及常用 DPI 清晰，状态始终有文字。TUN 未确认接口和路由前，不能仅因有 PID 显示“已连接”。
2. macOS 未允许时明确引导到系统设置；已允许时能完成冷启动和后续重连。socket 建立不代表认证完成，登记 enabled 不代表 helper 就绪。
3. GUI 和 helper 必须是同一安装、同一构建的程序，并完成协议确认，才可传节点凭据、配置参数、日志或执行网络操作。错误副本、旧程序、伪造/不完整握手一律拒绝。
4. 用户停止可打断授权等待及准备；取消之后不能迟到启动核心或提交成功记录。网络清理完成后才可显示已断开。缺少清理证据时保留真实 PID/阶段和错误。
5. 用户断开/退出结束本次控制会话及按需 worker；macOS 允许保留系统登记和被 launchd 管理的监听端点，不要求每次断开注销授权。内部应用更改/一次回退可短暂复用同一已认证会话。
6. helper 或 GUI 失联触发有界清理，不恢复旧节点、不循环申请权限。再次连接只可由明确用户动作或本次启动的一次恢复发起。
7. 更新先停止 TUN、确认资源及 helper 收尾，再替换应用；旧构建不能因磁盘路径已被替换就被当作新构建通过。失败不覆盖有效设置，不删除用户节点或授权核心缓存。
8. 快照采集及 Poll 只读；同一次响应的 PID、phase、按钮和文字来自同一份采样结果。

本轮不增加系统代理、订阅、测速、策略组、通知中心、通用 daemon、自动修复循环或崩溃重连；不迁移设置 schema，不改变外部核心，不升级依赖，不引入新的生产依赖。macOS 最低 TUN 版本仍是 13；其他系统的产品边界不变。Developer ID、公证和长期发布者信任不由这次身份修复代替，也不作为偷偷新增的发行基础设施。

## 4. 授权连接：从字节通道到可信会话

### 4.1 保留 Unix socket，补齐明确的建立协议

继续使用当前 SMAppService、BundleProgram 和 socket activation，不为此次问题改成 XPC、另一个 helper 二进制或常驻通用服务。采用 worker 主动就绪的短握手，消除 connect/accept 之间的身份竞争。

```text
GUI                                launchd / helper
读取并核对系统登记、当前 bundle
connect --------------------------> launchd 排队并按需激活
等候有界的就绪帧                    helper accept
                                    认证 GUI 的系统凭据及运行代码
                                    取得机器租约、准备受保护资源上下文
        <-------------------------- Ready(protocol, sessionNonce)
重新读取对端凭据并认证 helper
Confirm(protocol, sessionNonce) ---> 核对本次挑战及协议
        <-------------------------- Accepted(sessionNonce)
至此返回 AuthenticatedChannel
Start(闭合节点及官方核心版本) ------> 校验 → 启动 → 观察
```

- Ready 只在 helper 接管后发出，因此 GUI 读取 Ready 后再核对对端身份。系统凭据不能用帧内自报 PID/UID/路径替代。Ready 不是认证结论。
- Ready/Confirm/Accepted 使用确定格式，单帧不超过 1 KiB；nonce 复用现有随机数来源。未认证阶段不返回日志、节点、文件路径或配置。随机挑战只绑定本次会话，不能代替 OS 身份校验。
- 使用明确的新控制协议版本；Start/Poll/Stop/Quit 保持闭合命令和 2 MiB 上限。删除旧 Hello 的重复初始化分支。新旧协议不能混跑，不做认证失败后的旧协议降级。
- GUI 的平台连接先返回仅供内部握手使用的 PendingChannel；只有建立成功后创建私有 AuthenticatedChannel，业务读写只接受后者。该封装限定在网络控制模块，不做全项目通信框架。
- daemon 接受无效客户端后关闭该连接，在同一次有时限的激活窗口内继续接受；不因一次陌生连接占住机器租约或立刻终止整个服务。一次只处理一个待认证连接，每个连接有短时限，整体窗口不因恶意重试无限延长。
- 机器租约在客户端身份通过后取得，在完整清理后释放；Start 之前仍重新验证输入及官方核心。忙状态返回固定错误，不能抢占已有会话。
- 所有 read、write、连接等待和等待确认均受同一个阶段截止时间及取消标志控制。收到 Ready 后、发送 Start 前再次检查取消，不能依赖排在 Start 后面的 Stop 来撤销一个未执行的请求。

### 4.2 macOS 认证运行中的代码，而非重新读同名文件

生产 macOS 使用系统 Security.framework 和现有 objc2/Foundation 绑定，直接 FFI 的不安全代码仅在平台模块内；优先复用已有可用绑定，不为几处调用新增 crate。

认证条件同时成立：

- 从内核取得连接对端身份；Ready 后通过 `LOCAL_PEERTOKEN` 取得 audit token，校验长度并用 `SecCodeCopyGuestWithAttributes(kSecGuestAttributeAudit)` 绑定运行进程，避免仅按一个可复用 PID 查文件。
- 用 `SecCodeCheckValidity` 检查运行代码；从 self/peer 代码签名信息比较 identifier 及精确构建身份（CDHash/unique），拒绝无签名、缺字段、失效或构建不同。当前 ad-hoc 签名可以表达同一构建，不被描述为发行商认证。
- 核对规范化后的安装可执行路径，必须归属同一份完整 app。不同目录中的同字节副本不能默认为当前获授权安装。
- GUI 侧要求 helper 的有效 UID 为 root；helper 侧校验实际 GUI 代码和安装归属。不会因为 UID 为 root、bundle ID 相同或版本文字相同就跳过代码身份。
- 更新时同时核对运行映像和当前安装的签名材料；旧进程与新磁盘内容不一致应失败并要求重启正确实例。路径与静态摘要只做归属/完整性辅助，不能作为运行身份的唯一依据。

运行 CDHash 与磁盘签名信息须明确区分：先验证动态 SecCode；若所用信息接口实际返回静态代码材料，使用系统 `csops_audittoken(CS_OPS_CDHASH)` 等受 audit token 约束的内核查询确认运行映像，不能给磁盘散列换一个签名 API 名称后继续保留同一缺陷。self 的身份也取自正在运行的进程，不从每次调用时的 current_exe 文件重新推断。

原生可行性验证先确认 macOS 13 及当前支持系统对 audit token、ad-hoc 代码验证和运行映像替换的实际行为。如某 API 不能证明旧/新运行映像有别，必须在该阶段解决，并记录确切系统行为；禁止静默退回“磁盘散列一样就允许”。不硬编码不存在的 Team ID，也不因没有 Developer ID 放宽为任意同名程序。

Windows/Linux 保留现有 OS 凭据、提权及相同程序核验，纳入共享握手和进程收尾回归；不趁此扩成跨平台签名体系。macOS 的普通 GUI 入口不再因 elevated 自动选择测试用 direct_connect；已有可丢弃 root fixture 明确走隔离入口，不能让 root 夹具覆盖率被算成普通用户 launchd 路径通过。

### 4.3 登记状态、安装前置条件和故障诊断

将 SMAppService 状态映射为固定枚举：Unsupported / NotRegistered / RequiresApproval / Enabled / NotFound / Error。不要在 settings.json 再保存一个 enabled。登录自启的 mainAppService 与网络 daemon 的登记状态继续独立。

网络授权前复用或抽出当前登录项已有的 bundle/安装检查：合法 bundle ID、同一个可执行文件、包内 daemon plist 及 BundleProgram、稳定安装位置。允许 `/Applications` 和 `~/Applications` 下的完整 app，拒绝 DMG、AppTranslocation、临时目录和裸开发二进制。抽取仅包含路径及身份的 macOS 小模块，登录项自己的图标检查仍归登录项；不要让网络代码调用“修改登录项”的用例。

Enabled 仅进入激活握手；RequiresApproval 只提示用户允许；读取状态和自动恢复不调用 register。用户手动首次连接才在合法安装中尝试登记。注册失败保存脱敏 NSError domain/code/描述及操作阶段，不统一归因为“请重装”。

错误使用小型结构：阶段（registration/activation/authentication/protocol/cleanup）＋固定原因码＋用户可读说明＋允许的操作。至少区分等待授权、未就绪、对端身份不符、协议过旧、机器资源被占用、取消、超时和清理未完成。页面和托盘用固定原因码选择行为，不解析错误字符串。诊断限于本机、有界、按文本展示；不输出节点凭据、配置或完整日志包，不自动上传。

不在失败后循环 unregister/register，不自动删除 socket 或其他安装。默认复用既有 Enabled 登记。旧服务或位置不符时，提示退出其他副本并从当前安装重开及检查系统设置；只有原生同位置升级测试证明必须重新登记，才在本主题内增加一次明确、可失败的本服务重新登记操作。该操作必须先确认没有活动资源所有者，保留用户禁用状态，读回实际结果，不能强行终止未知身份进程。未能证明安全归属时保留状态并报错，不靠“修复”绕过认证。

## 5. 会话所有权、取消与收尾

### 5.1 保留一个控制线程，收拢状态写入

沿用当前 mpsc＋一个控制线程，不引入 actor 库、新运行时或通用事件总线。控制线程独占通道和控制阶段；worker 独占其 RuntimeService、启动任务和机器租约。调用方提交命令并等待结果，不再把 Snapshot 重置为 Default 来表示“正在处理”。

Client 保留命令发送端、取消标志、完成结果及 JoinHandle。平台返回的通道同时保留需要等待退出的 worker 句柄或经过认证的进程身份：Windows 不提前丢 hProcess，Unix 直接启动的 Child 必须 wait/reap，macOS 用系统进程身份作有界退出确认，不向未知 root 进程发信号。

只在完成结果到达、线程实际结束并 join 后清除 Client。等待超时保留原所有者及最近真实快照；后续停止/退出继续收尾同一对象，不能创建第二个控制线程。读写均有截止时间；不在持有 Client/state Mutex 时阻塞等待，避免停止阻塞状态读取和托盘主线程。

将隧道内部 phase 从任意字符串改为封闭枚举，序列化名称保持清晰；过渡阶段和错误阶段可区分。更改仅限本链路，不要求把整个 RuntimeService、更新器的所有字符串状态一次改完。已认证进程身份/nonce 是本会话上下文，不持久化成另一套 PID 数据库。

### 5.2 明确两种停止

| 操作 | 使用者 | 完成条件 |
| --- | --- | --- |
| 内部 `stop_core` | 应用更改、一次连接恢复或核心回退 | 启动任务停止、真实核心退出、接口释放；保留同一短期认证通道与租约，马上进入同一操作的下一步 |
| `disconnect` | 用户停止、取消首次启动、退出、更新及 Drop 兜底 | 完成 stop_core，结束控制协议、worker 释放租约/临时配置，控制线程退出；正常退出/更新还确认已认证 helper 收尾 |

SessionService 仍负责用户意图、成功保存和一次回退，不在 Tunnel::Drop、worker 停止或平台进程退出时写 resumeCore。内部 stop_core 不单独暴露成 UI 命令。Apply 的准备/失败恢复沿用已有单次规则，结束后不能留下无人管理的空闲通道。

取消须贯穿登记后的等待、握手、消息交换、官方材料准备、启动观察和关闭。取消之后，所有迟到 Ready/Accepted/成功结果均不能触发 Start 或成功提交。沿用现有 intent epoch；隧道只在确有跨会话回调时绑定本次 sessionNonce/代次，不给每个字段另造版本号。

### 5.3 清理与查询分开

- Poll 和 snapshot 只读。worker 在自身运行循环内处理“已知核心退出”及“控制端失联”事件，至多触发一次自动清理，失败保留 cleanupRequired 等待明确停止；不由轮询频率决定重复清理次数。
- 普通 stop/quit、握手后断线、读写超时、GUI 崩溃统一进入同一 worker 收尾函数。它负责取消并收束启动任务、停止核心、核对接口、删除已确认属于本会话的临时配置及残留记录。
- 资源仍可能存在或观察失败时保持 cleanupRequired，禁止另起会话或安装。无 PID 并不等于接口已释放；网络 observe 失败也不能按默认全 false 当作已清理。
- Worker 的启动 JoinHandle 同样不能在未结束时 take 后丢弃；准备取消及下载/校验路径必须能在阶段预算内收束。清理失败保留受保护记录，下一次明确授权只执行核对清理，不恢复旧节点。
- 正常结束增加明确 Closed 结果：仅在核心、接口、配置及租约收尾之后发出。GUI 读取 Closed、EOF 并完成本地线程及平台 worker 退出核对后，才报告完整断开/允许安装。断线没有 Closed 时，通过后续可信清理及实际网络观察确认，不能把 EOF 当成功。
- macOS daemon 在最后一个有效会话结束后退出。系统监听 socket 可继续存在；不加 KeepAlive、RunAtLoad 或无用户会话时反复拉起核心。

### 5.4 时限采用阶段预算

把现有散布的 120/180/35/25 秒梳理为少量具名预算，在等待时向下传剩余时间，不能每层重置完整超时。初始目标为：已有 UAC/polkit 用户交互上限保持 120 秒；macOS 激活和认证合计 15 秒；就绪帧后的每个确认不超过 3 秒；核心材料及启动总预算沿用 180 秒；资源停止预算 35 秒，最终通道与进程结束附加 10 秒。

阶段预算必须覆盖实际下载取消和核心优雅停止的上限。实现时以相关适配器已有上限及原生慢启动实验校准；调整只能改具名预算及相应测试，不能在各调用点叠加 sleep。Admission::wait/runtime wait 与 lifecycle 的外层截止时间必须覆盖内部收尾预算；调用方超时仍保留所有权和禁止重入状态。

## 6. SessionService 采样、恢复与更新交接

### 6.1 一次采样，一份运行事实

删除 Session::snapshot_for 的无效 `_r` 参数和内部重复读取方式。SessionService 采集一次 settings、当前模式对应的 runtime/tunnel 紧凑状态、核心资产状态、restore 和接纳事实，再组装一个 SessionView；IPC 外层和 Desktop 使用该结果中的同一 runtime。

这是一份内部自洽的采样结果，不声称多个服务在全局锁下形成数据库事务。避免同时长时间持有多个锁，采集期间发生的后续变化由下一次刷新呈现。日志单独读取，不因每秒托盘刷新克隆整份节点、日志或重新执行网络/包校验。

SessionView 内明确本次模式、阶段、PID、清理状态、授权问题及能否启动/停止。页面保持必要的显示差异，例如 TUN 与原始配置名称，但都依据这些事实；不再由 Desktop 和页面分别拼“PID 大于零即运行”。维护失败属于资产问题，可提示，不能覆盖仍运行的核心事实。

### 6.2 恢复只验证当前模式的必要条件

节点模式检查：已保存的 lastConnection、官方受管核心可用、恢复意图、安全初始化结论、系统授权已允许。原始配置模式才检查 config_path/pending_config_path。已允许仍需本次完整认证，不把 already_allowed 当作认证缓存。

等待批准、身份不符或协议不符会消费本次恢复请求并给出原因，不自动登记、不弹权限重试、不延时循环连接。主动停止的 intent epoch 必须胜过正在建立的认证或启动成功；手动启动、托盘启动、恢复和一次回退继续走同一成功提交。

### 6.3 生命周期保留一条退出路径

保持 `预检草稿 → 封闭 Admission/取消 → 等待操作所有者 → 内部 disconnect → 停止维护 → handed_off → updater/退出`。本次只修正等待预算、所有权保留和 helper 收尾条件，不新增第二个退出协调器。

窗口隐藏不触发授权注销、停止或清理。更新前无需每次注销 SMAppService；必须先确认当前会话及 helper 已结束，再由官方 updater 替换 bundle。更新失败重新开放接纳，保留真实失败阶段和资源所有者，不假称已停止，也不自动恢复旧连接。

同位置升级、应用移动、旧副本仍在运行分别验收。不得通过混用新旧协议或仅比较版本号接受旧 helper；如果系统要求重新批准，明确呈现实际状态。原始配置、用户节点、恢复偏好和独立 updater 签名不做无关迁移。

## 7. 托盘：稳定标识、完整投影和平台资源

从现有 `assets/icon.svg` 的贝壳轮廓提炼托盘专用图稿，保留可识别主体。Windows/Linux 使用彩色或高对比贝壳，停止为主体、运行/异常用小形状标记；macOS 使用透明背景单色贝壳及有区别的形状标记，由系统 template 适配浅/深背景。不要直接把整块粉色应用图标压成不可辨认的小方块，也不只靠颜色传状态。

资源以少量源码 SVG 和已生成的 PNG 内置提交，覆盖实际托盘小尺寸及 2×，生成时抗锯齿。使用现有 Tauri 图像能力；如果需要 PNG 解码，优先 Tauri 现有能力或 compile-time include_image，不额外引入图像库。不得运行时联网取图或在授权进程中生成品牌资源。构建和资源引用需可重复验证。

引入具体的 `TrayProjection { status, action, enabled, icon_state, tooltip }`，icon_state 使用有限的 Idle / Busy / Running / Attention。可见的贝壳主体始终保留；Busy 不增加动画或新的加载服务。图像、文本及动作分别比较变化，避免一次按钮变化重复替换同一图像。创建时立即使用当前完整投影，不能先画已停止再等首个定时刷新纠正。

优先级为：清理未完成 → 正在停止 → 授权/准备/启动 → 已确认运行 → 等待恢复 → 停止/需处理。TUN 只有 worker 的运行阶段和接口观察已确认才显示已连接；PID 可保留在异常详情中。运行中的核心维护错误仍显示运行，文字附有问题入口。普通成功和普通维护不抢焦点。

macOS 使用已锁定 Tauri 的 `set_icon_with_as_template` 原子设置图像和 template，创建与每次更新规则一致。保留现有托盘失败退回可见窗口、Windows 左击返回及右击菜单、macOS 菜单交互、Linux 最小化窗口入口，不扩大窗口行为范围。

## 8. 文件与重构边界

| 位置 | 目标改动 | 约束 |
| --- | --- | --- |
| `platform/authorization.rs` 及新增 `platform/authorization/macos.rs` | 薄平台入口，macOS 服务、peer 身份和进程退出核对；保留直接启动 worker 所有权 | 不按美化目的拆完 Windows/Linux；同文件内可复用 Unix 小函数 |
| 必要的 `platform/macos_bundle.rs` | 抽出登录项/网络共同需要的 bundle 身份与稳定安装检查 | 仅实际复用部分；图标和登录项策略不强行通用化 |
| `session/tunnel.rs` | 保留公开入口、Client 所有权、业务会话及取消/断开 | 不加第二个调度器 |
| 新 `session/tunnel/protocol.rs` | 闭合消息、有限 phase/错误、帧上限、建立协议及 AuthenticatedChannel | 无任意 RPC、插件、命令执行或跨版本协商框架 |
| 新 `session/tunnel/worker.rs` | 从原文件移动 Worker、受保护核心准备及 serve/收尾 | 保留全部官方校验、秘密脱敏和回退限制；不复制实现 |
| `session/mod.rs`、`main.rs` | 一次 SessionView 采样、恢复条件、IPC 同步 | 不重写 CoreService/SettingsStore |
| `lifecycle.rs`、必要的 `app.rs` | 收尾预算、等待及失败保留；沿用现有协调器 | 不扩大安装器职责 |
| `desktop.rs`、图稿及 `src-tauri/icons/` 的托盘资源 | 完整投影与平台资源 | 不重画应用主图标，不全站改版 |
| `src/core.ts`、`src/runtime.ts` 及必要授权提示 | 显示结构化阶段、问题和正确操作 | 仍为原生 DOM，不新增 UI 框架 |
| 原生测试、现有 scripts 和 workflows | 生产 Rust 建立链路、清理/更新及视觉验收 | 不使用假 helper 成功替代实际包 |

代码移动保留当前公开 `session::tunnel::Connection`、fixture/dispatch 等入口或在同次提交更新所有调用；避免长期维护两套协议/停止路径。新增文件因职责而存在，不为每个类型单开目录。预计主体限于上述模块；发现与问题链路无关的缺陷单独记录，不顺手重构节点解析、路由、下载器或登录项全套实现。

## 9. 验证方案与阻断条件

### 9.1 先复现和确认平台事实

在可丢弃 macOS ARM64 原生环境运行生产 Rust 通道测试：父进程预先监听、子进程延迟接管，记录 Ready 前后 OS 返回的对端身份；再用真实 launchd socket activation 重复。前者验证竞争及认证时序，后者验证系统交接，均不把测试改成接受 launchd 通过。

用已签名实际 app 的普通用户 GUI 及注册的 daemon 验证最短闭环：系统允许 → 冷启动连接 → 运行身份及协议通过 → 停止并真实退出。收集系统版本、架构、构建、服务状态、失败阶段、对端 PID/运行代码身份、当前安装归属，不收集节点凭据。用户机器只有收到相同证据后，才可把报错最终归因于竞争、旧实例或安装变化。

### 9.2 必需回归矩阵

| 层次 | 必须覆盖的行为 |
| --- | --- |
| 建立与帧 | helper 延迟 accept/Ready；从未 Ready；半帧/超限/未知版本/nonce 不符；对端不读导致写超时；取消前后竞态；未认证前没有 Start/节点/日志 |
| 进程身份 | 正确程序通过；伪造 PID/自报版本无效；错误程序、同 bundle ID 不同构建、同构建不同安装、签名失效、进程退出/PID 复用、旧进程路径被新文件替换均拒绝 |
| 登记与安装 | 未登记、等待允许、拒绝、允许后冷启动、NotFound、NSError 保留；DMG/临时 app 拒绝；稳定安装允许；enabled 不掩盖激活失败；不改用户主应用自启 |
| 会话与取消 | 每个准备阶段取消；Start 排队后停止；重复 stop/quit；旧结果不覆盖新意图；停止超时仍保留同一 Client/JoinHandle；重试不产生第二 worker；断开后没有控制轮询/按需 helper |
| 清理 | Stop/EOF/read 超时/write 超时/GUI 崩溃/核心崩溃/worker 崩溃；读快照/Poll 不触发清理；接口观察失败不能假成功；清理失败保留记录并阻止重入；完成后核对接口、路由和核心退出 |
| 内部应用和回退 | stop_core 保留同一短期通道，候选失败只恢复一次旧连接；用户停止/退出打断后不回退；操作结束仍由一个 Client 管理 |
| 快照与呈现 | 同次 IPC 的 runtime 与 SessionView 一致；全部 phase 的按钮/文案/图像；有 PID 但 cleanupRequired、启动中有 PID、运行中维护失败、无 PID 恢复等待等组合 |
| 恢复 | 没有 config_path 的纯节点恢复；原始配置缺失仍拒绝；权限未允许不登记；已允许但认证失败仅消费一次；停止胜过迟到成功 |
| 生命周期和升级 | 用户断开、窗口隐藏、退出、更新各自行为；停止失败不 install、不 handed_off；正常确认 helper 结束后替换；同位置正式升级及新进程首次 TUN；不同位置/旧副本拒绝与有效操作建议 |
| 隔离与兼容 | 机器级第二会话拒绝；未认证连接不消耗租约；Windows/Linux 原有权限和进程组；现有 schema/外部核心/节点凭据和恢复偏好保持 |

协议和会话测试使用可控 stream、进程及故障注入验证行为，不写源码正则匹配当逻辑测试。注入入口限制为测试构建/夹具，不给生产认证增加 allowUnsafe、假 root、跳签名或环境变量降级开关。

### 9.3 原生和实际包证据必须分开

- 第一层：无提权的协议/状态回归；第二层：可丢弃提权环境中真实 worker、官方 sing-box、加密 TCP/UDP IPv4/IPv6 与清理；第三层：macOS 普通用户、实际包、SMAppService 允许后的 launchd 激活和真实 TUN。每层单独报告 passed/failed/skipped。
- 在 macOS CI 补充生产 Rust 的 launchd socket activation 认证回归。允许在可丢弃 runner 用隔离 job 做确定性注入，但此项只证明 launchd 机制，不能被命名成用户 SMAppService 审批通过。
- 现有 Swift preflight 若仍用于判断系统审批可用性，缩小为这一职责；不再为认证或生产 helper 实现第二份 Swift 逻辑。能被生产 Rust 实际包探针替代的部分删除。
- 真实允许提示无法自动完成时，保留人工步骤与原生机器证据。macOS 本次缺陷的修复完成/正式发布必须具有实际普通用户激活证据；不能继续仅跳过此项同时声称本问题已修好。
- 五个目标分别跑原生检查及实际包启停。Windows ARM64 / Linux ARM64 不使用交叉编译或 x64 执行代替；当前 Windows 本机验证不代表 macOS 通过。
- 托盘另做实际桌面视觉验收：Windows 常用 100%/150%/200% 缩放和浅/深任务栏；macOS Retina 浅/深菜单栏、首建及多次切换后 template；Linux 实际托盘宿主及缺失时窗口回退。截图包含实际尺寸和放大对照，不以托盘句柄创建成功替代可见性。
- `.github/workflows/network-preflight.yml` 目前 push 限定旧功能分支，需在本主题内调整为正确的 main/PR 相关路径或保持显式 dispatch 的必需检查，避免新增回归没有执行入口。保持标准 GitHub Actions，不新建外部发布系统。

实现时运行相关 `npm run check`、`npm run scripts:test`、`npm run test`、`npm run build`，并完成上述原生缺陷及实际包用例。通过与跳过以实际命令/机器/包来源记录，不将计划中的项目计作通过。

## 10. 实施顺序与每步完成条件

| 顺序/提交主题 | 交付 | 进入下一步前必须确认 |
| --- | --- | --- |
| A：复现与平台身份验证 | Rust 延迟接管/真实 launchd 复现；ad-hoc 动态代码及同路径替换验证；明确此次报错证据采集 | 能稳定区分 launchd、有效 helper、旧映像及错误副本；选定的系统 API 在支持版本可用 |
| B：可信控制会话 | 拆出必要协议及 macOS 适配；Ready/Confirm/Accepted、结构化错误、安装/服务检查 | 不泄露未认证数据；延迟/拒绝/取消/版本不符及其他平台共享握手通过；无旧协议静默降级 |
| C：资源所有权与生命周期 | 收拢 Client/Worker 所有权、内部 stop_core 与 disconnect、只读 Poll、时限及退出交接 | 超时仍可收尾，无迟到启动/重复 worker；故障矩阵及更新前资源阻断通过 |
| D：会话视图与托盘 | 一次采样、恢复模式判断、完整 TrayProjection、专用资源、原子 template 更新 | 页面和托盘状态组合正确；纯节点恢复回归通过；原生小尺寸视觉核对 |
| E：实际包与正式文档 | 真实普通用户 macOS 授权/TUN、五原生目标、实际升级/停止；更新对应合同和证据 | 新发现缺陷关闭，跳过项与风险明确；本问题关键激活路径不能被跳过 |

每个提交只有一个完整主题，重构搬移及其调用/测试同次完成。B/C 可按必要的可构建步骤合并实现，但不留下两套认证或停止路径。A 验证失败先解决平台事实，不继续靠假设铺开结构。

## 11. 文档、复杂度与完成标准

正式行为分别回写 [核心与配置](../features/core.md)（认证、取消、清理和错误）、[桌面与启动](../features/desktop.md)（投影、恢复及退出）、[图标与界面](../features/appearance.md)（托盘资源），更新交接补充 [软件更新](../features/updates.md)。职责和协议归[架构](../architecture.md)，测试层级与未覆盖范围归[开发](../development.md)，实际安装/升级及签名限制归[分发](../distribution.md)。导航只在 [docs/README.md](../README.md) 维护。

旧 `tray-startup-restore` 计划已由 v0.4.0 的正式文档及[发行记录](../releases/v0.4.0.md)承接，且内容仍停留 schema 3/两页阶段。本次删除该过期计划，未完成的托盘资源和实际视觉问题统一由本文管理，不另建完成归档；历史发行证据保持原样。

控制复杂度的约束：一个会话用例入口、一个控制线程、一个 worker 资源所有者、一套闭合协议、一份会话采样；仅增加完成这些职责必需的类型和文件。没有额外系统服务/二进制/数据库/设置迁移/生产依赖，也不把此次修改扩展为通用身份、进程或 UI 框架。

完成必须同时满足：用户可验收行为成立、相关失败回归及原生实际包已执行、macOS 关键激活链路有真实证据、正式文档同步、所有权及旧代码路径清理完毕。实施完成后删除本文，不把方案复制成完成报告。此次方案阶段不代表实现、macOS 复现或新发行验收已经通过。

本轮方案验证：Windows x86_64 执行 `npm run scripts:test`，12 项通过，包含本地文档链接；`git diff --check` 通过。没有应用代码变更，未运行 Rust 测试、应用构建或 macOS 原生/实际包验收。

## 12. 平台语义核对依据

- 锁定的 Tauri 2.12.5 `tray/mod.rs` 提供 `set_icon_with_as_template`；tray-icon 0.25.1 的 macOS `set_icon` 明确以非 template 设置图像。无需升级依赖来修复模板丢失。
- Apple XNU 的 [uipc_usrreq.c](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/uipc_usrreq.c) 中，LOCAL_PEERPID/LOCAL_PEERTOKEN 依据对端 socket 的 last_pid；[uipc_socket2.c](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/uipc_socket2.c) 创建待接收 socket 时继承监听者身份；[uipc_socket.c](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/uipc_socket.c) 的 accept 更新最后所有者。它们说明 connect 后立即查询存在竞争，不能证明用户机器已经实际触发。
- Apple Security 的 [SecCode.h](https://github.com/apple-oss-distributions/Security/blob/main/OSX/libsecurity_codesigning/lib/SecCode.h) 区分动态代码校验、audit token guest 查找和签名信息；具体接口组合仍须通过 A 阶段的原生替换测试，不能以静态文档阅读替代。
- 当前真实验证边界见[开发文档](../development.md)及 [v0.5.1 证据](../releases/v0.5.1-verification.json)：root worker 与用户允许后的 launchd 激活是两项不同证据，后者此前明确未覆盖。
