# 架构

Sing Shell 使用 Tauri 2 / Rust 与 TypeScript 原生 DOM / Vite。功能合同见[核心与配置](features/core.md)和[软件更新](features/updates.md)。

## 职责与链路

`页面/托盘/启动恢复 → SessionService → CoreService / RuntimeService → 核心进程`；应用更新由 UpdateService 准备，经统一退出协调器交接安装器。

- `main.rs` 只装配命令、文件框和原生窗口事件；文件框回调返回后才接纳选择，系统文件框不占核心变更票据。
- `app.rs` 组合服务并拥有一次启动的后台唤醒循环。应用和核心分别到期判断，不因一项网络操作等待另一项。
- `session/mod.rs` 的 SessionService 统一用户启停、内部停止、启动确认与恢复意图；页面、托盘、启动恢复及退出协调器使用同一入口。
- `core/mod.rs` 的 CoreService 管理来源、原始配置校验、受管选择、采用与版本回退；SessionService 在固定启动用例中调用其资产操作，不反向引用会话。
- `core/managed.rs` 的 ManagedCore 拥有候选、下载准备、核心维护周期和缓存，不自行改当前选择或运行代理。
- `core/package.rs` 核对/解包核心文件和受管路径，提供可取消的散列与清理，不读取当前配置或决定来源。
- `runtime.rs` 拥有临时校验进程、运行 Child、日志 reader、启动观察和真实退出结果；使用固定 StartSpec，不保存用户设置。supervisor 独占运行 Child，启动调用等待其有界结果。
- `settings.rs` 是设置的唯一串行写入者，分别校对核心与偏好修订；`platform/` 适配 Windows Job / Unix 进程组，`logs.rs` 保存有界内存日志。
- `updates/service.rs` 只负责应用检查、下载和准备，仅持有 Logs，不拥有核心运行服务；`application.rs` 调用 Tauri 官方 updater。`release.rs/network.rs` 复用两个具体发行适配与传输，不提供通用更新框架。
- `lifecycle.rs` 封闭接纳、取消维护、等待所有者、停止核心，再处理退出或应用安装；不采用核心版本。

前端四页为运行、节点、分流、设置，只呈现脱敏快照、编辑意图，不维护自动更新计时器。profile/service 管理导入票据与节点事务；profile/config 生成能力档固定的配置；rulesets 管理固定来源和不可变规则版本。session/tunnel 管理授权通道和闭合消息，platform/authorization 处理系统提权及身份，platform/network 观察接口路由与残留恢复。不引入框架、数据库或通用守护进程。

`session/tunnel/protocol.rs` 定义闭合业务消息和阶段，`worker.rs` 独占受保护核心准备、运行及清理，tunnel 控制线程独占通道及平台进程所有者。授权握手先由 helper 就绪，再核对对端并双向确认随机会话；macOS 通过 audit token、Security.framework 与运行 CDHash 校验同构建，安装归属检查在平台模块。线程/进程实际结束前不清除所有者；调用超时保留其状态供后续停止。Poll/快照只读，worker 运行循环处理异常退出。

授权启动和安全检查按 Windows／Unix 具体文件组织，进程从创建起归授权会话所有。worker 启动线程返回资源与结果，主循环统一发布状态并清理；macOS 临时 DNS 会话与生成配置纳入相同所有者。`platform/dns` 适配临时解析器，`platform/network` 核对预期地址和路由覆盖，`profile/tun` 提供固定规格与核心能力档。系统查询有时限和输出上限。Windows 安装范围由 `platform/installation` 提供给 updater 与登录项，登录项不拥有升级安装策略。

Tunnel 持有运行占用，覆盖授权、运行及清理待完成状态，RuntimeService 与核心采用共享该事实，避免各失败分支手工复位。实际关闭确认后释放；操作票据结束不代表连接结束。Session 统一阶段到进程兼容状态的投影，运行节点只随真实进程呈现，持久的上次成功连接独立保存。

SessionService 每次采样只读取一份对应模式的运行事实，IPC 外层及托盘使用同一份 runtime、阶段和操作许可。托盘使用包含图像状态的完整 Projection，分别更新变化的菜单、图像及提示，不通过文案或 PID 独立推断连接成功。

## 设置、缓存与恢复

settings schema 4 的 coreSource 区分 managed/external，managed 保存 active/previous/trial/blocked。profile 保存模式、节点、选择、全局分流和规则提交，startup.lastConnection 保存最后成功的固定输入。公开 DTO 排除凭据和 lastConnection。原始 configPath/pendingConfigPath 保留兼容，不上传配置正文。

coreRevision、profileRevision 与 preferencesRevision 分开。局部提交从最新设置合并对应字段，后台旧快照不能覆盖草稿。采用时串行核对核心上下文、关闭/取消和自动采用偏好。私有临时文件同步后原子替换；设置损坏或未知结构保留原件，阻止覆盖。

v0/v1/v2/v3 先备份再迁移；外部选择不变，有原始配置的用户保持原始配置模式。legacy 与官方证据区分。迁移不联网、不运行外部程序、不自动接管网络。

核心准备记录最后提交，绑定来源请求和选择代次；恢复不相信上次进度文案，重新核对本地字节。active、previous 和 trial 一次保存，确认启动才清 trial，未确认中断按恢复规则处理。进程 PID/版本来自真实进程快照，不从预期选择伪造。

## 并发边界

短接纳锁管理具体操作：核心变更互斥、单个核心下载准备、单个应用检查/下载、偏好写入和诊断。下载与当前代理可并行。采用和清理另获取核心文件票据，保护 staging 与版本目录；网络 worker 释放维护票据后再请求采用，避免循环持锁。

进度/错误带任务所有权，来源变更使旧操作失效；线路设置变化不污染新健康结果。配置变化使兼容结论失效，每次启动重验。散列、解包和检查接受取消，票据在实际资源收尾后释放。退出不把取消请求当作完成，失败时保留实际状态、允许重试。

受管版本只在停止或下次启动前采用。应用/核心/规则各自准备，不提供跨组件事务。网络 worker 独占机器级租约和受保护核心，控制端失联触发停止；残留记录绑定 PID/启动时间/路径。停止先有界优雅关闭再强制终止，Windows Job 使用 KILL_ON_JOB_CLOSE。接口未释放则保持 cleanupRequired，禁止重入。

## 依赖

沿用已固定的 Tauri updater/opener、reqwest/rustls、Tokio、semver、sha2、zip/tar/flate2，新增 macOS 限定的 objc2 / objc2-foundation 直接绑定，版本与 Tauri 已用版本一致；不增加另一套运行时。应用验签与安装交给官方 updater，配置、网络协议、DNS 和转发语义交给 sing-box。工程命令和平台验证见[开发](development.md)。

## 桌面与启动边界

SettingsStore 持有 instance::Lease；读取和迁移设置前完成独占与本地激活。Windows 管道限定当前用户并核对服务端身份，Unix socket 使用私有目录和同 UID 检查；协议只允许固定的显示窗口请求。更新重启有界等待旧锁，登录参数不传播到下一次重启。

desktop.rs 管理单窗口、托盘、模态保护与待导航，不管理 Child。startup.rs 归一化启动来源和重启环境。login_startup 读取/修改系统当前用户登记，不在设置复制 enabled。开发/隔离验收禁止修改真实登录项。

SessionService 在同一启动用例完成启动观察、可选一次核心回退和成功提交。startup.restoreCoreOnLaunch 为用户开关，resumeCore 为成功运行意图；主动停止撤销，内部停止和异常退出不改写。一次恢复请求可等待必需核心，失败/取消后消费，不形成守护循环。Stop 有独立票据并等待原启动所有者回收资源，意图代次拒绝迟到成功提交。

lifecycle.rs 使用 Quit / InstallAndRestart，先向当前前端文档发出带 ID 的预检，再封闭 Admission，等待 bootstrap、核心、网络和登录项操作，验包、内部停止核心，最后交给安装器。安装器直接终止进程前已经完成必要清理。
