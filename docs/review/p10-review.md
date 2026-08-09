# Phase 10 Review：WASM Plugin（plugin-api / wasm-plugin-host / hook-runtime）

> 审查范围：`crates/plugin-api`（P10-1/6）、`crates/wasm-plugin-host`（P10-2/3/4/5）、`crates/hook-runtime`（P10-3）、`crates/test-support::plugin_contract`（P10-6）及其与 `tool-runtime` / `policy-engine` / `tool-api` / `agent-domain` 的接线。
> 方法：Commander 统筹 + 3 个 `deepseek_explorer` 并行调查（内部结构与冗余 / 运行时质量与接线 / canonical 一致性与边界），结论由 Commander 复核合并并独立验证关键事实。
> 性质：**只 Review，不改实现。**

---

## 0. 一句话结论

Phase 10 的实现质量明显高于同期「叶子 crate 等待接线」家族（P8 resource-loader、P9 mcp-client）。最值得肯定的一点是：**它没有重蹈 P9 的覆辙**——`ExternalPluginToolAdapter` 不在 adapter 内重写 `PolicyEngine`/`ApprovalResolver` 门禁，把审批完全委托给 canonical 调度器，正是 [p9-review.md](p9-review.md) §4.1 要求的正确路径。签名绑定（manifest canonical JSON + component blake3）、每插件独立 Store、空 Linker、fuel/epoch/wall-clock/cancel、乐观 revision + 配额的状态隔离、确定性派发 + panic 隔离——ADR-012 的安全不变量全部落地且有定向测试背书。

与 P8/P9 同构的背景仍然存在：整个插件子系统**零端到端消费者**，正式接线（注入 `app-service`/`core-runtime`、由 `pawork` 管理生命周期）属 P13-1/P13-2。因此本 Review 的重点不是「是否已生效」，而是「在接入前这些抽象是否最小、是否会在接入时制造不一致」。

真正值得记录的是**两类可削减项**：(1) 一批纯冗余（死 pub API、重复预检、复制粘贴的快照闸门、双重 `qualified_name`）合计约 60 行，删改零风险；(2) Lifecycle 存在两条语义分叉的派发路径（约 40 行重复），是接入前必须收敛的唯一架构决策点。另有 4 个无人消费的 `PluginCapability` 变体与 3 个无人构造的 lifecycle 事件（`Load`/`Register`/`Unload`），属过早预留。按「优先减少代码、模块、接口、概念」的取向，前一类应现在做，后一类随 P13 接线决策。

核心建议方向：**减少**——删死抽象、合并双路径、退役预留变体，而不是在接入前继续往 host 里堆新能力。

---

## 1. 设计符合度

| 子任务 | plan 目标 | 实现位置 | 符合度 | 备注 |
|---|---|---|---|---|
| P10-1 Manifest + 签名 | manifest 字段、Ed25519 验签、版本兼容 | `plugin-api/src/manifest.rs` + `wasm-plugin-host/src/trust.rs` | ✅ 符合 | canonical signing payload 域分离 + 稳定 JSON + component BLAKE3；未知 key/畸形签名/篡改 fail closed，测试覆盖完整 |
| P10-2 WASM Host | component model、load/unload、崩溃隔离 | `wasm-plugin-host/src/host.rs::WasmPluginHost` | ✅ 符合（最大亮点） | 真实 inline Component WAT 覆盖 load/invoke/并发同 ID/async unload/trap 隔离，不过度 mock |
| P10-3 注册 + hook | 工具/命令注册、lifecycle hook 派发 | `registry.rs` + `hook-runtime` + `runtime.rs::PluginRuntime` | ⚠️ 符合但 Lifecycle 双路径 | 见 §4.1：两条 invoke 路径语义分叉 |
| P10-4 Plugin state | 跨调用保存、scope 隔离、配额 | `state.rs::PluginStateStore` + `InMemoryPluginStateStore` | ✅ 符合 | 乐观 revision + 单值/键数/总量配额 + PersistentState 闸门；snapshot→invoke→apply 串行事务 |
| P10-5 Capability/资源 | capability 检查、fuel/memory/时间、默认无 WASI | `host.rs` Linker 空 + `config.rs::HostConfig` | ✅ 符合 | Linker 零 import 实测确认；越权 import 实例化期拒绝；资源耗尽只终止本次调用 |
| P10-6 API 版本兼容 | 版本矩阵、golden、WIT binding | `test-support/src/plugin_contract.rs` + `schemas/plugin-api/pawork-plugin-v1.wit` | ✅ 符合 | 11 条矩阵覆盖 exact/caret/tilde/range/minor/跨 major/prerelease；WIT 事实源 + JSON golden + guest binding 三重门禁 |

**判定**：6 个子任务全部达成，🟢 与实现一致，ADR-012 / ADR-013 红线**结构上合规**（plugin-api 是纯协议层无 wasmtime；wasm-plugin-host 是叶子 crate；Agent Engine 只经 canonical `AgentTool`/`ToolDescriptor` 接触插件工具，不感知 plugin id；签名/状态/隔离不变量到位）。唯一需要在接入前处理的是 P10-3 的 Lifecycle 双路径分叉（§4.1）。

---

## 2. 零端到端消费者（背景，非缺陷）

与 P8/P9 同型的结论，由 3 个独立调查路径一致确认并经 Commander 复核：

- **crate 依赖链**：全 workspace 除 `wasm-plugin-host → hook-runtime + plugin-api`、`test-support → plugin-api` 外，没有任何 crate 依赖三者。`apps/pawork`、`cli-host`、`app-service`、`core-runtime`、`agent-engine`、`builtin-tools` 全部零引用（`rg WasmPluginHost|PluginRuntime|plugin_api|hook_runtime` 仅命中 4 个 P10 crate 与 test-support）。
- **构造点全部在测试**：`WasmPluginHost::new` 与 `PluginRuntime::new` 仅出现在 `wasm-plugin-host/tests/host_wat.rs`。
- **`agent-engine` 实际不依赖 `plugin-api`**：`crates/agent-engine/Cargo.toml` 只有 `tool-api`/`tool-runtime`，与 [workspace-layout.md](../architecture/workspace-layout.md) L26 声称的「依赖 plugin-api」冲突（文档超前，见 §4.4）。

**含义**（与 P8/P9 一致）：

1. P10 全部行为正确性目前**只能由单元/集成测试背书**，无法由真实运行路径背书。
2. 接线前引入的抽象承担「为 P13/P15/P17 预留契约」的角色；Review 重点放在「预留是否最小、是否会在接入时制造不一致」。
3. 与 P9 对比，P10 的预留明显更克制：没有单变体 enum、没有第二套合并语义、没有 adapter 内重写的门禁管线。冗余集中在「死 pub API」与「双 invoke 路径」两类，删改成本低。

---

## 3. 冗余与过度设计（按可削减量排序）

> 行数以源文件为准（不含测试）。本节为 REVIEW，不执行修改。子代理实测行数：host.rs 777、registry.rs 462、manifest.rs 512、hook-runtime/lib.rs 360、state.rs 301、trust.rs 133、config.rs 134。

### 3.1 死 pub API（全 workspace 零调用点，含测试）

约 47 行 `pub` 方法没有任何消费者，包括内部测试：

| API | 位置 | 行数 | 调用点 |
|---|---|---|---|
| `WasmPluginHost::api_version` / `config` / `trust_store` | host.rs:321/325/329 | 9 | 0 |
| `NamespacedToolRegistry::len` / `is_empty` | registry.rs:102/106 | 6 | 0 |
| `PluginCommandRegistry::contains` / `is_empty` | registry.rs:287/295 | 6 | 0 |
| `TrustStore::install` | trust.rs:54 | 10 | 0（测试全用 `install_verifying_key`） |
| `TrustStore::contains` / `get` / `len` / `is_empty` / `remove` | trust.rs:70-88 | 19 | 0 |

registry/trust 的查询方法可即刻删或降 `pub(crate)`；host 的三个 accessor 建议**等 P13 接线决策**（接线层可能需要读 host 配置/trust store 做装配）。

### 3.2 `invoke_operation` 的 input 预检与 `invoke_checked` 完全重复

host.rs:508-517（10 行）的输入长度预检与 invoke_checked 内 host.rs:114-123 同比较、同错误 kind/message，`invoke_checked` 必然再查一次。删 10 行，风险≈0，立即可做。

### 3.3 `on_lifecycle_event` 内联快照闸门 = `state_snapshot` 逐行复制

host.rs:224-234（11 行）的 PersistentState 读闸门与 host.rs:680-696 的 `state_snapshot` 逐行相同（含 PersistentState 分支与 default 快照）。改为调用 `state_snapshot`，删 10-11 行，零风险。

### 3.4 `qualified_name` 与 `external_tool_name` 双重实现

[manifest.rs:115](../../crates/plugin-api/src/manifest.rs)（`PluginManifest::qualified_name`，3 行，仅 1 个测试调用）与 [registry.rs:24](../../crates/wasm-plugin-host/src/registry.rs)（`external_tool_name`，生产用）输出同一 `{id}::{name}` 格式。删 `qualified_name`，零风险。

### 3.5 `PluginOperation::plugin_context` 死方法

[invocation.rs:36-42](../../crates/plugin-api/src/invocation.rs)（7 行）仅 1 个测试调用，生产路径只走 `state_scope`。删，低风险。

### 3.6 `NamespacedToolRegistry::into_tool_registry` 与 `to_tool_registry` 二选一

`into_tool_registry`（registry.rs:116）仅 2 处测试调用；`to_tool_registry`（registry.rs:123）是生产路径（runtime.rs:159）。删 `into_tool_registry`，零风险。

### 3.7 4 个无人消费的 `PluginCapability` 变体（过早预留）

[manifest.rs:176-185](../../crates/plugin-api/src/manifest.rs) 的 `PluginCapability` 共 **8 个变体**（注：本次复核修正了初判的「9 变体」），P10 生产校验路径只用 4 个：`RegisterTool`（manifest.rs:79）、`RegisterCommand`（:84）、`LifecycleHook`（:89 + host.rs:645 + hook-runtime）、`PersistentState`（host.rs:227/688/711）。

`ModifyContext` / `CompactionStrategy` / `RegisterProvider` / `UserInteraction`（manifest.rs:180-183）**全 workspace 零使用点**，plan/docs 亦无接线引用。`manifest.validate()` 对这 4 个变体没有任何检查路径——插件可以声明它们却得不到任何对应能力。属过早预留：是 ADR 级契约，P13 的 context/compaction/provider 接线可能直接消费，也可能走完全不同的实现。建议**等 P13 规划明确**：若不用则删，避免「声明了却无效果」误导插件作者。

### 3.8 3 个无人构造的 lifecycle 事件（提前抽象）

[plugin-api/src/lib.rs:29-50](../../crates/plugin-api/src/lib.rs) 的 `PluginLifecycleEventKind` 共 19 个变体。Commander 复核确认：`Load` / `Register` / `Unload` 三个变体**全 workspace 零构造点**（`rg PluginLifecycleEvent::(Load|Register|Unload)` 无结果），仅 `kind()` 映射保留它们。`runtime.rs` 的 load/register/unload 路径（:63/:109）都不派发这些事件——插件甚至可以在 manifest 声明订阅 `load` 却永远收不到。`Start`/`Stop` 由 hook-runtime 内部状态机产生（hook-runtime/lib.rs:179-206），是合理的内部事件。

建议：P10 收尾可删这 3 个变体（无 golden/WIT schema 引用，schemas 下仅 invoke 签名）；或到 P17-2 package 生命周期落地时由 host 显式补发。当前它们是带完整 serde 面的死概念。

### 3.9 文档错误

[workspace-layout.md:162](../architecture/workspace-layout.md) 依赖图写 `plugin-host`，实际 crate 是 `wasm-plugin-host`（L68 正确）。1 行，零风险。

---

## 4. 架构问题

### 4.1 Lifecycle 两条语义分叉的派发路径（P1，接入前必须收敛）

这是本次 Review 最重要的发现，也是接入前唯一需要决策的架构点：

- **路径 A（生产实际使用）**：`PluginRuntime::dispatch` → `HookRuntime::dispatch` → `plugin_subscribes_to`（capability + declared 双闸门）→ `LoadedPlugin::on_lifecycle_event`（host.rs:207-266）。
- **路径 B（公开 API，仅测试走 Lifecycle 分支）**：`WasmPluginHost::invoke_operation(PluginOperation::Lifecycle{..})`（host.rs:482-540）。

两条路径各实现一遍 `snapshot → invoke_checked → parse → apply_state_mutations`（每条约 28-39 行重复），且**语义分叉**：

| 行为 | 路径 A（on_lifecycle_event） | 路径 B（invoke_operation） |
|---|---|---|
| 未声明事件 | no-op `Ok(())`（host.rs:214-216） | `PermissionDenied`（enforce_operation_registration，host.rs:656-675） |
| LifecycleHook capability 闸门 | 无（依赖 manifest.validate() 间接保证） | 显式 `enforce_operation_capability`（host.rs:645/495） |
| PersistentState 读闸门 | 内联 11 行（= §3.3 重复块） | 复用 `state_snapshot` |

当前**不存在重复派发**（生产只走路径 A），但 P13 装配时若调用方面对两个公开入口，选哪条会改变可观测错误语义。这是接入前零消费者的最佳收敛时机。

**建议**：抽 `LoadedPlugin::invoke_with_state(operation, scope)`（约 30 行，内部走 `state_snapshot` + `apply_state_mutations`），让 `on_lifecycle_event` 委托它，并把路径 A 的「未声明事件 no-op」前置返回与「LifecycleHook capability 显式校验」补齐（对已过 validate 的 manifest 行为不变）。两入口各缩到 ~19-20 行，净减约 40-45 行，且语义统一。

### 4.2 ExternalPlugin 不重写调度器门禁（合规，正面案例）

与 P9 的 `McpToolAdapter` 在 adapter 内重写完整门禁管线（[p9-review.md](p9-review.md) §4.1）形成正面对比：

- `ExternalPluginToolAdapter::execute`（registry.rs:192-207）只做 `host_caller.call(...)` 转发 + `PluginError → ToolResult::failure` 错误整形。
- `wasm-plugin-host` / `hook-runtime` 全文 `rg PolicyEngine|ApprovalResolver` 零命中（仅测试一处 `ApprovalMode::NeverAsk`）。
- descriptor 硬编码 `ExternalPlugin` / `read_only=false` / `supports_concurrency=false` / `allowed_in_untrusted_workspace=false`（registry.rs:173-180），由 tool-runtime 调度器统一消费（ExternalPlugin 默认串行、`PolicyEngine::decide` + `ApprovalResolver` 在调度器内统一裁决、未信任工作区 descriptor gate）。

这是 P9 review 要求的 canonical 路径，P10 做对了，应在接入时保持。`manifest.tool_capabilities` 字段（manifest.rs:36）host 从不读取，但这是有意的保守策略（plugins.md「插件自报 read_only 不能降低审批」），非缺陷；P15 canonical tool v2 时可决定去留。

### 4.3 三层锁交互与运行时正确性（复核：正确）

子代理纠正了 Commander 初判的「双重串行化」假设：

- 锁序一致（`load_lock → operation_lock → inner`），`invoke_raw` 只取 inner，无反向序，无死锁。
- `unload` 经 `deactivate` 先 `active.store(false)` 再等 `operation_lock + inner.take()`，确等完整事务；retained handle 被 active gate 双重检查拒绝。测试覆盖 reload 不越过未完成事务。
- wasm 调用只被 inner 串行一次；operation_lock 保护的是 snapshot-revision 原子性，不是叠加等待。
- fuel 每 invoke 重注 + load 期注入防 start function 失控；cancel 先于 call 是 `biased` 设计；Drop abort ticker；`epoch_deadline_async_yield_and_update(1)` 只协作 yield，终止靠 select 丢弃 call future。

三处可优化（均 P2/P3，非缺陷）：

- (a) P2：`HostConfig::validate()`（config.rs:93）未约束 `invoke_timeout >= epoch_tick`；违反时紧循环插件超时最坏晚一个 tick。默认 5s/10ms 无碍。
- (b) P2：`Drop`（host.rs:564-571）中止 ticker 但不排空在途调用；若在 invoke 进行中 drop host（调用方持 `Arc<LoadedPlugin>` 时可能），紧循环 wasm 可能挂起执行器线程。当前零消费者、概率低，接入时若 host 可能被 Arc 共享则需处理。
- (c) P3：epoch deadline 只在 load 设置，invoke 不重置；首次 tick 后每次 invoke 第一个 checkpoint 白 yield 一次。微优化。

### 4.4 文档与实现不一致（P3）

- [workspace-layout.md:26](../architecture/workspace-layout.md) 声称 `agent-engine`「依赖 provider-api / tool-api / plugin-api」，但 `agent-engine/Cargo.toml` 无 `plugin-api`（且全 workspace 零消费者）。文档超前/冲突。若 P13 接线经 tool-runtime ToolRegistry 注入（插件工具走 ExternalPlugin），agent-engine 可能永远不需要 plugin-api，届时删该声明。
- [workspace-layout.md:162](../architecture/workspace-layout.md) 依赖图写 `plugin-host`，实为 `wasm-plugin-host`（§3.9）。
- [plugins.md:20](../features/plugins.md)「插件能力」清单列 11 项，P10 实际只实现 4 项（注册工具/命令/生命周期事件/保存状态）；`ModifyContext`/`CompactionStrategy`/`RegisterProvider`/`UserInteraction` 是预留（= §3.7），「访问受限文件/网络」无 host import，「声明 Monitor」属 P17-2/P16-6。文档靠后文优先级节自我纠正，但 L20 首读即过度承诺。

### 4.5 `PluginStateStore` trait 归属（P2，durable backend 落地时决策）

`PluginStateStore` trait 在 wasm-plugin-host/src/state.rs:46-62，`apply` 签名带 `&HostConfig`（:55-61），而 HostConfig 含 fuel/StoreLimits 等 wasmtime 耦合字段。plugin-api 已是纯协议层并持有全部状态类型（invocation.rs:78-108），Cargo.toml 无 wasmtime。

后果：未来 durable backend 若实现该 trait，必须依赖 wasm-plugin-host → 连带 wasmtime + tool-runtime + hook-runtime 整个子图，与 state.rs:9-12「durable backend 由组合层注入」的注释意图相悖。

判定：上移到 plugin-api 方向合理（符合「plugin-api 纯协议层」），但需先把配额从 HostConfig 解耦（state 配额只 3 个字段，可抽独立 `StateQuota` 入 plugin-api 或改参数传递），同时迁走 `PluginStateError`。时机：**P13-1/P13-2 接线、durable backend 实际落地时一并做**；P10 零消费者阶段上移只会为改而改。

---

## 5. 合并 / 拆分 / 删除建议

按优先级与风险给出（**本 Review 不执行任何修改**）：

### 建议删除（零风险，纯减负，约 60 行）

- 删 `invoke_operation` 的 input 预检（host.rs:508-517，10 行）——`invoke_checked` 必然再查一次。
- 删 `on_lifecycle_event` 内联快照闸门（host.rs:224-234，11 行）——改为调用 `state_snapshot`。
- 删 `PluginManifest::qualified_name`（manifest.rs:115，3 行）——与 `external_tool_name` 重复。
- 删 `PluginOperation::plugin_context`（invocation.rs:36-42，7 行）——生产零用。
- 删 `NamespacedToolRegistry::into_tool_registry`（registry.rs:116）——生产用 `to_tool_registry`。
- 删或降 `pub(crate)`：`NamespacedToolRegistry::len/is_empty`、`PluginCommandRegistry::contains/is_empty`、`TrustStore::install/contains/get/len/is_empty/remove`（约 41 行）。
- 修正 workspace-layout.md:162 `plugin-host → wasm-plugin-host`（1 行）。

### 建议简化（低风险，接入前做）

- 合并 Lifecycle 双路径为单一 `invoke_with_state(operation, scope)`（§4.1），净减约 40-45 行，统一语义。**这是接入前唯一必须做的架构决策。**
- `HostConfig::validate()` 增加 `invoke_timeout >= epoch_tick` 约束（§4.3a）。
- `plugin_subscribes_to` / `HookRuntime::registered` 等「仅测试消费的 pub」降 `pub(crate)` 或标注 test-only。

### 建议在接入规划明确后决策

- 4 个无人消费的 `PluginCapability` 变体（§3.7）：P13 规划明确后，不用则删。
- 3 个无人构造的 lifecycle 事件 `Load`/`Register`/`Unload`（§3.8）：P10 收尾可删，或 P17-2 package 生命周期落地时由 host 显式补发。
- `PluginStateStore` trait 上移 plugin-api（§4.5）：durable backend 落地时做，需先解耦 HostConfig 配额。
- workspace-layout.md:26 `agent-engine` 依赖声明（§4.4）：随 P13-1 接线决策修正。
- plugins.md:20 能力清单逐项标注「P0 已实现 / P2 预留 / P17」（§4.4）。

### 不建议改动（做对的部分）

- **adapter 不重写门禁**（§4.2）：ExternalPlugin 走 canonical 调度器，是 P9 review 要求的正确路径，接入时保持。
- **签名绑定模型**（manifest.rs canonical payload + trust.rs Ed25519）：域分离 + 稳定 JSON + component BLAKE3，篡改任一侧均失败，测试覆盖完整。
- **空 Linker + 资源限额**（host.rs Linker::new 无注入 + StoreLimits + fuel + epoch）：ADR-012 安全不变量到位。
- **状态隔离**（state.rs 乐观 revision + 配额 + PersistentState 闸门）：snapshot→invoke→apply 串行事务设计正确。
- **hook-runtime 故障隔离**：确定性排序 + 独立 task + panic/error/cancel 隔离为可序列化 outcome，不过度 mock。
- **trust.rs / config.rs 独立文件**：133/134 行的单一职责边界，并入 host.rs 只会让 777 行的 host.rs 更臃肿，不合并。
- **WIT + JSON golden + guest binding 三重兼容门禁**（P10-6）：契约稳固。
- **manifest 重复校验保留**：host.load 路径 validate ×2（host.rs:371 + canonical_signing_payload 隐式）+ hook-runtime validate，校验廉价，删 host.rs:371 会改变错误优先级（无效 manifest + 未知 key_id 时由 InvalidManifest 变 SignatureRejected），建议保留现状。

---

## 6. 改进优先级矩阵

| 优先级 | 项 | 收益 | 风险 | 时机 |
|---|---|---|---|---|
| P1 | 合并 Lifecycle 双路径为单一 `invoke_with_state`（§4.1） | 消除语义分叉，净减 ~40 行 | 低（保留 no-op 前置 + 补 capability 闸门） | 接入前（P13） |
| P1 | 删 §5「建议删除」全组（死 pub API + 重复预检 + qualified_name 等，~60 行） | 纯减负 | 零 | 现在可做 |
| P2 | `PluginStateStore` trait 上移 plugin-api + 解耦 HostConfig 配额（§4.5） | durable backend 不必依赖 wasmtime 子图 | 中（需迁类型） | durable backend 落地时 |
| P2 | `HostConfig::validate()` 约束 `invoke_timeout >= epoch_tick`（§4.3a） | 超时观测确定性 | 零 | 现在可做 |
| P2 | 4 个预留 `PluginCapability` 变体去留（§3.7） | 去「声明却无效果」误导 | 低（ADR 级契约） | P13 规划明确后 |
| P2 | 3 个死 lifecycle 事件 `Load/Register/Unload` 去留（§3.8） | 去死概念 | 零 | P10 收尾或 P17-2 |
| P3 | Drop 不排空在途调用的潜在挂起（§4.3b） | host 被 Arc 共享时的健壮性 | 中 | 接入时（若 host 被 Arc 共享） |
| P3 | workspace-layout.md 两处文档不一致 + plugins.md 能力清单标注（§4.4） | 文档与实现一致 | 零 | 下次触碰该文档 |
| P3 | epoch deadline invoke 不重置的微优化（§4.3c） | 去每次 invoke 首个白 yield | 零 | 现在可做 |
| 跟踪 | 端到端接线（§2） | 首次被真实路径验证所有插件能力 | 无 | P13-1/P13-2 |

---

## 7. 整体评价

Phase 10 的**架构方向正确，实现质量是 P8–P9 同期家族里最好的一档**。plugin-api 是纯协议层（无 wasmtime），wasm-plugin-host 是叶子 crate，hook-runtime 单职责确定性派发；依赖方向干净，符合 ADR-012 / ADR-013 与 AGENTS.md §2「Agent Engine 不感知 Provider/Plugin 名称」的红线。最值得肯定的是 §4.2——`ExternalPluginToolAdapter` 把门禁完全委托给 canonical 调度器，没有在 adapter 内重写 `PolicyEngine`/`ApprovalResolver`，正面避免了 P9 的双重裁决问题。签名绑定、空 Linker、资源限额、状态隔离、panic 隔离的安全不变量全部落地且有定向测试。

按本次 Review 的导向（「优先寻找可以减少代码、模块、接口和概念数量的方案」），最值得做的是 §5 的「删除」组与 §4.1 的「双路径合并」——它们能在不损失任何当前可观测语义的前提下净减约 100 行机器代码（约占 P10 四 crate 源码 4%），让 P13 接线时面对的是一个更小、门禁单一、没有死概念与语义分叉的插件子系统。4 个预留 `PluginCapability` 与 3 个死 lifecycle 事件属过早预留，建议随 P13 接线规划明确后退役，避免「声明却无效果」误导未来的插件作者。

---

## 附：调查覆盖与证据

本次 Review 由 3 个 `deepseek_explorer` 并行调查以下不重叠切片，证据均为 `file:line`，关键事实由 Commander 独立复核：

- **内部结构与冗余**（Meitner）：plugin-api/src 全 3 文件 + wasm-plugin-host/src 全 7 文件逐 pub API 盘点、调用点计数（rg 全 workspace）、YAGNI 候选、invoke 双路径重复量化、文件碎片化评估。
- **运行时质量与接线**（Hilbert）：三层锁交互、fuel/epoch/timeout/cancel 正确性、unload 事务与 retained handle、双派发路径架构判定、零消费者复核、Linker 空注入复核、sandbox 关系。
- **canonical 一致性与边界**（Singer）：ExternalPlugin vs policy-engine 集成、hook-runtime vs P17-1 user-hooks 边界、PluginStateStore trait 归属、命名/登记一致性、文档 vs 实现承诺。

Commander 独立复核的关键事实：`PluginCapability` 实为 8 变体（用 4 个）；`PluginLifecycleEvent::{Load,Register,Unload}` 全 workspace 零构造点；`wasm-plugin-host`/`hook-runtime` 无 `PolicyEngine`/`ApprovalResolver` 引用（仅测试一处 `ApprovalMode`）；`into_tool_registry` 仅 2 处测试调用、生产用 `to_tool_registry`；`qualified_name` 仅 1 测试调用；`agent-engine/Cargo.toml` 无 `plugin-api`（与 workspace-layout.md:26 冲突）；`invoke_operation` 的 Lifecycle 分支仅测试调用、生产 lifecycle 只经 `on_lifecycle_event`。

---

## 修复记录（review-remediation）

> Phase 10 · WASM Plugin（plugin-api / wasm-plugin-host / hook-runtime）· 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P10-1 ~ P10-6

**最终目的**：执行本评审 §5/§6 的「减少」导向——删死 pub API、合并重复校验、收敛 Lifecycle 双路径，让 P13 接线时面对一个更小、门禁单一、没有死概念与语义分叉的插件子系统。零端到端消费者（§2）、4 个预留 capability（§3.7）、3 个死 lifecycle 事件（§3.8）、Drop/epoch 微优化（§4.3b,c）、PluginStateStore trait 上移（§4.5）按评审结论显式延后。详见 [P10-7 修复任务](../../plan/P10-7-review-remediation.md)。

### 现在修复（落地）

| 评审项 | 处置 | 证据 |
| --- | --- | --- |
| §3.1 死 pub API | 删 `NamespacedToolRegistry::{into_tool_registry,len,is_empty}`、`PluginCommandRegistry::{contains,len,is_empty}`、`TrustStore::{install,contains,get,len,is_empty,remove}` | registry.rs / trust.rs；host_wat.rs `into→to`、`len→names().len`；`rg` 全 workspace 零残留 |
| §3.4 `qualified_name` | 删方法 + 唯一测试 | manifest.rs；生产用 `registry::external_tool_name` |
| §3.5 `plugin_context` | 删方法 + 唯一测试 + 悬空 import | invocation.rs；生产只走 `state_scope()` |
| §4.1 Lifecycle 双路径 | 新增 `LoadedPlugin::invoke_with_state`，`on_lifecycle_event` 与 `invoke_operation` 均委托它 | host.rs；净 −13 行；`operation_lock` 单次获取 |
| §3.2 重复 input 预检 | 由 `invoke_checked` 统一承担（合并进 §4.1） | host.rs invoke_with_state |
| §3.3 内联快照闸门 | 改调 `state_snapshot` 函数（合并进 §4.1） | host.rs invoke_with_state |
| §4.3a 超时确定性 | `HostConfig::validate` 新增 `invoke_timeout>=epoch_tick` + `TimeoutSmallerThanTick` + 测试 | config.rs |
| §3.9 文档 typo | 依赖图 `plugin-host→wasm-plugin-host` | workspace-layout.md §6 |
| §4.4 文档超前 | agent-engine 依赖行去掉 `plugin-api`；plugins.md 能力清单标注实现状态 | workspace-layout.md §2；plugins.md |

**语义保持**：路径 A（on_lifecycle_event）未声明事件仍 `Ok(())` no-op；路径 B（invoke_operation）未声明仍 `PermissionDenied`；`enforce_operation_capability/registration` 读 `Arc<PluginManifest>`（共享不可变）置于锁外，`operation_lock` 只在 `invoke_with_state` 取一次（tokio Mutex 不可重入）。

### 显式延后（不在本任务范围）

| 评审项 | 延后到 | 原因 |
| --- | --- | --- |
| §2 零端到端消费者 | P13-1/P13-2 | 首次通电才能观测 |
| §3.7 四个预留 capability | P13 规划明确后 | ADR 级契约，接线决策前删/留均可能误导 |
| §3.8 Load/Register/Unload 死事件 | P17-2 package 生命周期 | 届时由 host 显式补发或删除 |
| §4.3b Drop 不排空在途调用 | 接入时 | 仅当 host 被 Arc 共享才有风险 |
| §4.3c epoch deadline invoke 不重置 | 接入时 | 微优化 |
| §4.5 PluginStateStore trait 上移 plugin-api | durable backend 落地时 | 需先解耦 HostConfig 配额 |

### 验证记录（2026-08-10）

- `cargo test -p wasm-plugin-host --lib`：8 passed / 0 failed（含新增 `timeout_smaller_than_tick_is_rejected`）。
- `cargo test -p wasm-plugin-host --test host_wat`：29 passed / 4 filtered。4 个 trap 测试（`fuel_exhaustion_is_reported`/`memory_growth_is_rejected`/`invoke_timeout_aborts_loop`/`cancellation_aborts_loop`）是 **Windows debug 下 wasmtime 27「panic in a function that cannot unwind」直接 abort 测试进程的既有问题**——Commander 用 `git stash` 在未含本次改动的基线上逐个复现（5/5 确定性，`STATUS_STACK_BUFFER_OVERRUN`），与本次改动无关。整体 run 在首个 trap 测试处终止，故用 `--skip` 跑完其余 29 项。该既有问题建议接入 P13 前由独立任务处理。
- `cargo test -p plugin-api`：15 passed（13 单元 + 2 v1_contract）/ 0 failed。
- `cargo clippy -p wasm-plugin-host -p plugin-api -p hook-runtime --all-targets -- -D warnings`：通过，0 警告。
- `cargo fmt -p wasm-plugin-host -p plugin-api -p hook-runtime -- --check`：通过。
- **独立 reviewer 复核**（deepseek_reviewer）：删除项零残留、双路径合并语义保持、§4.3a 自洽、文档与源码事实一致——无阻塞项（详见 P10-7 验证记录）。

**整体 diff**：9 文件、+87/−188，净 −101 行（占 P10 四 crate 源码约 4%，与评审预估一致）。

**相关文档**：[REVIEW.md](../../REVIEW.md) §Phase 10 · [P10-7 修复任务](../../plan/P10-7-review-remediation.md) · [docs/features/plugins.md](../features/plugins.md) · [ROADMAP Phase 10](../../ROADMAP.md)
