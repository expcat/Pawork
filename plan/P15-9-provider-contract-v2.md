# P15-9：Phase 15 功能簇门禁（Contract / Golden / Fuzz / 兼容性）

> Phase 15 · Provider Native Capabilities · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P15-1 ~ P15-8、P2-11、P2-12

**最终目的**：为 Phase 15 整簇（Canonical Tool v2、三家现代 Provider API、server tool 事件、tool search、reasoning state、capability discovery）建立集中门禁——扩展统一 Contract Tests、引入 golden 三家字段映射快照、对 citation/reasoning 回灌做 fuzz、验证旧 Chat Completions / P6 基线的兼容性。与 P15-1~8 不同，本任务要求**集中功能簇门禁**并使用**独立 `CARGO_TARGET_DIR` 隔离构建产物，门禁后清理**，不污染日常 target 目录。

**涉及范围**：`provider-openai` / `provider-anthropic` / `provider-xai`（contract 套件扩展）、`provider-runtime`（共享夹具）、`agent-domain`（golden 快照）；新增测试 crate 或 `tests/` 目录按 P2-11 既有约定；不改功能代码，仅补门禁。

## 细分步骤

1. **独立 target 目录门禁脚本** —— 目的：提供门禁脚本，设置 `CARGO_TARGET_DIR=target/gates` 隔离构建产物，并在 `finally` 执行 `cargo clean --target-dir target/gates`，确保门禁通过或失败都不污染日常 target 缓存。
2. **Contract Tests 扩展** —— 目的：在 P2-11 统一 Provider Contract 套件上扩展 Phase 15 场景——Responses / 现代 Messages 传输、server tools 往返、reasoning 加密凭证往返、capability 协商降级；三家共用同一套 canonical 断言。
3. **Golden 三家字段映射** —— 目的：为 P15-5 citation/source 与 P15-7 reasoning 凭证的三家字段映射建立 golden 快照（insta），锁定「OpenAI/Anthropic/xAI 原始字段 → canonical」的归一行为；Provider 改字段时快照差异显式可审。
4. **Citation / reasoning 回灌 fuzz 与受保护 blob 守护** —— 目的：用 proptest/arbitrary 对畸形或边界的 citation 列表、reasoning 凭证 blob、interleaved 顺序做 fuzz，断言归一不 panic、不丢字段、凭证原文不被解码/落日志/落普通 Event payload/落 OS Keychain（ADR-032，与 P15-7 §7 红线一致）。
5. **兼容性门禁** —— 目的：验证旧路径（P6-1 Chat Completions、P6-2 Messages、P6-10 xAI Chat）在引入 v2 后行为不变——同一 canonical 输入经旧路径与新路径（协商降级时）产出等价事件流（差分断言）。
6. **`no_provider_branch` 守护** —— 目的：扩展 `agent-engine/tests/no_provider_branch.rs`（P6 既有），覆盖 Phase 15 新增的 hosted_tools / ReasoningItem / `ReasoningEffort` / `EmbeddingProvider`（P16-7 canonical embedding）/ 协商记录不含 Provider 名称分支；并守护 `user-hooks` 的 PromptEval/AgentEval handler 不含 Provider 名分支（P17-1）。
7. **门禁执行与清理** —— 目的：在独立 `CARGO_TARGET_DIR` 下依次跑 contract / golden / fuzz / 兼容性四类门禁，汇总结果；无论成败用 Cargo 清理 `target/gates`，产出可复核的结论（非完整日志）。

## 主要产出物

- Phase 15 contract 套件扩展（三家现代 API 场景）
- citation / reasoning 三家字段映射 golden 快照
- citation / reasoning 回灌 fuzz 用例
- 独立 `CARGO_TARGET_DIR` 门禁脚本（含清理）
- 兼容性差分断言（旧 Chat / P6 基线 vs v2 降级路径）

## 验收标准

- [ ] contract / golden / fuzz / 兼容性 四类门禁在独立 `CARGO_TARGET_DIR` 下通过
- [ ] 门禁脚本在 `finally` 执行 `cargo clean --target-dir target/gates`，失败路径也不残留隔离构建缓存
- [ ] golden 快照锁定三家 citation/source 与 reasoning 凭证字段映射，差异可审
- [ ] fuzz 覆盖畸形 citation / reasoning 凭证 / interleaved 顺序，归一不 panic；reasoning 凭证原文不落日志/普通 Event payload/OS Keychain（ADR-032）
- [ ] `no_provider_branch` 守护覆盖 hosted_tools / ReasoningItem / ReasoningEffort / EmbeddingProvider / 协商记录 / hooks handler
- [ ] 旧路径（Chat Completions / P6 Messages / xAI Chat）在 v2 后行为不变（兼容性差分断言）
- [ ] 失败时给出可读差异与定位，不粘贴完整日志

**相关文档**：[providers](../docs/features/providers.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [ADR-016 事件持久化重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [测试体系](../docs/quality/testing.md) · [P2-11 Contract Tests](P2-11-contract-tests.md) · [P2-12 Phase 2 评审修复](P2-12-review-remediation.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增依赖，复用基线已列测试栈（insta / proptest / arbitrary / wiremock）；fuzz 若需 `cargo-fuzz` 按基线既有流程，不在此新增。独立 target 目录仅作隔离，门禁脚本负责清理。
