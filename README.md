# Pawork

> 纯 Rust 编码智能体核心平台 —— CLI 与 Core 同进程同二进制，无 Node / Bun / JavaScript Runtime。

Pawork 用 Rust 从零实现一个编码智能体（Coding Agent）平台核心。它以 Pi 的功能、工作流与交互习惯为参考，但**不复用**其 TypeScript 实现：**CLI 与 Rust Core 是同一个程序和进程边界**（二进制 `pawork`），Phase 19 的 Tauri + React GUI 作为独立进程，经 CLI 暴露的 GUI Connection Protocol 连接 Core。

## 项目定位

Pawork 不是「Pi 的桌面壳」，而是一个独立的 Rust Coding Agent 平台。Pi 仅作为功能参考、行为参考与迁移数据来源。

## 设计目标

- 多模型 Provider Runtime
- 完整 Agent 循环（流式、工具调用、审批、重试、取消）
- 会话、分支、恢复与压缩
- 上下文构建与 Token 预算
- 文件读写、编辑、搜索与命令执行
- 权限、审批与 Sandbox
- Git、Worktree、Diff 与回滚
- Skills、Prompt 与项目指令
- MCP 与 WASM 插件
- 多 Agent 调度
- Provider Account / Credential Lease、确定性路由、Tenant/Usage/Audit 控制面
- Codex App Server、Claude Gateway、ACP 等外部 Agent Client Adapter
- 为 GUI 提供稳定的 CLI/Core 宿主与接入协议
- 独立 Tauri + React Desktop GUI（Timeline、Composer、Diff、Terminal、Settings 与 Workflow）
- `pawork` CLI 是 Core 的唯一正式宿主，可脱离 GUI 独立运行
- 一个 CLI/Core 实例可同时服务多个本地与远程 GUI

## 不追求的兼容性

- Pi TypeScript API / Extension API 兼容
- npm 插件兼容
- Pi 内部类名、事件名兼容
- Provider SDK 行为逐行复刻

## 架构总览

```text
┌──────────────────────────────────────────────┐
│ CLI + Rust Core（同一进程，二进制 pawork）     │
│  CLI Commands / Renderers   GUI Server        │
│  Agent Engine          Provider Runtime       │
│  Agent Supervisor      Account Control Plane  │
│  Context Engine        Tool Runtime           │
│  Session Store         Policy Engine          │
│  Workspace Service     Git / Diff             │
│  Plugin / MCP Host     Artifact Store         │
│  Auth / Models         Tenant / Usage / Audit │
└────────────────────┬─────────────────────────┘
            │ GUI Connection Protocol（Local / Remote Transport）
┌───────────▼─────────┐  ┌─────────────────────┐
│ Local GUI A         │  │ Remote GUI C/D      │
│ Tauri + React       │  │ Tauri + React       │
└─────────────────────┘  └─────────────────────┘
```

核心原则：

- 不使用 Node / Bun，不嵌入任何 JavaScript Runtime
- 不启动 Pi Sidecar，不依赖 `@earendil-works/pi-*`
- 不实现 TUI
- `pawork` CLI 是 Core 的唯一正式宿主；GUI 经协议连接 CLI，不嵌入 Core
- 一个 CLI/Core 实例可同时服务多个本地与远程 GUI；GUI 断线不影响任务

详见 [docs/architecture/overview.md](docs/architecture/overview.md)。

## 仓库结构

```text
Pawork/                       # 仓库根 = Cargo workspace 根
├── crates/                   # 核心 crate
├── apps/                     # 可执行入口（pawork、protocol-test-gui、desktop）
├── schemas/                  # JSON Schema（core-api / gui-protocol / events / transport / authentication / plugin-api / mcp / import）
├── fixtures/                 # 测试夹具
├── benches/                  # 性能基准
└── docs/                     # 架构、ADR、功能、质量文档
```

> 以上为规划结构，目录在 [P0-1](plan/P0-1-workspace-skeleton.md) 创建。

完整结构见 [docs/architecture/workspace-layout.md](docs/architecture/workspace-layout.md)。

## 文档导航

| 类别 | 内容 |
| --- | --- |
| 总体架构 | [overview](docs/architecture/overview.md) · [workspace 结构](docs/architecture/workspace-layout.md) · [领域模型](docs/architecture/domain-model.md) · [控制流](docs/architecture/control-flow.md) · [GUI Connection Protocol](docs/architecture/api-surface.md) |
| Control Plane / Client | [Provider Account Control Plane](docs/features/provider-control-plane.md) · [Tenant、Usage 与 Audit](docs/features/tenant-audit.md) · [Agent Client Adapters](docs/features/client-adapters.md) |
| CLI Host / GUI 接入 | [CLI Host](docs/features/cli-host.md) · [GUI 连接与多客户端](docs/features/gui-connection.md) · [Desktop GUI](docs/features/desktop-gui.md) |
| 功能模块 | [docs/features/](docs/features/) |
| 质量门槛 | [性能目标](docs/quality/performance-targets.md) · [安全验收](docs/quality/security-acceptance.md) · [测试体系](docs/quality/testing.md) |
| 架构决策 | [docs/adr/](docs/adr/)（ADR-001 ~ ADR-034） |
| 术语 | [glossary](docs/glossary.md) |
| 路线图 | [ROADMAP.md](ROADMAP.md) |

## 项目状态

Phase 0 已完成；Phase 1～7 的主体实现已落地，当前优先处理七个 review remediation（从 P1-13 开始）与 P11-1 沙箱骨架。Provider Native、Modern Workflow、Ecosystem/Host、Account Control Plane & Client Adapters 与 Desktop GUI（Phase 15～19）仍处于规划阶段。任务完成度和下一项工作以 [ROADMAP](ROADMAP.md) 的实时进度表为准，不再用 README 固定阶段文字替代源码与路线图事实。

## 贡献

- 工作约定见 [AGENTS.md](AGENTS.md)
- 新增 crate、命名与依赖规则见 [workspace 结构](docs/architecture/workspace-layout.md)
- 架构决策须以 ADR 记录，见 [docs/adr/](docs/adr/)

## 许可证

待定（将在首次实现提交前确定）。
