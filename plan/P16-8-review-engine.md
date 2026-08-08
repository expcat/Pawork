# P16-8：Review Engine（行锚点评审与解决）

> Phase 16 · Modern Agent Workflow · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-3、P7-3、P1-8

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

- [ ] 评审意见带 `file:line` 锚点，文件编辑后可 re-anchor，漂移时标 `stale` 不静默失效
- [ ] `ReviewFinding` 可携带 evidence、severity 与 `SuggestedPatch`；补丁应用仍走 checkpoint/policy
- [ ] PR context/comment 通过平台无关 adapter 表达，生成评论不等于自动发布
- [ ] resolution 状态转移为 canonical event，解决可关联到修复 commit/patch/Run
- [ ] 评审引擎对工作区只读，写动作仍走既有工具与 policy
- [ ] 可按文件/严重度/状态聚合，供 Plan 评审与 Agent 评审复用
- [ ] ForgeAdapter 可替换且仅在显式 publish 后产生外部副作用；Review Engine 无 GitHub/GitLab 名称分支

**相关文档**：[git-diff](../docs/features/git-diff.md) · [workspace-index](../docs/features/workspace-index.md) · [plan-review-approval (P16-2)](P16-2-plan-review-approval.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；行锚点复用 `diff-service`（基线 `similar`），路径解析复用 `file-index`。新 crate `review-engine` 依赖方向：`agent-domain → review-engine → app-service`。
