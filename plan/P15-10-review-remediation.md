# P15-10：Phase 15 评审修复（REVIEW remediation）

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：domain + adapter verified，host composition deferred）· 依赖：P15-1 ~ P15-9

**最终目的**：按 [docs/review/p15-review.md](../docs/review/p15-review.md) §3/§6 的改进优先级收敛 Phase 15「domain + adapter 已交付、主循环与宿主装配延后」之外的复杂度与重复抽象问题——统一三家 reasoning 保护抽象为一份 `ReasoningProtector` trait（typed `ProtectedBlobRef` 引用）、删除与 `ToolKind` 1:1 冗余的 `ExecutionOwner`（保留有真实消费者的 `ContinuationMode`）、把零消费者 tool_search 收到默认关闭的 `tool-search` feature 门后、为 protected-blob-store 删除公开 `rotate` / `RotateReport`、`integrity_check` / `IntegrityReport`、`disk_usage` 与专属 `all_rows` / 测试（retention、disk_budget 内部约束、crash reconcile、refcount / gc / shutdown 全部保留）、稳定 capability_key 并去掉 `Debug` 字符串反解。同时按源码证据纠正三条评审事实（Anthropic 已共享解析、`ServerToolEvent` 有真实 wire producer、`provider_capability_negotiated` Diagnostic 有通用消费）。`ResponsesStreamAssembler` 判定保留 adapter-local（两家 hosted tool 子集差异真实存在，下沉收益小于差异风险）。最终定向门禁前另修正 OpenAI/xAI 默认 protector 每次 `stream` 重建的问题：默认 `InMemoryReasoningProtector` 提升为 provider 实例级 `Arc`，同一实例可跨 `stream` 回灌 reasoning，并补回归。宿主装配真实 Provider、provider catalog 统一、持久化 protector 生产接线按评审结论显式延后至 Phase 18 配套任务（P18-3 / P18-4 / P18-14）。无新增 crate 与抽象，全部为「删重复 / 收口 / 瘦身 / 事实纠正」。

**涉及范围**：`provider-runtime`（reasoning.rs：统一 trait + `ProtectedBlobStoreProtector`，删 `ReasoningStateBridge` ref-count 生命周期；negotiate.rs：capability_key 稳定化）、`provider-openai` / `provider-xai` / `provider-anthropic`（三家迁移 `with_reasoning_protector(Arc<dyn ReasoningProtector>)`，删本地 ReasoningProtector 拷贝与 `ReasoningContinuationStore`）、`agent-domain`（tool.rs：删 `ExecutionOwner`，保留 `ToolKind` + `ContinuationMode`）、`tool-api`（capability tag 稳定 key）、`tool-runtime`（tool_search 收进 `tool-search` feature，默认关闭）、`protected-blob-store`（lib.rs：删公开 `rotate` / `RotateReport`、`integrity_check` / `IntegrityReport`、`disk_usage` 与专属 `all_rows` / 测试）

## 处置策略（按评审 §6 矩阵）

- **现在修复（落地）**：§3.2 统一 reasoning 保护抽象为单一 `ReasoningProtector` trait + typed ref，三家全部迁移（P1）、§3.1 删 `ExecutionOwner`（P1）、§2.2 tool_search 收口到默认关闭的 `tool-search` feature（P1）、§3.6 protected-blob-store 瘦身——删公开 `rotate` / `RotateReport`、`integrity_check` / `IntegrityReport`、`disk_usage` 与专属 `all_rows` / 测试（P1；retention、disk_budget 内部约束、crash reconcile、refcount / gc / shutdown 保留）、§3.8 稳定 `capability_key` 去掉 `Debug` 格式反解（P2）、§6 P0 状态语义（Phase 15 标注「domain + adapter verified，host composition deferred」，遵循既有 plan-README / testing.md 的 L0/L1 规则）。
- **事实纠正（评审结论修正）**：§3.3「modern.rs 重写一遍」不成立——`modern.rs` 复用 `request.rs` 的 message / tool-choice / thinking-budget / cache-breakpoint helpers，现代与基线路径共同进入 `provider.rs::pump_messages`，共享 auth、`SseParser` 与 `stream.rs::event_to_events`（含 usage 归一）；§2「ServerToolEvent 部分变体仅 fixture 触发」不成立——OpenAI `responses.rs` 对 `CitationAdded` / `SourceAdded` / `ProgramStarted` / `ProgramOutput` / `ComputerActionRequested` / `ComputerScreenshot` / `Started` / `Completed` 有真实 wire producer；§2.3「Diagnostic 只 emit 无消费者」不成立——`provider_capability_negotiated` 落入通用观测通道并有断言消费。
- **保留（评审建议但判定不采纳）**：`ResponsesStreamAssembler` 保留 adapter-local（openai / xai 各自 crate），不强制下沉 `provider-runtime`；`CapabilityNegotiator` 单元结构体保留（降为自由函数收益约 10 行，不值得）；protected-blob-store 保留独立 crate（不并入 artifact-store——AEAD / scope / keyver 隔离符合 ADR-032 职责分离，合并收益小于风险）。
- **显式延后（含后续任务）**：§2.4 宿主装配真实 Provider（`register_provider` / `builtin_models` 接入 model-registry）→ [P18-3 Provider Account](../plan/P18-3-provider-account.md)、[P18-4 Credential Lease](../plan/P18-4-credential-lease.md)（随账号控制面与凭证生命周期一起装配）；§3.7 provider catalog 统一（builtin 能力声明两处分裂）→ [P18-14 Provider Registry / Pool Reconciliation](../plan/P18-14-pool-reconciliation.md)；§2.1 持久化 protector 生产接线（`ProtectedBlobStoreProtector` + 生产 `ProtectedKeyResolver` 注入 host）→ P18-3 / P18-4 / P18-14（凭证边界与 registry 装配成熟后接线）。

## 细分步骤

### A. 统一 reasoning 保护抽象（§3.2，P1，provider-runtime + 三家）

1. `provider-runtime::reasoning` 收敛为单一 `ReasoningProtector` trait（实际 API：`protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError>` + `resolve(&self, blob_ref: &ProtectedBlobRef) -> Result<ProtectedBlob, ReasoningProtectError>`，scope 由 `ProtectedBlobStoreProtector::new(store, scope)` 在构造时捕获，不逐次传参；统一引用类型 `ProtectedBlobRef`、统一错误类型）；`InMemoryReasoningProtector` 默认实现保留，新增 `ProtectedBlobStoreProtector`（基于 `protected-blob-store` 的标准实现，三家持久实现均为它）。
2. 删三家本地拷贝：openai / xai 逐字重复的 `ReasoningProtector` + `InMemoryReasoningProtector`、anthropic `ReasoningContinuationStore`；三家 `with_reasoning_protector(Arc<dyn ReasoningProtector>)` 注入点统一。OpenAI/xAI 的默认 `InMemoryReasoningProtector` 为 provider 实例级 `Arc`，不再每次 `stream` 重建；同一实例跨 `stream` 回灌由回归测试覆盖。
3. 删 `ReasoningStateBridge` ref-count 生命周期 API（`retain` / `release` / `rollback_uncommitted` / `gc` / `shutdown`——无生产调用方，等 compaction / session-delete 真正接入再加）。
4. 残留核验：`rg "ReasoningStateBridge|ReasoningContinuationStore"` 在 crates/ apps/ 零生产命中。

### B. 删 ExecutionOwner，保留真实消费者 ContinuationMode（§3.1，P1，agent-domain）

5. 删 `ExecutionOwner { Core, Provider, Extension }`（与 `ToolKind` 严格 1:1，无独立消费者）与 `ToolKind::execution_owner()`；`ToolKind` 三执行位点不变。
6. 保留 `ContinuationMode { CoreSuppliedResult, ProviderTranscript }`——`CoreSuppliedResult` 是唯一由适配器翻译为 Provider function-result 字段的形态、`ProviderTranscript` 续接 ServerToolEvent / 原生 output item，有独立判别用途（scheduler 路由与适配器翻译均消费）。
7. 残留核验：`rg "ExecutionOwner"` 在 crates/ apps/ 零生产命中。

### C. tool_search 收口（§2.2，P1，tool-runtime）

8. `tool-runtime` 新增 `tool-search` feature（默认关闭）；`tool_search` 模块与 `pub use` 置于 `#[cfg(feature = "tool-search")]` 之后，非默认构建不编译不导出。
9. `LazyToolIndex` / `ToolSearchIndex` / `search_tools` / `activate_tool` 保留为完整实现但仅在显式启用 feature 时可用；接入主循环前不启用（零消费者期间不占默认构建与公共 API 面）。

### D. protected-blob-store 瘦身（§3.6，P1，protected-blob-store）

10. 删公开 `rotate()` / `RotateReport`、`integrity_check()` / `IntegrityReport`（含其 `orphans` 字段）与 `disk_usage()`（含仅供这些 API 使用的 `all_rows()` 内部 helper 与专属测试，净删 250 行）；保留 AEAD seal / open + `BlobScope` + `ProtectedBlobRef` + key version + `ProtectedKeyResolver` + 原子两阶段写入，以及 retention（默认 7 天）、disk_budget 内部约束（put 时计费、reconciliation 前保持计费）、`pending→ready/deleting` crash reconcile（启动恢复 + 并发 open-time 复核）、refcount / `retain` / `release` / `gc()` + `GcReport` / `shutdown()`——这些均属当前真实需求；上层 append 失败时以 `release` 回滚未提交的首引用，store 不另设 `rollback_uncommitted` API。
11. 残留核验：`rg "RotateReport|IntegrityReport|disk_usage|all_rows"` 在 `crates/protected-blob-store` 零命中；`integrity_check` / `disk_usage` 在 `artifact-store`（ADR-004 自有）与 `session-store`（lifecycle 只读检测）为合法保留，不在本任务删除面。

### E. 稳定 capability_key（§3.8，P2，provider-runtime + tool-api）

12. `ToolCapabilityTag::capability_key()` 稳定字符串作为协商键；`negotiate.rs` 与 `AcceptedResponsesTools::from_supported` 改为类型化匹配（`BTreeSet<ToolCapabilityTag>` / 稳定 key），删除 `format!("tool:{tag:?}")` `Debug` 格式反解。
13. 新增断言：`all_tool_tags_negotiate_via_stable_capability_key`（全能力模型 + 全部 tag 请求 → 每个 tag 都以 capability_key 进入协商，不依赖 `Debug` 格式）。

### F. 事实纠正与保留（评审结论修正）

14. 纠正 Anthropic 已共享解析：`modern.rs` 复用 `crate::request` 的 message / tool-choice / thinking-budget / cache-breakpoint helpers；现代与基线路径共同进入 `provider.rs::pump_messages`，共享 auth、`SseParser` 与 `stream.rs::event_to_events`（含 usage 归一），非「重写一遍」；文档同步。
15. 纠正 ServerToolEvent 真实 wire producer（三家均有真实发射点，非仅 fixture）：OpenAI `responses.rs` → `CitationAdded` / `SourceAdded` / `ProgramStarted` / `ProgramOutput` / `ComputerActionRequested` / `ComputerScreenshot` / `Started` / `Completed`；Anthropic `stream.rs` → `CitationAdded` / `SourceAdded` / `Started` / `Completed`（现代 server tool 生命周期）；xAI `responses.rs` → `CitationAdded` / `SourceAdded` / `ProgramStarted` / `ProgramOutput` / `Started` / `Completed` / `Failed`。`ProgramStarted` / `Computer*` 属「真实 wire producer + 部分变体待真实服务端工具消费」。
16. 纠正 Diagnostic 通用消费：`provider_capability_negotiated` 经通用观测通道落入 provider_loop 并有断言消费（`chosen_transport` 可观察）。
17. 保留 `ResponsesStreamAssembler` adapter-local（openai / xai 各自 crate），两家 hosted tool 子集差异真实存在（openai computer/image/local_shell/custom_tool vs xai live_search/collection_search），共享骨架下沉收益小于差异风险；不强制。

### G. §6 P0 状态语义（W，文档）

18. Phase 15 行与 P15-2/3/4/7/8 文档标注「domain + adapter verified，host composition deferred」（有界 TargetVerified），不得声称生产已装配；P15-1/5/9 维持 TargetVerified（领域 + 门禁真实闭环）。
19. P15-1～P15-10 全部标 🟢；计数 Phase 15 10/10，总计 218/165（逐 Phase 行机械求和；P15-10 前旧合计为 217/167、旧 Phase 行实际为 217/164，既有完成数漂移 +3）。

## 主要产出物

- **删除**：`ExecutionOwner` 与 `execution_owner()`；openai / xai `ReasoningProtector` + `InMemoryReasoningProtector` 本地拷贝；anthropic `ReasoningContinuationStore`；`ReasoningStateBridge` ref-count 生命周期 API；protected-blob-store 公开 `rotate` / `RotateReport`、`integrity_check` / `IntegrityReport`（含 `orphans` 字段）、`disk_usage` 与专属 `all_rows` / 测试；`Debug` 格式 tag 反解。
- **新增**：`provider-runtime::reasoning::ReasoningProtector` 单一 trait + `ProtectedBlobStoreProtector` 标准实现；`tool-runtime` `tool-search` feature（默认关闭）；`ToolCapabilityTag::capability_key()` 稳定协商键与类型化匹配。
- **迁移**：三家 provider 统一 `with_reasoning_protector(Arc<dyn ReasoningProtector>)`；OpenAI/xAI 默认 `InMemoryReasoningProtector` 为实例级 `Arc`，同一实例跨 `stream` 可解析上一轮引用（持久化接线延 P18）。
- **保留**：`ToolKind` 三执行位点、`ContinuationMode`（真实消费者）、`ResponsesStreamAssembler`（adapter-local）、`CapabilityNegotiator` 本体、P15-9 门禁脚本、canonical 领域类型。
- **测试**：新增/强化断言（capability_key 稳定协商、统一 trait 三家往返、OpenAI/xAI 默认 protector 同实例跨 `stream` 回灌、tool-search feature 编译门），8 crate 联合 18 targets / 364 passed + tool-runtime feature 1 target / 27 passed。

## 验收标准（保留 REVIEW 追踪章节）

- [x] **§3.2**：reasoning 保护只有 `provider-runtime::reasoning::ReasoningProtector` 一份 trait；三家 provider 均经 `with_reasoning_protector` 注入；OpenAI/xAI 默认 protector 为实例级 `Arc`，同实例跨 `stream` 回灌回归通过；`ReasoningStateBridge` / `ReasoningContinuationStore` / 本地 `ReasoningProtector` 拷贝全删（rg 零生产命中）
- [x] **§3.1**：`ExecutionOwner` 全删（rg 零生产命中）；`ContinuationMode` 保留并确认真实消费者（适配器翻译 function-result / transcript 续接）
- [x] **§2.2**：tool_search 默认不编译不导出（`tool-search` feature 默认关闭）；feature 开启时既有索引/搜索/激活/审批/预算联动测试全过（27 passed）
- [x] **§3.6**：protected-blob-store 删公开 rotate / RotateReport、integrity_check / IntegrityReport、disk_usage 与专属 all_rows / 测试（rg 在 protected-blob-store 零命中）；retention、disk_budget 内部约束、crash reconcile、refcount / gc / shutdown 全部保留
- [x] **§3.8**：协商与 `AcceptedResponsesTools` 均经稳定 `capability_key` 类型化匹配，不依赖 `Debug` 格式（断言覆盖全 tag）
- [x] **§3.3 / §2 / §2.3 事实纠正**：Anthropic 已共享解析、ServerToolEvent 真实 wire producer、Diagnostic 通用消费按源码证据修正并同步文档
- [x] **保留项**：`ResponsesStreamAssembler` adapter-local；`CapabilityNegotiator` 本体、P15-9 门禁、canonical 领域类型不动
- [x] **§6 P0**：Phase 15 状态写有界 TargetVerified（domain + adapter verified，host composition deferred），不声称生产已装配；计数 P15 10/10、总计 218/165
- [x] **定向验证**：8 crate 联合 `cargo test`（18 targets / 364 passed）/ tool-runtime `--features tool-search`（1 target / 27 passed）/ 8 成员及 feature `cargo clippy --all-targets -- -D warnings`（0 warning）/ 21 个本任务 Rust 文件 rustfmt check（PASS，workspace fmt 未跑）/ `git diff --check` 与残留 `rg` 核验（PASS）/ `scripts/p15-gate.sh`（13 targets / 48 passed，contract / golden / fuzz / 兼容性 PASS，`target/gates` 清理）；三条测试命令累计执行 32 target executions / 439 test executions / 0 failed（feature 与 gate 含有意复跑，非去重用例数；见验证记录）

### Deferred items（建议/跟踪，本任务不做）

- **§2.4 宿主装配真实 Provider**：`pawork` / `core-runtime` / `cli-host` 仍不构造真实 Provider，`app-service::register_provider` 唯一调用者是测试 mock。由 [P18-3](../plan/P18-3-provider-account.md)（Provider Account / 多账号）与 [P18-4](../plan/P18-4-credential-lease.md)（CredentialPool / Lease）提供账号与凭证边界后统一装配，不在 Phase 15 内各自补丁。
- **§3.7 provider catalog 统一**：provider `builtin_models()` 的 v2 能力声明与 model-registry `caps()` 两处清单仍分裂。由 [P18-14](../plan/P18-14-pool-reconciliation.md)（Provider Registry / 健康探测 / 热切换）统一目录与协商证据，消除分裂。
- **§2.1 持久化 protector 生产接线**：生产路径默认 `InMemoryReasoningProtector`（进程内可回放、重启即丢）；`ProtectedBlobStoreProtector` 与生产 `ProtectedKeyResolver` 的 host 注入随 P18-3 / P18-4 / P18-14 凭证边界与 registry 装配接线，兑现 ADR-032「加密落盘 / crash 恢复」承诺。因持久实现构造时捕获单一 `BlobScope`，后续 factory / pool 必须按实际 Session/run scope 构造或选择 protector，禁止跨 Session 复用同一 scoped 实例；该约束已登记到 P18-3 / P18-14 验收项。
- **`ResponsesStreamAssembler` 下沉**：维持 adapter-local（见保留项）；若后续出现第三家同构 assembler 或共享骨架收益明确，再评估下沉 provider-runtime（YAGNI 优先）。

### Reviewer 提出但判定为可接受的低优先项（不另立任务）

- **§3.9 `CapabilityNegotiator` 降为自由函数**：收益约 10 行且损失语义分组，保留单元结构体。
- **§2.3 Diagnostic 降级 `tracing::debug!`**：`provider_capability_negotiated` 已确认有通用消费（观测通道 + 断言），保留 Diagnostic 形态。
- **§3.4 assembler 骨架下沉**：保留 adapter-local（理由见细分步骤 17）。

## 验证记录（2026-08-12）

- `cargo test`（8 crate 联合：agent-domain / provider-runtime / provider-openai / provider-anthropic / provider-xai / tool-api / tool-runtime / protected-blob-store）：**18 test targets / 364 passed / 0 failed**（覆盖统一 trait 三家往返、OpenAI/xAI 默认 protector 同实例跨 `stream` 回灌、ExecutionOwner 删除后路由回归、capability_key 稳定协商、blob 瘦身后 put/get/resolve、三家现代路径 wiremock smoke等断言）。
- `cargo test -p tool-runtime --features tool-search --all-targets`：**1 test target / 27 passed / 0 failed**（feature 开启时索引 / 搜索 / 激活 / 审批 / 预算联动完整保留）。
- `scripts/p15-gate.sh`：**13 test targets / 48 passed / 0 failed**，contract / golden / fuzz / 兼容性四类全部 PASS，`trap` 清理生效——`target/gates` 在门禁结束后不残留（`P15_GATE_KEEP_TARGET` 未设置）。三条测试命令累计执行 **32 target executions / 439 test executions / 0 failed**；feature 命令会复跑默认 tool-runtime tests，专用 gate 也会在隔离 target 中复跑 provider gate，因此这里只报告真实累计执行次数，不声称去重用例数。
- `cargo clippy`（8 成员 + `tool-runtime/tool-search` 与 `agent-domain/typegen` feature，`--all-targets -- -D warnings`）：**0 warning**。
- 21 个本任务 Rust 文件 rustfmt check：PASS（`rustfmt --check` 逐文件，覆盖 8 crate 的全部本任务改动文件；workspace `cargo fmt --all` 未跑、不作为门禁）；`git diff --check`：PASS；残留核验 `rg`：已删符号 `ExecutionOwner` / `ReasoningStateBridge` / `ReasoningContinuationStore` 在 crates/ apps/ 零生产命中；`RotateReport` / `IntegrityReport` / `disk_usage` / `all_rows` 在 `protected-blob-store` 零命中（`integrity_check` / `disk_usage` 在 artifact-store / session-store 属各自合法 API，非本任务删除面；`rotate` 在 auth-service / mcp-client 指 OAuth token 轮换，语义不同不误报）。
- 门禁节奏：P15 定向功能簇按实际 diff 使用 8 个 `-p`、两项 feature 与专用 gate 组合验证，覆盖已充分；Full workspace gate NOT RUN（8 个 `-p` + 专用 gate 已充分覆盖）。三平台与发布门禁留待 Core 主干 L3。
- 独立 `deepseek_reviewer` 只读复核：**VERDICT: PASS**。其指出的两项低影响证据口径（439 为含有意复跑的累计执行次数；旧表写作 217/167、而旧 Phase 行机械和为 217/164）已在本记录、REVIEW 与 ROADMAP 中校正；未发现代码缺陷、越界写入或未登记的 Review 遗漏。

```text
Validation Level: L2（P15 定向功能簇门禁）
Affected crates: agent-domain、provider-runtime、provider-openai、provider-anthropic、provider-xai、tool-api、tool-runtime、protected-blob-store（changed + 关键直接消费者）
Validated: cargo test（8 crate：18 targets / 364 passed）/ tool-runtime --features tool-search（1 target / 27 passed）/ scripts/p15-gate.sh（13 targets / 48 passed；四类 PASS 且 target/gates 清理）/ cargo clippy（8 成员 + 两项 feature，0 warning）/ 21 个本任务 Rust 文件 rustfmt check（workspace fmt 未跑）/ git diff --check 与 rg 残留核验；三条测试命令累计 32 target executions / 439 test executions / 0 failed（feature 与 gate 含有意复跑）
Targeted regressions: 统一 ReasoningProtector 三家往返、OpenAI/xAI 默认 protector 实例级 Arc 与同实例跨 stream 回灌、ExecutionOwner 删除后位点路由、ContinuationMode 消费者回归、capability_key 稳定协商全 tag、tool-search feature 编译门与功能保留、blob 瘦身后 put/get/resolve、三家现代路径 wiremock smoke、p15-gate contract/golden/fuzz/兼容四类
Full workspace gate: NOT RUN（8 个 -p + 专用 gate 已充分覆盖）
```

**相关文档**：[REVIEW.md](../REVIEW.md) §P15 · [docs/review/p15-review.md](../docs/review/p15-review.md) · [ADR-032](../docs/adr/ADR-032-protected-blob-store.md) · [ROADMAP Phase 15](../ROADMAP.md) · [plan/README](../plan/README.md) · [测试体系](../docs/quality/testing.md)
