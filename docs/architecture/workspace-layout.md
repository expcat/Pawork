# Cargo Workspace 结构

## 1. 顶层布局

仓库根即 Cargo workspace 根。顶层目录：

```text
Pawork/
├── crates/      # 核心 crate
├── apps/        # 可执行入口
├── schemas/     # JSON Schema 与生成的 TypeScript declarations
├── fixtures/    # 测试夹具
├── benches/     # 性能基准
├── docs/        # 文档
└── Cargo.toml   # workspace 根 manifest
```

> 进程模型：CLI 与 Core 是同一进程同一二进制，`apps/pawork` 是唯一正式宿主；GUI 作为独立进程经 GUI Connection Protocol 连接 CLI。不存在独立的 `core-daemon`、`core-client`、`core-server` 入口。详见 [总体架构](overview.md) §2 与 [ADR-021](../adr/ADR-021-cli-core-same-process.md)。

## 2. crates/

| crate | 职责 | 依赖方向备注 |
| --- | --- | --- |
| `agent-domain` | 领域类型：消息、Run、事件、ID | 最底层，零外部 IO 依赖 |
| `agent-events` | 事件类型与序列化、schema version | 依赖 agent-domain |
| `agent-engine` | Agent Loop、运行控制、预算 | 依赖 provider-api / tool-api / plugin-api |
| `agent-api` | 对外领域 API 聚合 | 依赖 agent-engine |
| `provider-api` | Provider Trait、canonical 请求 / 事件 / 错误 | 依赖 agent-domain |
| `provider-runtime` | Provider 协议运行时、SSE/JSONL 解析、重试；Phase 2 现有 HTTP 基线后续抽到通用 `http-runtime` | 依赖 provider-api；抽离后依赖 http-runtime |
| `provider-control` | ProviderAccount、CredentialPool/Lease、RoutingPolicy、ErrorClassifier、Health 与 Session Affinity | 依赖 provider-api / model-registry / agent-domain；Secret 解析由组合层注入，不持久化明文；P18-3～P18-7/P18-14 |
| `provider-openai` | OpenAI 适配 | 依赖 provider-runtime |
| `provider-anthropic` | Anthropic 适配 | 依赖 provider-runtime |
| `provider-google` | Google Gemini 适配 | 依赖 provider-runtime |
| `provider-openai-compatible` | OpenAI 兼容适配（含 Ollama / vLLM / LM Studio） | 依赖 provider-runtime |
| `provider-xai` | xAI Grok 适配（API Key + OAuth bearer 订阅） | 依赖 provider-openai-compatible / provider-runtime；OAuth 登录与刷新由组合层调用 auth-service，不反向引入 |
| `provider-zhipu` | 智谱 GLM 适配（BigModel OpenAI-compatible） | 依赖 provider-openai-compatible / provider-runtime |
| `provider-qwen` | 阿里 Qwen 适配（DashScope OpenAI-compatible） | 依赖 provider-openai-compatible / provider-runtime |
| `provider-moonshot` | Moonshot Kimi 适配（OpenAI-compatible） | 依赖 provider-openai-compatible / provider-runtime |
| `provider-bedrock` | AWS Bedrock（优先级 P1） | 依赖 provider-runtime |
| `provider-mistral` | Mistral（优先级 P1） | 依赖 provider-runtime |
| `auth-service` | 认证方式、Secret 后端、OAuth（PKCE/Device Flow/refresh/callback） | 依赖 provider-api |
| `model-registry` | 模型目录、别名、能力、定价 | 依赖 provider-api |
| `tenant-service` | Tenant/Principal、RBAC、provider/model/account policy、legacy `local/default` 映射 | 依赖 agent-domain；通过 API 与 policy-engine 组合，不反向依赖 Provider adapter；P18-2/P18-9 |
| `usage-ledger` | tenant/account/session/agent 多维 Usage/Cost 持久账本 | 依赖 agent-domain / agent-events / model-registry；P18-8 |
| `quota-service` | account-scoped 用量与额度监控、多适配器、窗口聚合与缓存 | 依赖 provider-api；消费 tenant-service / usage-ledger，复用 auth-service / provider-runtime |
| `config-service` | 确定性配置 schema、来源发现与层级合并 | 独立；供 context-engine / policy / resource-loader 等消费 |
| `context-engine` | 上下文构建、Token 预算、Resource 优先级 | 依赖 agent-domain / resource-loader；只消费中性 DTO，不参与文件 IO |
| `compaction-engine` | 自动 / 手动压缩、摘要 | 依赖 agent-events / session-store |
| `session-store` | SQLite Event Store、Projection、迁移 | 依赖 agent-events |
| `artifact-store` | Blob Store、内容寻址、GC | 独立，被 session-store / tools 引用 |
| `app-database` | SQLite Actor、连接、备份、只读恢复 | 独立连接层，不依赖具体 schema；session-store 依赖它 |
| `tool-api` | AgentTool Trait、Descriptor、Result | 依赖 agent-domain |
| `tool-runtime` | Tool Scheduler、并发 / 串行策略 | 依赖 tool-api |
| `builtin-tools` | read / write / edit / apply_patch / command / search / find / list | 依赖 tool-api |
| `process-runtime` | 跨平台进程、超时、取消、进程树终止 | 独立 |
| `pty-service` | 集成终端 PTY | 依赖 process-runtime |
| `policy-engine` | Approval、Policy 决策、路径 / Shell 安全 | 依赖 tool-api |
| `sandbox-runtime` | Sandbox Backend、capability 策略 | 依赖 process-runtime / policy-engine |
| `audit-log` | versioned canonical 审计事件、tenant-scoped 查询与 OTel/SIEM 脱敏导出 | 依赖 agent-domain；运行时由组合层接入 tenant-service / usage-ledger；P18-13 |
| `workspace-service` | 工作区、多 Root、Git 检测、设置 | 依赖 agent-domain |
| `file-index` | 文件索引、ignore、`@file` 搜索 | 依赖 workspace-service |
| `resource-loader` | AGENTS.md / Skills / Prompt / Profile 加载 | 依赖 agent-domain / config-service / workspace-service / diagnostics；不依赖 context-engine |
| `git-service` | 系统 Git 封装、缓存、Worktree | 依赖 process-runtime |
| `diff-service` | 结构化 Diff 解析、分页 Hunk | 依赖 git-service |
| `checkpoint-service` | Run 写操作快照、回滚 | 依赖 git-service / artifact-store |
> 注：Phase 4 交付的 checkpoint-service 基于 artifact-store 实现 Blob 快照与回滚；git-service 接入（导出 patch / 固化为 commit）随 Phase 7 完成。
| `plugin-api` | 插件 Trait、Manifest、生命周期事件 | 依赖 agent-domain / tool-api |
| `wasm-plugin-host` | WASM Component 宿主、capability / fuel | 依赖 plugin-api |
| `mcp-client` | MCP Transport、Tools / Resources / Prompts、Server 生命周期与安全边界 | 依赖 agent-domain / tool-api / tool-runtime / policy-engine / config-service / auth-service；MCP 协议与 transport 由 crate 内部封装的 `rmcp` 提供，不向 Core 泄漏 SDK 类型之外的运行时职责 |
| `hook-runtime` | 生命周期 Hook 派发 | 依赖 plugin-api |
| `orchestration` | Multi-Agent、Supervisor、Worker、任务图（优先级 P2） | 依赖 agent-engine / workspace-service |
| `core-api` | 应用层 Command / Event / Query 类型（CLI 与 GUI 共享的 schema source） | Phase 0 依赖 agent-domain / agent-events；后续由 app-service 使用 |
| `core-runtime` | 完整 Core 生命周期与业务运行时装配 | 依赖 agent-api 及几乎所有核心 |
| `app-service` | CLI 与 GUI 共享的应用 API、状态聚合、监督 | P1 骨架依赖 core-api；Phase 13 接入 core-runtime |
| `cli-host` | 将 Core、CLI、GUI Server 装配到同一进程、生命周期管理 | 依赖 app-service |
| `cli-command` | 命令解析与稳定命令模型 | 独立；由 cli-host 映射到 app-service |
| `cli-renderer` | CLI 文本 / JSON / 流式输出（消费 Event Hub） | P1 骨架依赖 app-service；Phase 13 接入 core-api / agent-events |
| `gui-protocol` | GUI Command / Query / Event / Snapshot 协议类型 | 依赖 core-api |
| `gui-server` | CLI 内部运行的 GUI 协议服务器 | 依赖 gui-protocol / app-service |
| `gui-client` | Tauri GUI 使用的 typed 连接 SDK：握手、认证、订阅、Snapshot/Event 重连 | 依赖 gui-protocol / transport-api；不得依赖 core-runtime / app-service |
| `connection-manager` | 管理一个 CLI 实例上的多个 GUI 连接 | 依赖 gui-server |
| `subscription-hub` | 将 Core Event 广播给 CLI 与所有 GUI | 依赖 core-api / agent-events |
| `snapshot-service` | 为 GUI 提供当前状态快照与重连恢复 | 依赖 app-service |
| `client-auth` | GUI 客户端身份验证 | 依赖 auth-service |
| `transport-api` | GuiTransportServer / Client / 帧抽象 | 独立 |
| `transport-local` | 本地 Transport（Unix Socket / Named Pipe） | 依赖 transport-api |
| `transport-memory` | 进程内 Transport（测试用） | 依赖 transport-api |
| `transport-remote-placeholder` | 远程 Transport 占位接口（可替换 Adapter） | 依赖 transport-api |
| `diagnostics` | 诊断包、脱敏日志、metrics | 横切 |
| `test-support` | Mock Provider / Mock Tool、测试工具 | 仅测试依赖 |
| `schema-typegen` | 从 core-api / gui-protocol 生成并校验 `.d.ts` | 仅构建工具依赖 core-api / gui-protocol，不进入运行时 |

### 2.1 Phase 15–19 与跨阶段规划新增 crate（登记在册，尚未实现）

> 登记即冻结 crate 名、职责与依赖方向；落地计划见 [ROADMAP](../../ROADMAP.md) Phase 15–19。新增 crate 遵循 §6/§7 规则，`agent-domain` 保持零外部 IO 依赖。embedding 经评估**不新增独立 crate**，扩展 `provider-api`（见下表脚注）。Phase 19 不新增 Core crate，复用 `gui-client` 并在 `apps/desktop` 内实现 Tauri bridge 与 React renderer。

| crate | 职责 | 依赖方向备注 |
| --- | --- | --- |
| `http-runtime` | 通用 HTTP client、超时、代理、header、trace、取消、重试；供 Provider、Hooks、Marketplace、Forge 等复用 | 从 `provider-runtime::http` 抽离；不得依赖具体 Provider；P2-1 后续收敛 |
| `protected-blob-store` | 受保护敏感制品（reasoning 凭证等）加密落盘：encrypted-at-rest、Provider/Session 作用域、retention、引用计数 GC、完整性校验 | 依赖 `agent-domain`；与 `artifact-store`（非加密）平级、共享存储底层但不混用安全语义；ADR-032 |
| `monitor-service` | 声明式监视循环 + 常驻进程托管；Plugin Package Monitors 的唯一运行时执行宿主 | 依赖 `agent-domain` / `process-runtime`；P16-6 |
| `memory-service` | 跨会话长期记忆；提炼 / 嵌入检索 / 失效 | 依赖 `agent-domain` / `provider-api`（canonical `EmbeddingProvider`）；P16-7 |
| `plan-service` | Plan 模式与 PlanArtifact | 依赖 `agent-domain` / `context-engine`；P16-1/P16-2 |
| `goal-service` | durable objective + success criteria | 依赖 `agent-domain`；P16-3 |
| `task-manager` | process/agent/monitor/automation 统一后台任务管理 | 依赖 `agent-domain` / `process-runtime`；P16-4 |
| `automation-service` | cron/interval/once/event trigger + inbox；外部 trigger 经 adapter | 依赖 `agent-domain`；P16-5 |
| `review-engine` | 行锚点评审 + re-anchor + resolution 生命周期 | 依赖 `agent-domain` / `diff-service`；P16-8 |
| `user-hooks` | 用户声明式事件钩子（Command/Http/PromptTransform/PromptEval/AgentEval/McpTool） | 依赖 `agent-domain` / `provider-api` / `mcp-client`；与 `hook-runtime` 并列；P17-1 |
| `plugin-package` | Plugin Package 聚合格式（Skills/Agents/Hooks/MCP/LSP/Monitors） | 依赖 `agent-domain` / `resource-loader`；P17-2 |
| `marketplace` | 扩展市场：发现/安装/更新/卸载/签名/trust/team-policy | 依赖 `plugin-package` / `http-runtime`；P17-3 |
| `lsp-runtime` | LSP **Client** Runtime：启动/管理/调用现有 Language Server | 依赖 `agent-domain` / `process-runtime` / `sandbox-runtime`；P17-4 |
| `teams` | Agent Teams / peer messaging / shared task board | 依赖 `agent-domain` / `orchestration`；P17-6 |
| `client-adapter-api` | 外部 Agent Client 的统一 adapter/factory、capability snapshot、Session Registry 契约 | 依赖 `agent-domain` / `core-api`；P18-10 |
| `client-codex-app-server` | Codex App Server Thread/Turn/Item/approval/subagent adapter | 依赖 `client-adapter-api` / `app-service`；P18-11 |
| `client-claude-gateway` | Claude Gateway session/agent identity、Messages stream 与审计归属 adapter | 依赖 `client-adapter-api` / `app-service`；P18-12 |
| `acp-host` | 公共 Agent Client Protocol adapter 与协议宿主 | 依赖 `client-adapter-api` / `core-api` / `app-service` / `agent-events` / `subscription-hub`（连接 pawork Host）；P17-7 |
| `agent-sdk` | Rust client SDK + Headless JSON 协议接入 | 依赖公开 schema/framing（不依赖 `core-runtime`）；P17-8 |
| `ide-host-adapter` | IDE 生命周期/诊断/交互桥接 + 可选 LSP Server 输出 | 依赖 `agent-sdk` / `core-api`；与 `gui-server` 平级；P17-9 |
| `browser-computer-runtime` | Browser/Computer 能力 facade（Local/MCP/ProviderHosted 三执行位点） | 依赖 `tool-api` / `provider-api` / `sandbox-runtime`；P17-10 |
| `transport-remote` | 真实远程 Transport（替代 placeholder） | 依赖 `transport-api`；P17-11 |
| `remote-control-adapter` | Mobile / 远程控制受限通道 | 依赖 `transport-api` / `core-api`；P17-12 |
| `compat-loader` | Claude/Codex/Grok/Cursor/Pi 配置兼容导入（只读） | 依赖 `agent-domain` / `resource-loader`；P17-13 |

> embedding 决策（2026-08 冻结）：**不新增独立 `embedding-api` / `embedding-runtime` crate**。canonical embedding 抽象（`EmbeddingProvider` trait + `EmbeddingRequest` / `EmbeddingResponse` / `EmbeddingModelDefinition` / `EmbeddingCapabilities`）扩展进 `provider-api`——embedding 是 Provider 的另一项 canonical 能力，与 `ModelProvider` 平级放在同一层最契合现有依赖方向，复用同一套凭证与 model-registry，并使 `memory-service` 只依赖 `provider-api`（Provider 无关）。各 `provider-*` 实现 `EmbeddingProvider`。

## 3. apps/

| 目录 | 说明 |
| --- | --- |
| `apps/pawork` | **唯一 CLI/Core 主程序**（二进制名 `pawork`），CLI 与 Core 同进程；提供 run / serve / shell / watch / service 等模式 |
| `apps/protocol-test-gui` | GUI Connection Protocol 测试客户端，用于在不开真实 GUI 时验证多 GUI 全流程 |
| `apps/desktop` | Phase 19 Tauri + React GUI；独立进程，经 `gui-client` / GUI Connection Protocol 连接 `pawork`，只保存 UI preference 与可丢弃投影 |

> 已删除的原计划入口：`apps/core-daemon`、`apps/core-cli`、`apps/core-rpc`、`crates/core-server`、`crates/core-client`、`crates/core-daemon`。它们的能力全部并入 `apps/pawork` 与 `cli-host`/`gui-server`。

## 4. schemas/

| 目录 | 说明 |
| --- | --- |
| `core-api/` | app-service Command / Event / Query 的生成 `.d.ts`（Rust 类型是 schema source） |
| `gui-protocol/` | GUI Connection Protocol 握手、帧、Snapshot 的生成 `.d.ts` |
| `events/` | Core Event 序列化 schema |
| `transport/` | Transport 帧与端点 schema |
| `authentication/` | 客户端认证 schema |
| `plugin-api/` | 插件 API JSON Schema |
| `mcp/` | MCP 消息 JSON Schema |
| `import/` | Pi 导入格式 JSON Schema |

## 5. fixtures / benches / docs

- `fixtures/`：providers / sessions / diffs / tools / pi-import 测试夹具
- `benches/`：性能基准（区分 Rust Core / Git 子进程 / Provider 网络 / 模型生成 / 外部命令 / GUI 渲染）
- `docs/`：architecture / adr / features / quality

## 6. 依赖方向与禁依赖

```text
agent-domain
     ↑
provider-api  tool-api  plugin-api
     ↑
provider-*   builtin-tools   plugin-host
     ↑
agent-engine
     ↑
core-runtime
     ↑
app-service（core-api 类型为 schema source）
     ↑
cli-host
  ├── cli-command / cli-renderer
  └── gui-server
         ↑
      transport-api
        ├── transport-local
        └── transport-remote-placeholder

gui-client ↑ Tauri GUI（独立进程）
```

Provider 调用与外部 Agent Client 的扩展链保持单向：

```text
Codex / Claude / ACP
        ↓
client-*-adapter → client-adapter-api → app-service
                                          ↓
                               tenant-service / orchestration
                                          ↓
                                  provider-control
                                          ↓
                               provider-api / provider-runtime
```

> 本图仅画主干链；完整 crate 清单以 §2 为准（含 agent-api、app-database、transport-memory、hook-runtime 等）。

强制规则：

- `pawork` 是 Core 的唯一正式可执行宿主。
- CLI 命令直接通过 `app-service` 操作同进程 Core；GUI 必须通过 `gui-server` 操作 Core；GUI 不直接链接 `core-runtime`。
- `apps/desktop/src-tauri` 只链接 `gui-client` 与必要 Tauri 官方插件；不得链接 app-service、数据库、Provider、Tool、Git 或通用 shell/fs/http/sql capability。
- `gui-server` 与 CLI 命令共享同一个 `app-service` 实例；CLI 与 GUI 的操作进入同一个 Command Router，接收同一个 Event Hub。
- Transport 不包含 Agent 业务逻辑。
- 必须禁止循环依赖。

`agent-domain` 不得依赖：

- Tauri
- SQLite
- HTTP Client
- OS Keychain
- Git
- 具体 Provider

## 7. 新增 crate 流程

1. 在本文件第 2 节登记 crate 与职责、依赖方向。
2. 创建 `crates/<name>/Cargo.toml` 与 `lib.rs`。
3. 确认不引入对 `agent-domain` 禁依赖项的引用。
4. 涉及对外契约（API / Schema）的，更新对应 `schemas/` 与 ADR。
5. 同步 [ROADMAP](../../ROADMAP.md) 中相关任务。

## 8. 相关文档

- [总体架构](overview.md)
- [GUI Connection Protocol](api-surface.md)
- [GUI 连接与多客户端](../features/gui-connection.md)
- [Desktop GUI](../features/desktop-gui.md)
- [CLI Host](../features/cli-host.md)
- [ROADMAP](../../ROADMAP.md)
- [ADR-001 纯 Rust Core](../adr/ADR-001-pure-rust-core.md)
- [ADR-002 Agent Engine 与 Provider 解耦](../adr/ADR-002-agent-engine-provider-decoupled.md)
- [ADR-021 CLI 与 Core 同进程](../adr/ADR-021-cli-core-same-process.md)
- [ADR-025 CLI 是唯一宿主](../adr/ADR-025-cli-is-sole-host.md)
