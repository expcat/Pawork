# P16-8：Review Engine（行锚点评审与解决）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：library/core verified，host composition deferred） · 依赖：P0-3、P7-3、P1-8

**最终目的**：提供一套通用的、以行锚点为核心的评审引擎，让 Reviewer（人或 Agent）对工作区、commit 或 PR 留下结构化 `ReviewFinding`、`SuggestedPatch` 与可发布的 `PRComment`，并跟踪 resolution 生命周期（`open → addressed → resolved / wontfix`），使评审意见可定位、可指派、可修复、可闭环。

**涉及范围**：新增 `review-engine`；复用 `diff-service`（P7-3，行锚点解析与稳定化）、`file-index`（P1-8，路径解析）、`agent-events`、`session-store`

## 细分步骤

1. **评审领域模型** —— 目的：定义 `ReviewSession`、`ReviewFinding`（锚点 `file:line` + 范围 + severity + 正文 + 证据 + 指派）、`SuggestedPatch`、`PRContext` / `PRComment` 与 `Resolution`；所有外部 PR 字段经 adapter 输入，不让 review-engine 依赖具体托管平台。
2. **行锚点稳定化** —— 目的：锚点经 `diff-service`（P7-3）解析为带上下文的稳定位置（文件 + 邻近语义指纹），在文件后续编辑后尽量重新定位（re-anchor），漂移时标注 `stale` 而非静默失效。
3. **SuggestedPatch 与 resolution 生命周期** —— 目的：建议补丁先 dry-run/checkpoint，再由现有 Tool/Policy 执行；状态转移均为 canonical event，解决时关联修复 commit/patch/Agent Run，形成「finding → suggestion → fix → resolution」可追溯链。
4. **批量、PR 与聚合** —— 目的：支持按文件/严重度/状态聚合，导入 PR diff/commit 范围并生成待发布评论；实际向托管平台发送评论是独立 adapter/用户动作，不由 Review Engine 自行产生外部副作用。
5. **ForgeAdapter（P2 扩展）** —— 目的：定义独立 `ForgeAdapter { GitHub, GitLab, Generic }` 接口，负责拉取 PR/MR context、把平台字段映射为 `PRContext`，并在用户显式 publish 后发送 `PRComment` / resolution。adapter 依赖通用 `http-runtime` / auth，`review-engine` 不依赖平台 SDK、名称或远程副作用实现。
6. **只读与权限** —— 目的：评审引擎对工作区只读（仅读取文件做锚点），写动作交由既有工具并受 policy 约束，评审本身不赋权。
7. **查询面** —— 目的：`core-api` 暴露评审列表/过滤/订阅，GUI/CLI 呈现行内评审与解决状态。
8. **定向 / Mock 测试** —— 目的：锚点解析与 re-anchor、漂移标 `stale`、resolution 转移与修复关联，以及 Forge adapter 未经 publish 不产生外部请求。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `review-engine`：行锚点评审 + re-anchor + resolution 生命周期
- 评审相关 canonical event 与查询面
- 定向测试

## 验收标准

- [x] 评审意见带 `file:line` 锚点，文件编辑后可 re-anchor，漂移时标 `stale` 不静默失效（测试支撑：`reanchor_relocates_after_edit_and_marks_stale_on_drift` / `anchor_rejects_traversal_and_out_of_range_lines`）
- [ ] 部分达成：evidence/severity/`SuggestedPatch` 已事件化且重放完整（评审 remediation 已修，测试支撑：`finding_lifecycle_with_rich_fields_and_fix_ref` / `replay_rebuilds_identical_lifecycle_state`）；补丁应用走 checkpoint/policy 未接线（现仅 dry-run）→ 未达成，登记 **P16-10 #T7**
- [ ] 部分达成：`PRContext`/`PRComment` 平台无关，生成评论不等于自动发布、publish 仅显式调用（测试支撑：`generating_comments_never_publishes_and_publish_is_explicit` / `publish_failure_leaves_no_event`）；无真实 GitHub/GitLab adapter，`GenericForgeAdapter::publish_comment` 仅生成本地合成 ID、无远端副作用 → 未达成，登记 **P16-10 #T7**
- [x] resolution 状态转移为 canonical event，解决可关联修复 commit/patch/Run（测试支撑：`resolution_state_machine_rejects_illegal_transitions` / `finding_lifecycle_with_rich_fields_and_fix_ref` 的 `fix_ref` 断言）
- [x] 评审引擎对工作区只读，写动作走既有工具与 policy（测试支撑：`engine_never_writes_to_workspace` / `patch_dry_run_validates_and_applies_in_memory_without_writes`；锚点解析仅 `fs::read_to_string`）
- [x] 可按文件/严重度/状态聚合（测试支撑：`aggregate_by_file_severity_status`；供 Plan 评审/Agent 评审消费的适配与查询面未接 → 归 P16-10 #T7）
- [ ] 部分达成：`ForgeAdapter` 可替换、无 GitHub/GitLab 名称分支、publish 显式（`review_core_has_no_platform_name_branch` / `generic_adapter_works_identically_without_platform_branch`）；真实平台 adapter 未实现 → 未达成，登记 **P16-10 #T7**

**相关文档**：[git-diff](../docs/features/git-diff.md) · [workspace-index](../docs/features/workspace-index.md) · [plan-review-approval (P16-2)](P16-2-plan-review-approval.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；行锚点复用 `diff-service`（基线 `similar`），路径解析复用 `file-index`。新 crate `review-engine` 依赖方向：`agent-domain → review-engine → app-service`。

## 校准记录（2026-08-12）

依据 [p16-review](../docs/review/p16-review.md) 评审结论与当前工作区 remediation 状态校准：行锚点/re-anchor/stale、resolution reducer、聚合、工作区只读与补丁 dry-run，以及 evidence/assignee/suggested_patch/fingerprint 随 `FindingOpened` 事件完整重放（评审 remediation 已修）属库级实现且有测试支撑，保留 **TargetVerified（library/core）**；「checkpoint/policy 接线、真实 Forge adapter（远端 ID 持久化）、core-api 查询/UI、Plan 评审消费适配」未达成，登记 **P16-10** 并映射后续任务 #T7。

验证记录：`scripts/p16-gate.sh` 四类全 PASS（独立 `target/gates` 跑完已清理）；review-engine 定向测试 18 passed（含旧流 JSON 兼容）。

```text
Validation Level: L2（P16 簇门禁脚本，文档校准任务附带复跑）
Affected crates: none（本任务仅改 plan/P16-8 文档）
Validated: scripts/p16-gate.sh；review-engine 定向测试
Targeted regressions: FindingOpened 富字段重放完整性
Full workspace gate: NOT RUN（未命中升级条件）
```
