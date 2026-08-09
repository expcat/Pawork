# P11-1：NativeRestricted backend

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P4-9、P4-12

**最终目的**：建立 `sandbox-runtime` crate 并实现 `NativeRestricted` 软沙箱后端——它是所有平台永远可用的兜底沙箱，把 Agent 调度的子进程约束在 workspace 路径、清洗后的环境与有限资源内，并为后续平台原生硬隔离后端（P11-2/3/4）冻结统一的 `SandboxBackend` trait 与 `SandboxPolicy` 契约。完成后，命令执行从「策略决定是否运行」升级为「以受控边界运行」，且在无任何平台原生沙箱依赖时仍可用。

**涉及范围**：`sandbox-runtime`（新增）、`policy-engine`（ExecutionConstraints → SandboxPolicy 归一化）、`builtin-tools`（run_command 接入沙箱）

## 细分步骤

1. **创建 sandbox-runtime crate** —— 目的：为沙箱提供归属 crate，冻结 `SandboxBackend` trait、`SandboxPolicy`、`SandboxProcessSpec`、`SandboxProcess`、`SandboxError`、`SandboxSelector` 类型骨架（见 [sandbox](../docs/features/sandbox.md)）；登记到 `Cargo.toml` members 与 [workspace-layout §2](../docs/architecture/workspace-layout.md)。依赖 `process-runtime`、`policy-engine`、`agent-domain`，本任务不引入新第三方依赖。
2. **NativeRestricted 后端** —— 目的：实现永远可用的软沙箱。机制：`env_clear` + `environment` 白名单/黑名单（复用 run_command 现有 ENV_ALLOWLIST 思路）、`cwd` 锁定 workspace、`SandboxPolicy.filesystem` 路径收敛（read_roots/write_roots/deny，复用 `policy-engine::resolve_workspace_path`）、`resources` 经 `rlimit`（Unix）/内存与时间预算约束、`process.max_procs` 软提示。网络：`network.mode` 在此后端降级为 `Hint`（仅记录，不强制）。
3. **ExecutionConstraints → SandboxPolicy 归一化** —— 目的：让 policy-engine 的裁决无缝驱动沙箱。新增归一化构造，把 timeout_ms/max_output_bytes 映射到 `SandboxPolicy.resources`，`ToolCapability::Network` 映射 network.mode，未信任工作区映射最小权限默认策略。
4. **SandboxSelector 探测与回退框架** —— 目的：确立「尝试硬隔离 → 回退 NativeRestricted」的可观测契约。`pick()` 已接入 macOS Seatbelt、Linux bwrap/Landlock 与 Windows Job-only，并通过结构化 `BackendSelection` 暴露实际隔离等级、全部探测与回退原因。
5. **run_command 接入沙箱** —— 目的：让真实工具走沙箱路径。改为经 `SandboxSelector`/`SandboxBackend::spawn` 执行；env 白名单下沉为 `SandboxPolicy.environment`；既有 timeout/cancel/超大输出/进程树终止语义不回退（复用 process-runtime 内部）。

## 主要产出物

- `sandbox-runtime` crate（trait + 类型 + NativeRestricted 后端 + Selector 框架）
- `policy-engine` → `SandboxPolicy` 归一化
- `run_command` 经沙箱执行
- L0/L1 测试（策略构造、NativeRestricted 软限制生效）

## 验收标准

- [x] `sandbox-runtime` 编译通过并登记到 workspace members 与 workspace-layout
- [x] NativeRestricted 后端永远可用：无平台硬后端时仍能运行命令并施加 env/cwd/资源软限制，同时明确报告网络不能硬隔离
- [x] `ExecutionConstraints` 能归一化为 `SandboxPolicy` 资源基线
- [x] 回退框架就位且可观测（Tool metadata 含 backend/isolation/fallback/note/attempted）
- [x] run_command 经沙箱执行后，流式输出、timeout/cancel、输出预算与进程树清理不回退
- [x] Windows 原生与 Linux WSL/musl L0/L1 通过，macOS target 编译通过

## 验证记录（2026-08-09）

- `sandbox-runtime`、`process-runtime`、`policy-engine` 与 `builtin-tools` 定向测试通过；Windows `run_command` 46 tests、sandbox 29 tests、process 8 tests。`run_command` 网络固定 fail-closed，旧网络请求仅审计；CPU/memory/fd/max-process/wall/output 均有默认值与硬上界。
- `x86_64-unknown-linux-gnu` 与 `aarch64-apple-darwin` 目标检查通过；Linux musl 测试包在 WSL 真实运行通过。
- 选择器探测结果按进程缓存，Native/降级路径均返回可序列化的实际隔离等级。

## 后续增强 / Maintenance Tasks

> 本节子任务为 P11-1 完成后的增量演进（Enhancement），针对当前源码事实中的两处简化——`IsolationLevel` 为 4 值 enum、`SandboxSelector::pick()` 为单后端选择——逐步演进。统一状态：🟡未开始 · 交付成熟度：Designed；不改变 P11-1 主任务 🟢 状态，保持向后兼容、不 big-bang rewrite，本阶段不落功能代码。平台能力事实查询日期：2026-08-09。

### P11-1.E1 Sandbox Guarantee Model

> Phase 11 · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P11-1

**最终目的**：`IsolationLevel`（Soft / Hard / HardFilesystemOnly / Degraded）目前只能表达摘要等级，安全敏感调用方无法获知「具体拿到了哪些隔离保证」。本子任务设计多维 `SandboxGuarantees` 模型（filesystem / network / process_tree / process_namespace / resource_limits / ipc_scope / syscall_filter / kernel_boundary 等维度，字段命名与现有代码设计对齐），让调用方按维度查询实际 capability/guarantee；`IsolationLevel` 保留作 UI/telemetry 摘要，真正安全判定查 guarantee，metadata/tracing 表示「要求了什么 vs 实际获得什么」，降级明确到维度。

**涉及范围**：`sandbox-runtime`（guarantee 模型、查询与序列化的设计；本阶段仅文档，不改功能代码）。

**依赖**：P11-1（`SandboxBackend` trait、`SandboxPolicy`、`BackendSelection`、probe 结果）。

**产出物**：

- `SandboxGuarantees` 多维模型设计：各维度字段、与 `SandboxPolicy` / `BackendSelection` 的映射关系
- 维度级降级可观测方案：metadata/tracing 记录 required vs actual 及降级原因
- 向后兼容说明：现有调用方零改动，默认仍消费 `IsolationLevel` 摘要

**验收标准**（设计验收）：

- [ ] 安全敏感判断路径明确以 guarantee 为准，`IsolationLevel` 仅作摘要，不再作唯一安全判定依据
- [ ] 每个维度可表达 required / actual 与降级原因，任何平台不把未实现保证报成 hard
- [ ] 现有调用方无需重构即可继续工作，不破坏 `run_command` 等既有接口
- [ ] 字段命名与现有 `SandboxPolicy` / `BackendSelection` 命名一致，可序列化进 metadata

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ADR-031 沙箱后端架构](../docs/adr/ADR-031-sandbox-backend-architecture.md) · [ROADMAP](../ROADMAP.md)

### P11-1.E2 Policy-aware Sandbox Planning(设计任务)

> Phase 11 · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P11-1.E1

**最终目的**：评估 `SandboxSelector::pick()`（当前单后端选择）是否演进为 `plan(policy, requirements)` 组合 enforcement layer，或仅新增 `BackendCapabilities` 已足够；明确哪些能力属 backend、哪些属可组合 enforcement layer、哪些由 ProcessRuntime 统一承担，并保持 `run_command` 兼容。不删除 `SandboxBackend` trait，不 big-bang rewrite。

**涉及范围**：`sandbox-runtime`（selector/planning 演进评估与设计；本阶段仅文档，不改功能代码）。

**依赖**：P11-1、P11-1.E1（`SandboxGuarantees` 作为 plan 的输入需求与输出契约）。

**产出物**：

- `pick()` → `plan(policy, requirements)` 演进评估结论，含「仅 `BackendCapabilities`」备选方案对比
- backend / enforcement layer / ProcessRuntime 三方责任划分
- 选择理由与拒绝的替代方案记录
- `run_command` 兼容性说明与迁移路径

**验收标准**（设计验收）：

- [ ] 给出明确演进结论与理由，记录被拒绝的替代方案
- [ ] `SandboxBackend` trait 保留，现有后端与 `run_command` 无需破坏性改动
- [ ] 平台能力探测保留且可观测（不静默移除 capability probe）
- [ ] 网络边界统一：Sandbox Runtime 负责 direct network containment（deny direct / allow port / allow proxy），hostname/domain/URL policy 归未来统一 egress broker/proxy，OS sandbox 仅允许访问该 broker，不做 DNS→IP 静态映射
- [ ] 不因统一 API 放弃已工作的 OS-native primitive；能力差异经降级维度可观测

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [ADR-031 沙箱后端架构](../docs/adr/ADR-031-sandbox-backend-architecture.md) · [ROADMAP](../ROADMAP.md)

**相关文档**：[sandbox](../docs/features/sandbox.md) · [policy](../docs/features/policy.md) · [process](../docs/features/process.md) · [ADR-031 沙箱后端架构](../docs/adr/ADR-031-sandbox-backend-architecture.md) · [ROADMAP](../ROADMAP.md)
