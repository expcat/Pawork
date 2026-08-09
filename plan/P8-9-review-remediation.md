# P8-9：Phase 8 评审修复（REVIEW remediation）

> Phase 8 · Skills、Prompts 与 Instructions（resource-loader）· 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P8-1 ~ P8-8

**最终目的**：执行 [docs/review/p8-review.md](../docs/review/p8-review.md) 的「减少」导向建议——删死抽象、合并重复校验、简化 Skills 依赖引擎，并为双优先级表加守护测试；把「解析后暂无消费者」的字段在文档中明确标记为 deferred-consumer，避免被误读为已生效。零端到端消费者（§2）、ResourceBundle 双状态（§3.4）、session/run → AdHocInstructions 泄漏（§4.2）、热重载/诊断视图的「过早基础设施」（§4.3）均属于 P13 Host-Run 接线才可观测的能力，按评审结论显式延后，不在本任务范围。

**涉及范围**：`resource-loader`（loader/lib/agents/source/diagnostics/request/skills/templates/profiles/io）、`context-engine`（resources.rs 测试模块）、`docs/features/skills.md`

## 处置策略（按评审 §5 / §6 矩阵）

- **现在修复（落地）**：§3.3 死 API/死字段删除；§3.1 Skills 依赖引擎简化；§3.5 重复校验/解析合并；§4.1 双优先级表 cross-check；§3.2 deferred-consumer 文档标记。
- **显式延后（P13 接线时处理）**：§2 零端到端消费者（首次通电验证）；§3.4 ResourceBundle 双重状态（与接线一起做更稳）；§4.2 session/run → AdHocInstructions 的 tier-prefix 隐性依赖（接线时重构 ContextSource）；§4.3 热重载/诊断视图（实现质量已达标，等待消费者）。这些在 `docs/features/skills.md` 新增的「已解析但暂无消费者」小节中明确标注。

## 细分步骤（分组）

### A. 死抽象与死字段删除（§3.3，零风险）

1. **LoadResources trait**（loader.rs）：单实现、零 trait-object/generic 使用。删除 trait + impl，把 `load` 方法体原样迁入 `impl ResourceLoader` 作为 inherent `pub fn load`，签名与行为不变。
2. **ResourceBundle::diagnostic_view / ResourceLoader::options getter**（loader.rs）：零调用。删除方法；`ResourceDiagnosticView` 类型与 `options` 字段保留（前者 P13 接线需要，后者内部仍用）。
3. **ResourceInstructionKind::priority**（loader.rs）：由 `const fn` 提升为 `pub const fn`，供 §4.1 cross-check 测试跨 crate 调用。
4. **AgentsDocument::{new, into_documents, iter, is_empty}**（agents.rs）：零调用。删除；`from_documents / documents / nearest / len` 保留（有调用）。`len` 因 `is_empty` 已删而触发 `clippy::len_without_is_empty`，加 `#[allow]` 并注释说明。
5. **PromptTemplate::render**（templates.rs）：零调用（活路径是自由函数 `render_candidate`）。删除方法及其 impl 块。
6. **ResourceOrigin::Builtin**（source.rs）：生产代码从不构造，仅 redactor match。删除变体 + diagnostics.rs 的两处 match 臂。
7. **ResourceLimits.max_include_depth**（request.rs）：自述「为后续递归 include 预留」，从不读取。删除字段与 Default 初始化点。

### B. Skills 依赖引擎简化（§3.1，语义保持）

8. **单次 BFS + 复用匹配结果**（skills.rs）：把 `resolve_active`（不动点收敛）+ `bfs_supported`（带 `allowed` 重收敛）+ `collect_dep_issues`（重复 semver 匹配）替换为一次 BFS（`traverse_dependencies`），在遍历中记录每条依赖边的裁决（Valid/Disabled/Missing/VersionMismatch），再由 `dep_issues`（生成诊断）与 `active_from`（生成激活集）复用同一份裁决，无第二次 semver 匹配。`detect_conflicts` 保留。
   - **诚实记录**：评审预估可削减约 113 行，实际净削减约 26 行（157+/183−）。原因——级联拒绝测试（`cascade_rejection_when_inner_dependency_missing` 等）证明「损坏的依赖方必须从激活集中剔除」是不可约简语义，plain BFS 不可达性无法复现；故 `active_from` 仍需一次沿 Valid 边的反向消除（O(V+E)、无重匹配）。评审关于「两者产出的 Rejected 状态完全相同」的前提在这些级联场景下不成立。简化仍达成核心目标（消除不动点循环 + 重复 semver 匹配），且全部 21 项 skills 测试零改动通过。

### C. 重复校验/解析合并（§3.5，低风险）

9. **io.rs 新增三个 `pub(crate)` 共享 helper**：
   - `is_valid_identifier(id: &str, allow_dot: bool) -> bool`：统一 4 份 ASCII 标识符校验（skills id / templates id 允许 `.`；templates 参数名 / profiles 名视调用点传 `allow_dot`）。
   - `is_safe_relative_reference(raw: &str) -> bool`：统一 skills `validate_declared_path` 与 templates `validate_relative_reference` 的路径安全语义（拒绝空、绝对、`..`、Windows 盘符 `C:`、非正常分量）。
   - `parse_toml_resource<T: DeserializeOwned>(content, issue_code, message) -> Result<T, ResourceIssue>`：统一 3 处 TOML 解析样板（skills manifest / templates frontmatter / profiles file），调用方再 `.for_resource(...)` 附加来源。
10. **切换调用点**：skills.rs（3 处）、templates.rs（4 处）、profiles.rs（2 处）改为调用上述 helper，保留原有 issue code/message 字符串逐字节一致。

### D. 双优先级表 cross-check（§4.1，纯测试）

11. **context-engine resources.rs 测试**：新增 `priority_tables_stay_consistent()`，对 7 个直接映射变体断言 `ResourceInstructionKind::priority()` == 对应 `ContextSource::priority()`（AgentProfile=2、UserGlobal=4、Workspace=5、RootAgents=6、PathAgents=7、ActiveSkill(s)=8、PromptTemplate=9），并断言 SessionInstructions=13 / RunInstructions=14 与 AdHocInstructions=14 的有意重映射（注释引用 §4.2）。一旦两表数值漂移即测试失败。

### E. deferred-consumer 文档标记（§3.2，纯文档）

12. **docs/features/skills.md**：新增「已解析但暂无消费者（deferred-consumer）」小节，列出 Skills `parameters/scripts/assets/permissions`、Templates `PromptDefaults.{model,thinking,tools,budget}` 与 `RenderedPrompt.included_files`、AgentProfile `default_provider/default_model`（resource-loader 自有副本；config-service 另有活动副本）、ResourceBundle 结构化字段、ResourceDiagnosticView、ResourceHotReload/watch，并标注 P13 接线后首次可观测。

## 主要产出物

- 删除：`LoadResources` trait、`ResourceBundle::diagnostic_view`、`ResourceLoader::options`、`PromptTemplate::render`、`AgentsDocument::{new,into_documents,iter,is_empty}`、`ResourceOrigin::Builtin`、`ResourceLimits.max_include_depth`。
- 简化：skills.rs 依赖引擎单次 BFS + 裁决复用（净 −26 行，消除不动点循环与重复 semver 匹配）。
- 合并：io.rs 三个共享 helper，9 处调用点收口，重复校验/解析样板消除。
- 测试：§4.1 双优先级表 cross-check（+1）；io.rs helper 单测（+3）。
- 文档：skills.md deferred-consumer 标记。

## 验收标准（保留 REVIEW 追踪章节）

- [x] **§3.3 死 API**：LoadResources / diagnostic_view / options / PromptTemplate::render / AgentsDocument 4 方法 / ResourceOrigin::Builtin / max_include_depth 全部删除，`rg` 全仓库零残留
- [x] **§3.1 Skills 引擎**：依赖解析单次 BFS、无重复 semver 匹配；21 项 skills 测试零改动通过；激活集与诊断（code/message）与改前逐字节一致（reviewer 逐场景追踪确认）
- [x] **§3.5 重复校验**：io.rs 三 helper 收口 9 处调用点；原 issue code/message（`skill_manifest_parse`/`prompt_frontmatter_invalid`/`agent_profile_invalid` 等）逐字节保留
- [x] **§4.1 优先级表**：cross-check 测试落地，7 直接映射变体数值一致，Session/Run→AdHoc 重映射有断言与注释
- [x] **§3.2 deferred-consumer**：skills.md 新增标记小节，覆盖评审 §3.2 全部字段与 §4.3 的 watch/诊断视图
- [x] **显式延后**：§2/§3.4/§4.2/§4.3 在本文与 skills.md 中明确标注归属 P13，未误标完成

## 验证记录（2026-08-09）

- `cargo test -p resource-loader`：54 passed（baseline 51 + io.rs helper 3），0 failed；其中 skills 21 项零改动通过（依赖引擎简化语义保持）。
- `cargo test -p context-engine`：31 passed（baseline 30 + §4.1 cross-check 1），0 failed。
- `cargo clippy -p resource-loader -p context-engine --all-targets -- -D warnings`：通过（agents.rs `len_without_is_empty` 已 `#[allow]` 并注释）。
- `cargo fmt -p resource-loader -p context-engine -- --check` 与 `git diff --check`：通过。
- `git diff --stat`：12 文件、+399/−309，净 +90；其中 skills.rs −26（依赖引擎）、templates.rs −31（render 删除 + 校验合并）。
- 按本任务门禁节奏只执行受影响 crate 的定向门禁；workspace 全量、三平台与发布门禁留待 Core 主干 L2/L3。
- **独立 reviewer 复核**（deepseek_reviewer）：dead-code 声明逐项 `rg` 复核、skills 简化逐场景行为等价追踪、io.rs helper 与原实现逐行比对、§4.1 测试数值对照两表——无阻塞项。唯一 [ISSUE]：`is_safe_relative_reference` 对 `.`/`./`/`C:foo` 等病态输入改为解析期前置拒绝（原为渲染期 `is not a regular file`），属有测试覆盖的有意加固，无真实输入回归，记录于此。

**相关文档**：[REVIEW.md](../REVIEW.md) §Phase 8 · [docs/review/p8-review.md](../docs/review/p8-review.md) · [docs/features/skills.md](../docs/features/skills.md) · [ROADMAP Phase 8](../ROADMAP.md)

> 跨任务协调：本任务仅触碰 resource-loader 内部公共面收缩与 context-engine 测试，不与 P1–P7 修复任务的写集（git-service / diff-service / provider-* / agent-engine 等）重叠；基线表无新增/移除依赖（§3.5 合并是 crate 内 helper，不触及 workspace 基线）。

> 延后项归属：§2（首次端到端验证）、§3.4（ResourceBundle 双状态二选一）、§4.2（session/run → ContextSource 重构）、§4.3（watch/诊断视图接线）统一在 P13 CLI Host 装配时处理，届时优先验证本评审标记的各处；在 docs/features/skills.md「已解析但暂无消费者」小节登记。
