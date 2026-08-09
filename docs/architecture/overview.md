# 总体架构

## 1. 定位

Pawork 是一个纯 Rust 编码智能体核心平台。**CLI 与 Rust Core 是同一个程序和进程边界**：`pawork` 二进制同时是 Core 的宿主、命令行入口与 GUI 连接服务器；Desktop GUI（Phase 19，Tauri + React）作为独立进程，通过 CLI 暴露的 GUI Connection Protocol 连接 Core，而不是直接嵌入 Core。

详见 [README](../../README.md) 的项目定位与 [ADR-001](../adr/ADR-001-pure-rust-core.md)、[ADR-021](../adr/ADR-021-cli-core-same-process.md)、[ADR-025](../adr/ADR-025-cli-is-sole-host.md)。

## 2. 运行架构

```text
┌──────────────────────────────────────────────────────────────┐
│ Host Environment（Windows / macOS / Linux）                   │
│ ┌──────────────────────────────────────────────────────────┐ │
│ │ CLI + Rust Core（同一进程，二进制 pawork）                │ │
│ │  CLI Commands        GUI Connection Server                │ │
│ │  Process Lifecycle   Connection Manager                   │ │
│ │  Service Mode        Authentication                       │ │
│ │  CLI Renderers       Subscription Hub                     │ │
│ │  Agent Engine        Provider Runtime                     │ │
│ │  Agent Supervisor    Provider Account Control Plane       │ │
│ │  Context Engine      Tool Runtime                         │ │
│ │  Session Store       Tenant / Policy / Usage / Audit      │ │
│ │  Workspace Service   Git / Diff                           │ │
│ │  Plugin / MCP Host   Artifact Store                       │ │
│ └───────────────┬──────────────────────────────┬───────────┘ │
│                 │ Local Transport              │             │
└─────────────────┼──────────────────────────────┼─────────────┘
          ┌───────▼────────┐             ┌───────▼────────┐
          │ Local GUI A    │             │ Local GUI B    │
          │ Tauri + React  │             │ Tauri + React  │
          └────────────────┘             └────────────────┘
                  Remote Transport Adapter ── 内网穿透 ── Remote GUI C/D
```

不存在独立的 Core Daemon 与 CLI Client；`pawork` 是 Core 的唯一正式宿主。CLI 自身发起的操作和任一 GUI 发起的操作都进入同一个 Core；所有 GUI 通过 CLI 接收统一的 Snapshot 与 Event。

### 2.1 CLI Host（CLI + Core 同进程）

`pawork` 二进制同时承担：初始化并运行 Core、提供命令行界面、运行 GUI 连接服务器、管理进程生命周期（前台/后台服务/开机启动/优雅关闭/崩溃恢复）、提供本地与远程 Transport。支持一次性命令、服务、交互与系统服务四种运行模式，详见 [CLI Host](../features/cli-host.md)。

### 2.2 Application Service（app-service）

面向 CLI 与 GUI 的稳定应用 API，负责状态聚合、事件限流、任务监督、错误转换。CLI 命令直接调用 app-service（不绕回自身建立 IPC）；GUI 通过 GUI Connection Protocol → GUI Server → app-service。二者共享同一个 app-service 实例与同一个 Event Hub，保证业务行为一致。

### 2.3 GUI（独立进程，可连接视图）

Timeline / Composer / Diff / Terminal / Settings / Workspaces / Sessions。GUI 连接指定 CLI/Core 实例，发送 Command、执行 Query、订阅 Event、获取 Snapshot、流式展示 Agent/Tool/Terminal。GUI 不直接加载 Core crate、不直接访问数据库、不直接调用 Provider/工具，本地只保存纯 UI 偏好。

Phase 19 在 `apps/desktop` 落地 Tauri + React 客户端：Tauri Rust bridge 只依赖 `gui-client`，React renderer 只消费生成的 TypeScript schema 与 bridge 事件。权威状态始终在 `pawork`；renderer 的 store 是可从 Snapshot/Event 重建的 materialized view，断线或版本缺口时必须重新同步，不能以 optimistic UI 覆盖 Core 拒绝结果。

### 2.4 Agent Core

所有能力实现层。职责拆分沿用 Pi 的领域划分，但不沿用其 TypeScript 实现。模块映射见 [workspace 结构](workspace-layout.md) 第 6 节。

### 2.5 Host Adapters / Client Channels（Phase 17–19）

`pawork` 是 Core 的唯一正式宿主；除 CLI 自身外，所有外部接入方都是「连接到 `pawork` Host 的 Client Channel / Host Adapter」，各自不构造第二个 Core、不替代 GUI Connection Protocol，并列存在：

```text
GUI ─────────┐
IDE ─────────┤   IDE Host Adapter → Agent SDK / Headless Protocol ──┐
Codex ───────┤   Codex / Claude / ACP ClientAdapter ────────────────┤
Claude ──────┼─▶ 连接 pawork Host ─▶ Core（单一事实源）
ACP ─────────┤   Mobile Remote Control Adapter ──────────────────────┘
Mobile/SDK ──┘
```

- **GUI**：独立进程经 GUI Connection Protocol 连接（[ADR-022](../adr/ADR-022-gui-connects-via-cli.md)）。
- **IDE**：经 IDE Host Adapter → Agent SDK / Headless 协议连接（P17-9/P17-8），**不「通过 SDK 嵌入第二个 Core」**；可选向 IDE 暴露 LSP Server 输出（复用 P17-4 LSP Client 聚合结果）。
- **Codex / Claude / ACP / SDK / Mobile**：各自是连接 `pawork` Host 的 Client Channel（P18-11 / P18-12 / P17-7 / P17-8 / P17-12），外部 Agent Client 复用 P18-10 `ClientAdapter`/Session Registry，GUI 仍使用自己的 GUI Connection Protocol。所有 channel 共享同一 `app-service` 与 Event Hub，互不触达对方协议帧。

## 3. 核心原则

- CLI/Core 单进程单二进制：`pawork` 是 Core 的唯一正式宿主；CLI 与 Core 同进程，不启动外部 Sidecar（[ADR-021](../adr/ADR-021-cli-core-same-process.md)、[ADR-025](../adr/ADR-025-cli-is-sole-host.md)）。
- GUI 是可连接视图：GUI 作为独立进程经 GUI Connection Protocol 连接 CLI/Core，不嵌入 Core（[ADR-022](../adr/ADR-022-gui-connects-via-cli.md)、[ADR-027](../adr/ADR-027-local-remote-same-protocol.md)）。
- 多 GUI 共宿主：一个 CLI/Core 实例可同时服务多个本地与远程 GUI；GUI 之间不做点对点同步，统一由 Core 广播（[ADR-023](../adr/ADR-023-one-core-many-guis.md)、[ADR-029](../adr/ADR-029-no-peer-gui-sync.md)、[ADR-030](../adr/ADR-030-core-sole-source-of-truth.md)）。
- CLI 与 GUI 一致：共享同一 app-service 与 Event Hub，命令进入同一 Command Router，事件以相同顺序扇出（[ADR-024](../adr/ADR-024-shared-app-service-event-hub.md)）。
- GUI 断线不影响任务：GUI 退出/断线不结束正在运行的 Agent（[ADR-026](../adr/ADR-026-gui-disconnect-safe.md)）。
- 纯 Rust：不使用 Node / Bun，不嵌入 JavaScript Runtime。
- 协议先行：GUI Connection Protocol 必须先冻结，Rust 类型是唯一 schema source，自动生成 TypeScript 类型。
- 可重放：所有 Agent 事件可持久化、可重放，崩溃后可恢复。
- 解耦：Agent Engine 与 Provider 通过 canonical domain 解耦，禁止按 Provider 名称走特例。
- 控制面分离：Provider protocol、Credential Pool、RoutingPolicy、Agent scheduling 与 ClientAdapter 是不同状态机；`ModelProvider` 不承担账号池或客户端职责（[ADR-033](../adr/ADR-033-control-plane-separation.md)）。
- Tenant 边界：Session/Agent/Account/Usage/Audit 都有 tenant scope；本地单用户默认映射到 `local/default`，跨 tenant 查询和 binding fail-closed。
- Host 唯一：`pawork` 是 Core 唯一正式宿主；GUI / IDE / ACP / SDK / Mobile 都是连接该宿主的 Client Channel / Host Adapter，并列存在，不构造第二 Core、不互相替代（[ADR-021](../adr/ADR-021-cli-core-same-process.md)、[ADR-025](../adr/ADR-025-cli-is-sole-host.md)、[ADR-030](../adr/ADR-030-core-sole-source-of-truth.md)）。
- 大数据用引用：大型内容走 Blob Store / Artifact ID，不在事件里内联数 MB 数据。
- 敏感制品隔离：reasoning 等敏感凭证走 Protected Blob Store 加密落盘，Event 只存安全引用，不内联、不入日志、不入 OS Keychain（[ADR-032](../adr/ADR-032-protected-blob-store.md)）。

## 4. 依赖方向

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

Phase 15–19 在该主干上按以下方向扩展，箭头表示“上层依赖下层”；组合统一发生在 `app-service` / `core-runtime`，不会形成第二宿主：

```text
provider-api ← provider-* / memory-service
      ↑
agent-engine ← plan-service / goal-service / task-manager / automation-service
      ↑
core-runtime ← protected-blob-store / user-hooks / plugin-package / lsp-runtime
      ↑
app-service ← gui-server / client-adapter-api / headless-json / ide-host-adapter / remote-control-adapter
                   ↑
             GUI / SDK / IDE / Codex / Claude / ACP / Mobile clients

apps/desktop → gui-client → transport-api → gui-server → app-service
```

`http-runtime` 是 Provider、User Hooks、Marketplace 与 Forge Adapter 共享的无 Provider 通用网络底层；`agent-sdk` 只依赖公开 schema/framing 并连接 `pawork`，不依赖 `core-runtime`。完整规划 crate 清单与依赖方向见 [workspace 结构 §2.1](workspace-layout.md)。

必须禁止循环依赖。`agent-domain` 不得依赖 Tauri、SQLite、HTTP Client、OS Keychain、Git、具体 Provider。

详见 [workspace 结构](workspace-layout.md) 与 [ADR-002](../adr/ADR-002-agent-engine-provider-decoupled.md)。

## 5. 与 Pi 的关系

| 维度 | 决策 |
| --- | --- |
| 运行时 | 不兼容，纯 Rust 重写 |
| 扩展机制 | 用 MCP、WASM、声明式资源替代 TypeScript Extension |
| Session | 仅支持导入 Pi JSONL（[ADR-005](../adr/ADR-005-pi-jsonl-import-only.md)） |
| 资源概念 | 兼容 `AGENTS.md`、Skills、Prompt |
| 配置迁移 | 提供模型 / Provider 配置迁移工具 |
| 差分测试 | 可用 Pi 作参考行为，不作为运行时依赖 |

## 6. 关键风险概览

- Provider 适配工作量（Tool Call / Thinking / Image / Cache / Usage / OAuth 差异大）
- Compaction 品质（影响 Agent 是否遗忘约束）
- Shell 跨平台（Windows / macOS / Linux 的 Shell、PTY、路径、进程树）
- 插件生态重建（放弃 TS Extension 后早期生态为空）
- 范围过大（必须严格按关键路径推进）

缓解对策见 [ROADMAP](../../ROADMAP.md) 的风险监控章节与各功能文档。

## 7. 技术选型要点

- 存储：SQLite Event Store + Materialized Projections + Content-addressed Blob Store（[ADR-003](../adr/ADR-003-sqlite-event-store.md)、[ADR-004](../adr/ADR-004-blob-store.md)）。
- Git：第一版调用系统 Git（[ADR-007](../adr/ADR-007-system-git.md)），非完全依赖 libgit2。
- 扩展：MCP 第一外部扩展机制（[ADR-011](../adr/ADR-011-mcp-first-extension.md)），WASM 第一代码插件机制（[ADR-012](../adr/ADR-012-wasm-first-plugin.md)），不公开 Native dylib API（[ADR-013](../adr/ADR-013-no-native-dylib-plugin.md)）。
- Secret：API Key / OAuth Token 等小型用户凭证存 OS Keychain（[ADR-014](../adr/ADR-014-secret-os-keychain.md)），不落库明文；reasoning continuation 等高频敏感制品走 Protected Blob Store（[ADR-032](../adr/ADR-032-protected-blob-store.md)），不写 Keychain。
- 写操作：建立 Checkpoint 可回滚（[ADR-010](../adr/ADR-010-checkpoint-all-writes.md)），默认启用 Workspace Trust（[ADR-009](../adr/ADR-009-default-workspace-trust.md)）。

## 8. 相关文档

- [workspace 结构](workspace-layout.md)
- [领域模型](domain-model.md)
- [控制流](control-flow.md)
- [GUI Connection Protocol](api-surface.md)
- [GUI 连接与多客户端](../features/gui-connection.md)
- [Desktop GUI](../features/desktop-gui.md)
- [Provider Account Control Plane](../features/provider-control-plane.md) · [Agent Client Adapters](../features/client-adapters.md) · [Tenant、Usage 与 Audit](../features/tenant-audit.md)
- [CLI Host](../features/cli-host.md)
- [ROADMAP](../../ROADMAP.md)
