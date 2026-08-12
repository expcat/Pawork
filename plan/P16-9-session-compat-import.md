# P16-9：Session 兼容导入（Claude / Codex / Grok / Cursor）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：library/core verified，host composition deferred） · 依赖：P16-1～P16-8、P5-8、P5-9、P0-3、P7-3

**最终目的**：把来自其他智能体工具（Claude / Codex / Grok / Cursor）的外部会话导入为 Pawork 的 canonical event，使既有对话与产物可被重放、检索与续接，而**绝不破坏 canonical event 模型**——外部格式始终是输入侧的适配/投影，导入产物是规范事件，不污染、不覆写既有事件。本任务同时承担 P16 功能簇的兼容/重放收尾门禁。

**涉及范围**：扩展 `session-store` 导入路径（P5-8 export-import / P5-9 pi-jsonl-import 的同类扩展）；复用 `agent-events`（canonical 目标）与 `agent-domain` 的 `ReviewAnchor`/`ReviewEvent`（评审项锚点）。因分层约束不反向依赖 `diff-service`/`artifact-store`/`review-engine`（见步骤 4 偏离说明）

## 细分步骤

1. **格式探测与 schema** —— 目的：识别 Claude（导出 JSON/对话）、Codex（rollout/JSONL）、Grok、Cursor 四种来源，各自定义只读解析 schema；无法识别或字段缺失时失败可读，不猜测填充。
2. **字段映射到 canonical event** —— 目的：把外部消息/工具调用/产物映射到 `agent-events` 的 canonical 类型（user/assistant/tool/tool_result/usage 等）。unknown 口径限定：逐条不可映射的记录进 `AgentEvent::Diagnostic`（raw metadata，遵循 ADR-002 Provider 解耦的 raw 保留原则）；顶层 `unknown_fields` 仅进 `CompatImportReport`，未持久化进事件。**不新增非规范事件类型**。
3. **不可破坏 canonical event** —— 目的：导入只生成新的 canonical event（新 Session / 新 event id），绝不修改、覆盖或删除任何既有 event；导入产物可被 compaction/checkpoint/replay 等既有机制正常消费，导入是「事件生产者」而非「事件改写者」。
4. **patch / 产物 raw 保留（与 plan 的偏离）** —— 目的：外部文件改动（unified diff）原样保留在 tool result content；仅显式携带 file/line 的评审意见映射为 canonical `Review(ReviewEvent::FindingOpened)`。因 `session-store` 是底层存储 crate，反向依赖 `diff-service`/`review-engine` 会破坏分层，导入期不生成无消费者的 pseudo anchor；真正的 Review consumer 出现后由上层复用 Review core 锚定（P16-10 #T8 复核）。
5. **结构与 Secret 校验 + 整批回滚** —— 目的：持久化前做结构校验（sequence 连续、parent 无悬空、tool result 有前置 tool call）并扫描 Secret（拒绝策略）；校验或持久化任一失败由单一 SQLite transaction 整批回滚、零残留。注意：当前 `validate_structure` 是结构校验，**不是**状态机 replay；「真实 reducer 重建完整可运行 snapshot」的 replay 校验登记 **P16-10 #T8**。
6. **查询面与去重** —— 目的：导入记录来源标识与原始 id，支持按来源去重（同一外部会话重复导入不产生重复 event）；`core-api` 暴露导入入口与导入历史。
7. **簇兼容/重放收尾门禁（独立构建目录）** —— 目的：作为 P16 功能簇的收尾门禁，定向验证「Plan / Goal / Background / Automation / Monitor / Memory / Review / 兼容导入」的 canonical event 可被一致重放；该门禁使用**独立的 `CARGO_TARGET_DIR`**（避免污染主 `target/`），跑完后清理该目录。

## 主要产出物

- 四来源（Claude / Codex / Grok / Cursor）的只读解析器 + 字段映射
- canonical event 导入产物 + 结构/Secret 校验（单事务整批回滚）+ 幂等去重
- P16 簇兼容/重放收尾门禁脚本（独立 `CARGO_TARGET_DIR` + 清理）
- 定向 / Mock smoke 测试

## 验收标准

- [x] 四来源（Claude / Codex / Grok / Cursor）只读解析并映射为 canonical event，逐条不可映射记录进 `Diagnostic`（raw metadata）、顶层 `unknown_fields` 仅 report 未持久化，不新增非规范事件类型（测试支撑：`parse_claude_maps_messages_and_preserves_unknown` / `parse_codex_maps_typed_entries_and_keeps_unknown_as_raw` / `parse_grok_and_cursor_handle_messages_array`；source 由调用方显式传入，无自动格式探测 → 归 P16-10 #T8）
- [x] 导入只新增事件，绝不修改/覆盖/删除既有 canonical event（测试支撑：`import_creates_new_session_and_is_append_only` / `import_does_not_modify_original_file`；`session_events` append-only 触发器为底层硬保证）
- [ ] 部分达成：结构校验（`validate_structure`：sequence 连续/parent 无悬空/tool 引用配对）+ Secret 拒绝 + 单事务整批回滚零残留已修（测试支撑：`validate_structure_catches_bad_envelope_and_accepts_good` / `import_failure_leaves_no_residue_and_is_retryable` / `import_rejects_secret_and_persists_nothing`）；「状态机可推进」的真实 reducer replay 校验未实现 → 未达成，登记 **P16-10 #T8**
- [x] 同一外部会话重复导入不产生重复 event（`(source, original_id)` + content fingerprint 幂等，测试支撑：`import_dedup_is_idempotent` / `concurrent_import_same_identity_and_fingerprint_is_idempotent` / 同 identity 异内容明确冲突拒绝）
- [x] P16 簇门禁脚本落地且通过（`scripts/p16-gate.sh`：独立 `CARGO_TARGET_DIR=target/gates` + 跑完清理；本次校准复跑 crates-test / crates-clippy / official-chain / schema-check 四类全 PASS，隔离目录已清理）

**相关文档**：[sessions](../docs/features/sessions.md) · [agent-events (P0-3)](P0-3-event-model.md) · [git-diff](../docs/features/git-diff.md) · [review-engine (P16-8)](P16-8-review-engine.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；JSON/JSONL 解析复用基线 `serde_json`（P2-3/P5-9 同款）；外部 diff 导入期原样保留，评审锚点复用 `agent-domain` 的 canonical 类型（Review consumer 出现后由上层复用 Review core 锚定，见步骤 4）。导入扩展在 `session-store` 内完成，不新建 crate。簇门禁脚本约定：

```bash
# 已落地的可执行门禁（替代早期 plan 中的 PowerShell 示例）：
# 覆盖 P16 crates test / clippy(-D warnings) / 正式链（app-service check +
# agent-engine、app-service 的 workflow_events 回归）/ schema-typegen --check，
# 独立 CARGO_TARGET_DIR=target/gates，结束即清理。
./scripts/p16-gate.sh
```

## 校准记录（2026-08-12）

依据 [p16-review](../docs/review/p16-review.md) 评审结论与当前工作区 remediation 状态校准：四来源解析、逐条 unknown 保留（Raw → `Diagnostic`）、顶层 `unknown_fields` 仅进 `CompatImportReport` 未持久化、append-only 不破坏既有事件、结构校验 + Secret 拒绝、单事务原子导入（Session + identity + 全部 event/projection 整批回滚、零残留可重试）、session-scoped 的 run/message/tool ID（schema v6 `compat_import_identity`）、幂等去重与冲突拒绝、tool arguments 保留为 `ToolCallArgumentsDelta`，以及独立 `CARGO_TARGET_DIR` 门禁脚本与清理——均属库级实现且有测试支撑（19 passed），保留 **TargetVerified（library/core）**；「core-api/CLI 导入入口、导入历史查询、格式自动探测、真实 reducer 状态机 replay 校验」未达成，登记 **P16-10** 并映射后续任务 #T8。

验证记录：`scripts/p16-gate.sh` 四类全 PASS（crates-test / crates-clippy / official-chain / schema-check；独立 `target/gates` 跑完已清理）；session-store 测试 57 passed（含 compat 导入 19 例：连续两会话、跨来源相同 tool ID、中途失败零残留、并发幂等与冲突）。

```text
Validation Level: L2（P16 簇门禁脚本，文档校准任务附带复跑）
Affected crates: none（本任务仅改 plan/P16-9 文档）
Validated: scripts/p16-gate.sh；session-store compat 定向测试
Targeted regressions: 原子导入/ID scope/identity 去重；结构校验重命名
Full workspace gate: NOT RUN（未命中升级条件）
```
