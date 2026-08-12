# P16-7：Long-term Memory（跨会话长期记忆）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：library/core verified，host composition deferred） · 依赖：P1-4、P3-2、P5-5、P0-4；建议 P5-8

**最终目的**：让 Agent 拥有跨 Session 的长期记忆——从历史 canonical event 中提炼可复用的事实/偏好/决策，按需检索注入当前上下文，使 Agent「记得」项目约定与过往结论，而非每次从零开始；记忆对 canonical event 只读、永不改写历史，且受隐私边界约束。

**涉及范围**：新增 `memory-service`；复用 `agent-events`（只读源）、`context-engine`（注入与预算，P3-2）、`session-store`、`compaction-engine`（P5-5，作为提炼输入之一）、`artifact-store`（记忆载体）、`provider-api`（**canonical EmbeddingProvider**，见步骤 3a）。`memory-service` 只依赖 `provider-api`（trait + canonical 类型），不依赖任何具体 Provider crate，保持 Provider 无关。

## 细分步骤

1. **记忆模型** —— 目的：定义 `Memory`（事实/偏好/决策条目，带来源 event 引用、置信度、过期/失效条件、隐私标签），纯领域类型；记忆本身也是 canonical event（`MemoryEvent::Recorded` / `MemoryEvent::Invalidated`）。
2. **提炼（只读历史）** —— 目的：从 `agent-events` 与 compaction 摘要中提炼候选记忆，对历史 event 只读不改；提炼可由 Agent 触发或自动化（P16-5）定期跑，结果需经 success criteria 或人审确认（避免幻觉写入）。
3a. **Canonical Embedding 契约（共享前置）** —— 目的：在 `provider-api` 新增 canonical embedding 抽象——`EmbeddingProvider` trait、`EmbeddingRequest`（含 batch / model / 维度选项）、`EmbeddingResponse`、`EmbeddingModelDefinition`、`EmbeddingCapabilities`（`dimensions` / `max_input_tokens` / `batch_size`）与 `usage`，凭证复用既有 `ResolvedCredential`。该契约是跨 Provider 的统一面，`memory-service` 只消费它，**禁止按 Provider 名称调用不同 API、禁止用 `provider_options` 绕过 canonical、禁止私自实现 Provider-specific embedding 请求**。Provider 侧落地由各 `provider-*`（OpenAI-compatible / GLM / Qwen / Moonshot 等）实现 trait，Core 不感知 Provider 名。
3b. **嵌入与检索** —— 目的：记忆文本经 canonical `EmbeddingProvider` 取向量，向量与余弦相似自实现最小子集，持久化于 SQLite blob 列（不引入向量数据库）；检索 Top-K 注入上下文，受 token 预算（P3-2）约束。`memory-service` 经依赖注入接收一个 `EmbeddingProvider` 实现，自身不含 Provider 名称分支。
4. **隐私边界** —— 目的：记忆带隐私标签与 workspace 归属，跨 workspace 默认不共享；含 Secret/敏感内容的 event 不进入记忆；记忆查询入审计并脱敏（复用 P1-9 redaction）。
5. **失效与版本** —— 目的：过时记忆可被标记失效（`invalidated`）而非删除，保留可追溯；与 checkpoint/compaction 协同避免「记住了已被回滚的结论」。
6. **查询面** —— 目的：`core-api` 暴露记忆查询/失效/导出，GUI/CLI 可审查与纠偏记忆。
7. **定向 / Mock 测试** —— 目的：提炼只读不改写、检索 Top-K 与预算裁剪、隐私标签隔离、失效可追溯、`memory-service` 经 mock `EmbeddingProvider` 取向量且不含 Provider 名分支。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `provider-api`：`EmbeddingProvider` + `EmbeddingRequest` / `EmbeddingResponse` / `EmbeddingModelDefinition` / `EmbeddingCapabilities`（canonical embedding 契约）
- `memory-service`：记忆模型 + 提炼 + 嵌入检索 + 失效（消费 canonical `EmbeddingProvider`）
- 记忆相关 canonical event 与审计
- 查询面与定向测试

## 验收标准

- [x] 记忆从历史 event 只读提炼，不修改/不删除任何 canonical event（测试支撑：`extract_is_readonly_and_groups` / `extract_from_events_has_no_source`）
- [ ] 部分达成：自实现余弦相似 Top-K 检索与预算裁剪已验（`cosine_basic` / `budget_caps_results` / `token_estimate`，无向量数据库）；context-engine 注入与 token 预算接线未接（无 consumer）→ 未达成，登记 **P16-10 #T6**
- [ ] 部分达成：`provider-api` canonical `EmbeddingProvider` 契约已落地，memory-service 只依赖 provider-api、无 Provider 名分支（`no_provider_branch` 断言通过）；全仓无生产 EmbeddingProvider 实现（唯一实现为测试内 `FixedEmbedder`）→ 未达成，登记 **P16-10 #T6**
- [x] 记忆按 workspace 归属与隐私标签隔离，含 Secret 内容不进入记忆（测试支撑：`privacy_filters_workspace_local` / `shareable_crosses_workspaces` / `secret_summary_rejected`）
- [x] 记忆记录/失效为 canonical event，embedding/confidence 随事件重放完整恢复（测试支撑：`replay_via_apply_matches_live_path_completely` / `apply_replay_matches_direct_validity`；旧流缺字段 serde 默认空向量并被检索过滤，需重新嵌入；SQLite 持久化未接，现为内存 `BTreeMap` → 归 P16-10 #T6）

**相关文档**：[providers](../docs/features/providers.md) · [models](../docs/features/models.md) · [context（预算）](../docs/features/context.md) · [sessions](../docs/features/sessions.md) · [observability（脱敏）](../docs/features/observability.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖、不新增独立 embedding crate。**embedding 采用「扩展 `provider-api`」方案**：`provider-api` 已是 canonical provider 契约面（`ModelProvider` / `ProviderError` / `ResolvedCredential` / model-registry 能力），embedding 是 Provider 的另一项 canonical 能力，与其平行放在同一层最契合现有依赖方向——避免平行 crate 层级、复用同一套凭证与 model-registry，并使 `memory-service` 只依赖 `provider-api`（Provider 无关）而非 `provider-runtime`。向量与余弦相似自实现最小子集，存储用基线 `rusqlite` blob 列。新 crate `memory-service` 依赖方向：`agent-domain → memory-service → app-service`；`provider-api` 由各 `provider-*` 实现 `EmbeddingProvider`。

## 校准记录（2026-08-12）

依据 [p16-review](../docs/review/p16-review.md) 评审结论与当前工作区 remediation 状态校准：只读提炼、隐私/workspace 隔离、Secret 拒绝、canonical 事件化与 embedding/confidence 完整重放（含旧流 serde 兼容）属库级实现且有测试支撑，保留 **TargetVerified（library/core）**；「SQLite blob 持久化、生产 EmbeddingProvider 实现、context-engine 检索注入与预算、compaction/checkpoint 协同、审计查询面」未达成，登记 **P16-10** 并映射后续任务 #T6。

验证记录：`scripts/p16-gate.sh` 四类全 PASS（独立 `target/gates` 跑完已清理）；memory-service 定向测试 17 passed。

```text
Validation Level: L2（P16 簇门禁脚本，文档校准任务附带复跑）
Affected crates: none（本任务仅改 plan/P16-7 文档）
Validated: scripts/p16-gate.sh；memory-service 定向测试
Targeted regressions: embedding/confidence 重放完整性 + 旧流默认值兼容
Full workspace gate: NOT RUN（未命中升级条件）
```
