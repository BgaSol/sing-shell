# 架构

技术栈为 Tauri 2 / Rust、TypeScript 原生 DOM / Vite，不增加生产依赖、数据库、UI 框架、通用任务平台或后台服务。sing-box 是独立子进程，负责协议、配置语义、DNS、路由与转发。

## 所有权与业务链路

- `main.rs` 装配启动、IPC 注册与窗口事件；commands.rs 负责薄命令适配和原生文件框，命令调用用例，不直接编排下载。
- `profile/service.rs` 管理节点导入票据与名称/标签事务；`profile/routing.rs` 是有序闭合策略模型；`profile/config.rs` 将固定输入编译为 sing-box 配置，不自行匹配流量。
- SettingsStore 串行原子写入，节点、分流、核心与下载偏好分别校对修订。分流保存时原子合并来源增删及更新偏好，内容版本不写入用户规则表。
- RuleService 复用 Application 已有后台唤醒循环，一次处理一个固定来源，读取网络偏好快照，下载、核心校验后原子发布不可变版本索引。运行、候选、恢复、包内引用保留，固定目录回收无引用内容。
- SessionService 接纳 `connect(nodeId)`、保存规则后应用、停止和一次恢复，冻结节点参数、有序规则、具体来源及核心版本。托盘、页面和退出使用相同的实际运行事实。
- SessionService 同时接纳只选择节点，统一派生改选、连接、停止能力。选择、连接目标、实际运行和成功恢复依据分开；显式连接保存选择，应用设置针对实际节点，成功确认不反写用户选择。`desktop/tray.rs` 只负责公开状态投影、原生菜单及明确事件，`desktop.rs` 保留窗口、导航和宿主故障回退。
- worker 独占受保护核心、TUN、临时 DNS、候选替换和清理。preparation.rs 冻结输入与本次绕过解析结果，resolver.rs 独占短期 DNS 实例和候选目录。准备在独立可取消任务执行，命令循环保持可响应；候选完整校验后才停旧。新连接失败最多恢复一次旧实际结果；准备失败不停止旧连接，清理失败阻止新 TUN。
- RuntimeService 管理正式核心与校验；runtime/process.rs 共用 Child、reader 和真实退出处理，runtime/control.rs 共用本机鉴权端点及禁代理、禁重定向 HTTP。平台进程组/Job、两种固定用途的残留记录、系统授权和网络适配留在 platform。
- ActivityCollector 跟随 worker 中的真实核心，探测能力后只读采样本机鉴权接口；连接按运行时 tag 清单归因。原始配置不注入监控，不能用展示故障推断网络已停止。
- CoreService / ManagedCore 保留官方材料、来源、版本采用及回退；UpdateService 只负责应用更新，复用官方 Tauri updater，不覆盖外部核心。

核心采用集中在 `core/adoption.rs`，由 CoreService 决定资格并提交选择；ManagedCore 独占单个准备槽及缓存恢复，package 负责字节校验与文件操作。设置中的 active/previous/trial 表达已提交选择，Prepared 保留未完成采用的来源依据；文件发布和选择提交之间允许重启后继续，不增加跨文件事务日志。设置重置由 SessionService 接纳，在现有门禁内备份并重新恢复本地核心状态。

受控替换固定当前核心，不同时试用待更新核心；首次停止状态启动才可采用新核心并最多回退一次。两类用例均最多两次核心启动，不递归叠加连接回退与核心回退。原始配置继续使用普通 RuntimeService，停止后重新校验启动。

## 授权协议与资源释放

控制线程独占通道和系统授权进程所有者，worker 持有机器级租约。握手协议 6 验证同构建、进程/安装身份和随机 nonce，之后只接收闭合节点/规则/官方版本及有界签名程序分块，不接受任意管理员路径、命令、DNS 解析结果或 URL。macOS 用系统管理员授权启动短期 worker，不登记持久网络服务；历史 helper 只按既有安全撤销路径处理。

轻量 Poll 不夹带完整日志或连接。活动、日志按需只读分页；控制队列、HTTP、字段、页数和编码字节均有界。Stop/Quit 取消准备和采集，等待真实子进程及任务结束后释放本次 DNS、核心、TUN、配置目录和通道；超时保持 cleanupRequired 和同一所有者供重试。

受保护配置目录使用随机会话 ID，当前目录单独登记以便崩溃后的清理；目录删除仍限定本应用 runtime 范围。进程残留记录绑定 PID、启动身份与实际映像，不能误杀复用 PID。控制端失联触发清理，核心运行中崩溃不自动重连。

## 状态与前端

四页为节点、分流、活动、设置，设置采用常规/更新/高级页内分类。main.ts 只装配，api.ts/ contracts.ts 分开公开 IPC 与类型，app-state.ts 独占配置读取合并。写中触发刷新会强制再读，旧响应不能覆盖写后结果；轻量会话携带修订，变化才读取配置。活动和日志按当前视图与窗口可见性读取。

分流使用 profile/change.rs 的有限操作，在 SettingsStore 写锁内构造完整候选并原子保存，与来源偏好一起提交；没有页面整表草稿。模态表单一次提交，开关/出口/顺序自动保存；连接入口等待本前端在途网络设置写入。SessionService 的运行指纹是待应用的唯一依据，后台来源版本更新仍独立于 routingRevision。

更新开关分别合并写入，下载网络以已有 network.revision 独立校对，同时递增 preferencesRevision 唤醒配置读取。IPv4 连接选项继续独立 connectionRevision。导入采用单预览槽与 Import 门禁，取消、迟到结果和新弹窗相互隔离，预览携带节点修订。

Modal 管理唯一编辑会话与提交回调，不承载表单 DSL。退出先冻结包含顶部栏在内的状态操作，等待保存，再处理当前编辑；保留 documentId/requestId 预检和 Admission 关闭/等待/释放顺序。收起不丢弃内容，自绘关闭按钮仍触发原生 CloseRequested。

SessionService 的模式转换先检查目标资格，再真实停止、清理和校对修订，最后原子保存模式与恢复意图。切回节点模式需可验证官方核心；缺失时明确要求先准备，不提交半成品。失败显示旧模式与真实停止结果，不自动重启。

## 持久化与恢复

schema 5 使用 nodesRevision、routingRevision、coreRevision、preferencesRevision。公开 DTO 排除 NodeSpec、lastConnection、控制密钥。旧代结构和损坏文件保留原件并阻止写入/恢复；显式备份初始化建立新结构，不做双读双写或迁移。

最后成功连接保存固定 NodeSpec、有序规则、逐来源版本和核心版本。名称/标签、禁用策略内容、更新频率、随机端口/密钥不影响有效运行指纹。主动停止撤销恢复意图，内部退出清理保留意图；一次恢复不套用最新草稿，材料缺失或平台要求交互授权时等待手动连接。

私有文件同步后原子替换，Windows 限定当前用户或管理员与 SYSTEM，Unix 0700/0600。日志/活动均驻本机有界内存。规则上游身份由授权侧独立验证，核心来源由项目签名材料离线验证，普通用户可写缓存的自报 hash 不是管理员信任依据。

## 平台与发行

沿用现有 Windows Job、Unix 进程组、临时 DNS 与登录项适配。实际启动、授权、双栈、清理和签名包必须由五目标原生运行器验证，ARM64 不用交叉编译替代。工程命令见[开发](development.md)，发行交接见[分发](distribution.md)。

## 连接输入与 DNS 编译

profile/connection.rs 的 ConnectionInput 是冻结业务输入；SessionService 统一构造，恢复直接使用成功快照。connectionRevision 独立于节点、路由和下载偏好，新增字段缺省兼容 schema 5。公开状态携带 worker 的实际运行选项，随机路径/密钥不进入有效指纹。

profile/bypass.rs 从有效规则收集目标并验证完整解析结果，ip_network.rs 只共用 IP/CIDR 运算。业务输入保留绕过意图，RunningConnection 另持本次固定地址；后者不持久化、不接受 IPC 提交。普通分流仍由核心匹配，显式绕过在系统路由层优先。

profile/config.rs 用固定材料路径与运行绑定一次编译，DNS 域名投影区分客户端 Fake-IP 回答与核心内部真实 Lookup；缓存、活动 API、绕过 hosts 和排除前缀在同一次编译组装。build_check 保持离线检查，compile_runtime 要求完整运行结果。network/routes.rs 同时核验排除空洞和剩余接管覆盖，不在壳中写系统路由。

session/tunnel/cache.rs 只派生受保护路径和检查链接，不实现数据库或映射。认证平台层返回核验过的用户身份，缓存按用户稳定隔离；机器级租约保持单写者，候选 check 与正式启动分开。

## 核心交付材料

core/material.rs 负责带用途标识的 Ed25519 签名材料、固定官方资产和程序摘要；复用 Rustls 现有验证实现，无新增生产依赖。普通核心维护只获取项目验证发布的稳定版，签名材料放在已选、上一版本及候选的来源记录中，不依赖单个归档缓存保留。

session/tunnel/install.rs 只处理认证通道上的 64 KiB 分块和固定受保护路径。管理员侧完整验签/哈希后才能执行 version/check，随后原子发布程序及来源记录；中断暂存不算安装完成，重试可以复用已发布但记录未提交的相同字节。核心安装在首次启动试用标记之前执行，安装失败不消耗启动回退；活动 worker 仍按原边界收尾。
