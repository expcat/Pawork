# 术语表

| 术语 | 含义 |
| --- | --- |
| Pawork | 本项目，纯 Rust 编码智能体核心平台 |
| Pi | 功能与行为参考的 TypeScript 编码智能体；仅作参考与迁移数据来源 |
| Agent Core | Rust 核心能力层 |
| Core Runtime | 完整 Rust Core 生命周期与业务运行时，由 `core-runtime` 装配 |
| CLI Host | CLI 与 Core 的同一进程宿主，`pawork` 是唯一正式二进制 |
| app-service | CLI 与 GUI 共享的应用 API 层，命令/事件的唯一稳定入口 |
| Command Router | 统一接收 CLI 与 GUI 命令并转交 app-service 的路由 |
| CommandSource | 命令来源（LocalCli / LocalGui / RemoteGui / Automation / Plugin / Mcp），所有状态变更均记录来源 |
| Event Hub | Core 事件统一分发点，扇出到 CLI 渲染器、所有 GUI、审计与自动化 |
| Subscription Hub | 将 Core Event 广播给 CLI 与所有 GUI 的订阅中心 |
| GUI Connection Protocol | GUI 与 CLI/Core 之间唯一的线上协议（Command/Query/Event/Snapshot） |
| GUI Server | CLI 进程内运行的 GUI 协议服务器 |
| GUI Client SDK | Tauri GUI 使用的连接 SDK（`gui-client`） |
| Desktop Projection Store | Desktop GUI 从 Snapshot/Event 重建的可丢弃 materialized view；不是权威状态，不保存业务事实 |
| Connection Manager | 管理一个 CLI 实例上多个 GUI 连接的组件 |
| Snapshot Service | 为 GUI 提供当前状态快照与重连恢复的组件 |
| Transport | GUI 与 CLI 之间的传输抽象（Local：Unix Socket / Named Pipe；Remote：可替换 Adapter） |
| Core Instance | 一个 CLI/Core 运行实例，默认 `pawork://default`，可命名多实例 |
| Agent Loop | 模型—工具循环，14 步见 [控制流](architecture/control-flow.md) |
| Run | 一次 Agent 执行，有唯一 ID 与状态机 |
| Session | 一次会话，事件为事实来源 |
| Branch / Fork | 从任意事件派生新会话分支 |
| Event Store | `session_events`，会话事实来源，可重放 |
| Event Stream | 事件的逻辑流（如某 Session / Run 的事件序列），带严格递增 `stream_sequence`；与 `global_sequence` 共同支撑重连重放 |
| Projection | 由事件重建的可派生表 |
| Blob Store | BLAKE3 内容寻址的大型内容存储 |
| Artifact | 大型内容的引用句柄，事件中只传 Artifact ID |
| Compaction | 会话压缩，保留关键约束与未完成任务 |
| Canonical | Provider 无关的统一请求/事件领域 |
| Provider Runtime | 统一封装各模型供应商的运行时 |
| Provider Account | 某 Provider 下可被策略和健康状态管理的账号资源；与 Credential/Secret 分离 |
| Credential Pool | 按 tenant、capability、health、policy 与并发限制获取 Credential Lease 的资源池 |
| Credential Lease | 一次有期限、可释放/回收的账号使用权；不等于 Secret 明文 |
| Routing Policy | 对合法候选执行 priority/weight/fill-first/affinity/fallback 的可组合策略，不承担 Agent 调度 |
| Error Classifier | 把 Provider/协议错误归一成 failure class/scope/health impact/failover safety 的扩展点 |
| Tenant / Principal | 组织/逻辑租户与当前用户/服务账号；本地默认分别为 `local/default` / `local/user` |
| Usage Ledger | 按 tenant/account/session/agent/provider/model 记录 canonical usage/cost 的持久事实源 |
| Client Adapter | 外部 Agent Client 协议与 Pawork canonical event 之间的版本化翻译层 |
| Session Registry | 保存外部/内部 session 映射、连接、revision、ownership epoch 与 capability snapshot 的权威登记表 |
| Model Registry | 模型目录、能力、别名、定价 |
| Tool Scheduler | 按 capability 调度工具并发/串行 |
| capability | 工具类别：ReadOnly / WorkspaceWrite / GitWrite / Process / Network / UserInteraction / ExternalPlugin |
| Checkpoint | 写操作前的逻辑快照，支持回滚 |
| Workspace Trust | 工作区信任状态，决定默认权限 |
| Sandbox | 受控执行环境，按 policy 限制文件/网络/进程/Secret |
| Skills | 声明式能力包（SKILL.md + manifest） |
| MCP | Model Context Protocol，第一外部扩展机制 |
| WASM Plugin | capability-based 的代码插件 |
| Diagnostics Bundle | 可导出的脱敏诊断包 |
| ADR | Architecture Decision Record，架构决策记录 |
| P0 / P1 / P2 | 优先级分级：P0 必须、P1 重要、P2 可推迟；与任务 ID 中的 Phase 序号无关 |
| Phase | 路线图阶段（当前 Phase 0–19）；任务 ID `P{n}-{seq}` 中的 `P{n}` 是 Phase 序号，不是优先级 |
| Streamable HTTP | MCP 远程传输规范（2025-03-26 起），取代旧 HTTP+SSE |
| 权威状态 | Core 是所有客户端状态的唯一权威来源，CLI 输出与所有 GUI 都是其观察者与操作入口（[ADR-030](adr/ADR-030-core-sole-source-of-truth.md)） |
