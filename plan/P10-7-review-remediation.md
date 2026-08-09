# P10-7：Phase 10 评审修复（REVIEW remediation）

> Phase 10 · WASM Plugin（plugin-api / wasm-plugin-host / hook-runtime）· 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P10-1 ~ P10-6

**最终目的**：执行 [docs/review/p10-review.md](../docs/review/p10-review.md) 的「减少」导向建议——删死 pub API、合并重复校验、收敛 Lifecycle 双路径，让 P13 接线时面对一个更小、门禁单一、没有死概念与语义分叉的插件子系统。零端到端消费者（§2）、4 个预留 `PluginCapability` 变体（§3.7）、3 个死 lifecycle 事件 `Load/Register/Unload`（§3.8）、Drop 不排空在途调用（§4.3b）、epoch 微优化（§4.3c）、`PluginStateStore` trait 上移（§4.5）均属 P13/P17 接线才可观测/决策的能力，按评审结论显式延后，不在本任务范围。

**涉及范围**：`plugin-api`（manifest.rs/invocation.rs）、`wasm-plugin-host`（host.rs/config.rs/registry.rs/trust.rs/tests/host_wat.rs）、`docs/architecture/workspace-layout.md`、`docs/features/plugins.md`

## 处置策略（按评审 §5 / §6 矩阵）

- **现在修复（落地）**：§3.1 死 pub API 删除（registry/trust 查询方法）；§3.2 invoke_operation 重复 input 预检；§3.3 on_lifecycle_event 内联快照闸门；§3.4 `qualified_name`；§3.5 `plugin_context`；§3.6 `into_tool_registry`；§4.1 Lifecycle 双路径合并为单一 `invoke_with_state`（吃掉 §3.2/§3.3）；§4.3a `HostConfig::validate` 约束；§3.9/§4.4 文档不一致。
- **显式延后**：§2 零端到端消费者（P13-1/P13-2 首次通电）；§3.7 四个预留 `PluginCapability`（P13 规划明确后决策去留）；§3.8 `Load/Register/Unload` 死 lifecycle 事件（P17-2 package 生命周期落地时补发或删除）；§4.3b Drop 不排空在途调用（接入时若 host 被 Arc 共享则处理）；§4.3c epoch deadline invoke 不重置的微优化（接入时）；§4.5 `PluginStateStore` trait 上移 plugin-api（durable backend 落地时，需先解耦 HostConfig 配额）。
- **不建议改动（做对的部分）**：§4.2 ExternalPlugin 不重写调度器门禁（P9 review 要求的 canonical 路径）；签名绑定模型；空 Linker + 资源限额；状态隔离；hook-runtime 故障隔离；WIT + golden + guest binding 三重门禁。

## 细分步骤（分组）

### A. 死 pub API 删除（§3.1/§3.4/§3.5/§3.6，零风险）

1. **`PluginManifest::qualified_name`**（manifest.rs）：与 `registry::external_tool_name` 同 `{id}::{name}` 格式、仅 1 个测试调用。删除方法 + 测试 `host_namespaces_plugin_registrations`。
2. **`PluginOperation::plugin_context`**（invocation.rs）：仅 1 个测试调用，生产只走 `state_scope()`。删除方法 + 测试 `command_exposes_plugin_context` + 悬空 `CoreInstanceId` 测试 import。
3. **`NamespacedToolRegistry::{into_tool_registry, len, is_empty}`**（registry.rs）：生产用 `to_tool_registry`；len/is_empty 0 外部调用。删除；`host_wat.rs` 两处 `into_tool_registry()` 改 `to_tool_registry()`。
4. **`PluginCommandRegistry::{contains, is_empty, len}`**（registry.rs）：0 调用。删除；`host_wat.rs` 一处 `commands.len()` 改 `commands.names().len()`；本文件测试 `registry.len()` 改 `registry.names().len()`。
5. **`TrustStore::{install([u8;32]), contains, get, len, is_empty, remove}`**（trust.rs）：测试与生产都用 `install_verifying_key`。删除 6 个方法，保留 `new/install_verifying_key/verify_signature`。
6. **不删（评审 §3.1 延后到 P13）**：`WasmPluginHost::{api_version, config, trust_store}`——接线层装配可能需要读取，0 调用但保留；`state_store()` 有 5 处测试调用。

### B. Lifecycle 双路径合并（§4.1 + §3.2 + §3.3，接入前唯一架构决策）

7. **新增 `LoadedPlugin::invoke_with_state`**（host.rs，~30 行）：统一 invoke 事务 `operation_lock → ensure_active → state_snapshot → 序列化 → invoke_checked → 解析 PluginInvocationOutput → apply_state_mutations`。`invoke_checked` 已含 input 长度检查（吃掉 §3.2 重复预检）；`state_snapshot` 函数复用（吃掉 §3.3 内联快照闸门）。
8. **`on_lifecycle_event` 委托**（路径 A）：`ensure_active` → 未声明事件 no-op `Ok(())`（保留路径 A 契约）→ `invoke_with_state(Lifecycle{event,context}, scope, cancel)` → `Success=>Ok / Error=>Err`。
9. **`invoke_operation` 委托**（路径 B）：`get` → `enforce_operation_capability` → `enforce_operation_registration`（保留路径 B `PermissionDenied` 契约）→ `invoke_with_state(operation, scope, cancel)`。
   - `operation_lock` 只在 `invoke_with_state` 内取一次（tokio Mutex 不可重入，避免死锁）；`enforce_*` 读 `Arc<PluginManifest>`（共享不可变）不需锁、置于锁外。

### C. HostConfig 超时确定性（§4.3a）

10. **`HostConfig::validate`**（config.rs）：`epoch_tick.is_zero` 检查后新增 `invoke_timeout < epoch_tick → Err(TimeoutSmallerThanTick)`（新错误变体）。违反时紧循环插件超时最坏晚一个 tick；默认 5s/10ms 满足。补单元测试 `timeout_smaller_than_tick_is_rejected`。

### D. 文档一致性（§3.9/§4.4）

11. **workspace-layout.md**：§6 依赖图 `plugin-host → wasm-plugin-host`（§3.9）；§2 crates 表 agent-engine 行去掉超前声明的 `plugin-api`（§4.4，已核对 `crates/agent-engine/Cargo.toml` 实际依赖）。
12. **plugins.md**：「插件能力」清单逐项标注实现状态：P0 已实现（注册工具/命令/lifecycle/状态）、P2 预留（ModifyContext/CompactionStrategy/RegisterProvider/UserInteraction）、预留（受限文件/网络，host Linker 默认不注入 WASI import，见 ADR-012）、P17-2/P16-6（Monitor）。

## 主要产出物

- 删除：`PluginManifest::qualified_name`、`PluginOperation::plugin_context`、`NamespacedToolRegistry::{into_tool_registry,len,is_empty}`、`PluginCommandRegistry::{contains,len,is_empty}`、`TrustStore::{install,contains,get,len,is_empty,remove}`，及对应唯一测试。
- 合并：`LoadedPlugin::invoke_with_state` 统一两条 invoke 路径（净 −12 行 host.rs），消除 §3.2 重复预检与 §3.3 内联快照闸门；语义统一为 snapshot→invoke_checked→apply 单一事务。
- 加固：`HostConfig::validate` 新增 `invoke_timeout >= epoch_tick` 约束 + `TimeoutSmallerThanTick` 变体 + 测试。
- 文档：workspace-layout.md 两处不一致修正；plugins.md 能力清单标注实现状态。
- 整体 diff：9 文件、+87/−188，净 −101 行（占 P10 四 crate 源码约 4%，与评审预估一致）。

## 验收标准

- [x] **§3.1/§3.4/§3.5/§3.6 死 API**：6 类方法全部删除，`rg` 全 workspace 零残留（tool-runtime/policy-engine 同名 `UserInteraction` 属不同类型，不计）
- [x] **§4.1 Lifecycle 双路径**：单一 `invoke_with_state`；路径 A 未声明事件仍 `Ok(())` no-op，路径 B 未声明仍 `PermissionDenied`；`operation_lock` 单次获取无重入死锁；hook-runtime 端到端测试 `loaded_plugin_dispatches_through_hook_runtime`/`lifecycle_event_routes_through_loaded_plugin_trait` 通过
- [x] **§3.2/§3.3 重复消除**：input 预检由 `invoke_checked` 统一承担，内联快照闸门改调 `state_snapshot`
- [x] **§4.3a 超时约束**：`invoke_timeout < epoch_tick` 被 `validate()` 拒绝；默认配置通过
- [x] **§3.9/§4.4 文档**：workspace-layout.md 依赖图 crate 名与 agent-engine 依赖行与 `Cargo.toml` 事实一致；plugins.md 能力清单标注实现状态
- [x] **显式延后**：§2/§3.7/§3.8/§4.3b,c/§4.5 在本文与 plugins.md 标注归属阶段，未误标完成

## 验证记录（2026-08-10）

- `cargo test -p wasm-plugin-host --lib`：8 passed / 0 failed（含新增 `timeout_smaller_than_tick_is_rejected`）。
- `cargo test -p wasm-plugin-host --test host_wat`：29 passed / 4 filtered（`fuel_exhaustion_is_reported`/`memory_growth_is_rejected`/`invoke_timeout_aborts_loop`/`cancellation_aborts_loop`）。这 4 个 trap 测试是 **Windows debug 下 wasmtime 27「panic in a function that cannot unwind」直接 abort 测试进程的既有问题**，与本次改动无关——Commander 用 `git stash` 在未含本次改动的基线上逐个复现（5/5 确定性），均在同一点 `STATUS_STACK_BUFFER_OVERRUN` 失败。整体 run 会在首个 trap 测试处终止，故验证用 `--skip` 跑完其余 29 项。该既有问题建议在接入 P13 前由独立任务处理（release 构建或 catch_unwind 包装）。
- `cargo test -p plugin-api`：15 passed（13 单元 + 2 v1_contract）/ 0 failed。
- `cargo clippy -p wasm-plugin-host -p plugin-api -p hook-runtime --all-targets -- -D warnings`：通过，0 警告。
- `cargo fmt -p wasm-plugin-host -p plugin-api -p hook-runtime -- --check`：通过。
- 按本任务门禁节奏只执行受影响 crate 的定向门禁；workspace 全量、三平台与发布门禁留待 Core 主干 L2/L3。
- **独立 reviewer 复核**（deepseek_reviewer）：见 p10-review.md 修复记录章节。按 reviewer 建议在路径 B 的 capability/registration 校验前补一次 ensure_active()，保留 NotLoaded 优先于 PermissionDenied 的原始错误优先级，消除 unload 窗口下的角点行为翻转。

**相关文档**：[REVIEW.md](../REVIEW.md) §Phase 10 · [docs/review/p10-review.md](../docs/review/p10-review.md) · [docs/features/plugins.md](../docs/features/plugins.md) · [ROADMAP Phase 10](../ROADMAP.md)

> 跨任务协调：本任务写集限定 plugin-api（manifest/invocation）、wasm-plugin-host（host/config/registry/trust + host_wat 测试）、两篇文档，不与工作区其他未提交改动（resource-loader P8-9、mcp-client P9-8）的写集重叠；基线表无新增/移除依赖。

> 延后项归属：§2（首次端到端验证）、§3.7（4 个预留 capability）、§3.8（Load/Register/Unload 死事件）、§4.3b/c（Drop/epoch 微优化）、§4.5（PluginStateStore trait 上移）统一在 P13 接线 / P17-2 package 生命周期 / durable backend 落地时处理，届时优先验证本评审标记的各处。