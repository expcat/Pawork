# Phase 5 Review：Session 树、Compaction 与上下文裁剪

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树仅含未跟踪的 REVIEW-P*.md，无源码改动）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 5 的 9 个任务（P5-1 ~ P5-9）的完成情况、所引入包是否合适、是否存在更优替代或自实现替换的必要；基线偏差（声明未引用 / 引入未登记）；漏洞与优化点一并列出。格式对齐 [REVIEW.md](../../REVIEW.md)。

### 1. 结论摘要

1. **完成度基本可信**：P5-1 ~ P5-9 全部 🟢。三个交付 crate（`session-store`、`compaction-engine`、`context-engine`）于 2026-08-08 复跑 `cargo test` 共 **63 项测试全部通过**（10 + 26 + 27）；`clippy -D warnings` 与 `fmt --check` 干净。各 plan 的验收项大多已勾选并有对应测试。
2. **包选型无问题**：三个 crate 实际引用的包（`serde` / `serde_json` / `thiserror` / `rusqlite` / `tokio` / `tiktoken-rs`）全部是基线内依赖，**Phase 5 没有引入任何新依赖**，因此不存在「声明未引用 / 引入未登记」的 Phase 5 新增偏差。基线已把 Compaction 引擎、JSONL 流式解析划为完全自实现，落地与此一致。
3. **两个正确性中风险值得在多分支上线前处理**：（V1）压缩引擎 `compact()` 用 `replay_events` 跨分支读取事件，导致 `replaced_range` 与 recovery fork 引用的 `branch_id` 和实际折叠的事件集合不对应；（V2）`import_session` 把导出里**所有分支**的事件都经 `append_event(active_branch)` 回灌，多分支会话往返后事件→分支归属丢失（现有往返测试只覆盖单分支，故未暴露）。
4. **Pi 导入器两处实现缺陷**：（V3）未知字段收集循环用 `unknown_entries.insert(0, …)` 固定写 key 0，多条未知记录互相覆盖，与「保存未知字段」验收项相悖，且测试未断言该报告内容；（V4）`ModelSwitch` 只计数不落事件，模型切换信息实际未还原；（V5）`import_pi_jsonl` 用同步 `std::fs::read_to_string` 读整个文件，阻塞 async 线程且无大小上限。
5. **能力已建但尚未接线**：`compaction-engine` 与 `context-engine` 在整个 workspace 中**没有任何消费者**（仅各自 Cargo.toml 出现），`trim_tool_result` 也未进入 `ContextBuilder`。与 Phase 1 的 `app-service` 骨架同理，属预期（接线在 `agent-engine`，后续阶段完成），但意味着这些路径尚无集成验证。
6. **中文场景的 token 估算偏差**（V6）：`HeuristicEstimator` 默认 `chars/4`、`estimate_text_tokens` 同样 `chars().count()/4`，对 CJK 文本会把 token 数低估约 4–6 倍；tiktoken（OpenAI 系）不受影响，问题集中在 Anthropic/Gemini 的启发式路径，当前 Provider 未接线为潜伏风险。

### 2. P5 任务完成情况核对表

| 任务 | 交付 crate / 模块 | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P5-1 Session Tree / Fork | `session-store::session_tree` | 🟢 | [fork_from_event](../../crates/session-store/src/session_tree.rs:35) 从任意事件分叉并校验事件存在/分支重名；[events_by_branch](../../crates/session-store/src/session_tree.rs:134) 按 branch+sequence 分页读取，大 session 不全量加载 |
| P5-2 Branch 切换 | `session-store::event_store` | 🟢 | [switch_branch](../../crates/session-store/src/event_store.rs:80) 切换 active branch；[append_event](../../crates/session-store/src/event_store.rs:129) 内 `active_branch != branch_id` 即拒写，并发写受保护 |
| P5-3 Resume/归档/删除/重命名 | `session-store::lifecycle` + `lib` | 🟢 | rename/archive/unarchive/resume/delete 齐全；[acquire_lease](../../crates/session-store/src/lifecycle.rs:169)/renew/release 带过期抢占；[integrity_check](../../crates/session-store/src/lifecycle.rs:321) 只读检测 sequence 间隙与 parent 缺失；[open_read_only](../../crates/session-store/src/lib.rs:62) 提供损坏后只读恢复入口 |
| P5-4 搜索 / 标签 | `session-store::search` | 🟢 | add/set/remove/list 标签（小写归一、去重）；search_sessions 命中标题/标签/内容并按维度去重 |
| P5-5 Compaction 引擎 | `compaction-engine::engine` | 🟢（决策态） | [compact](../../crates/compaction-engine/src/engine.rs:85) 读事件、Fork recovery branch（[create_branch](../../crates/compaction-engine/src/engine.rs:112)）、应用保留策略、产出版本化快照；快照版本化见 [snapshot.rs](../../crates/compaction-engine/src/snapshot.rs) |
| P5-6 压缩保留策略 | `compaction-engine::retention` | 🟢 | [apply](../../crates/compaction-engine/src/retention.rs:115) 纯函数：system 永留、最近 N 轮、未解决任务、用户约束、修改文件、pending/failed tool call；golden session 场景见 engine 测试 |
| P5-7 Tool Result 裁剪 | `context-engine::tool_result_trim` | 🟢（逻辑）/ ⚠️（未接线） | [classify](../../crates/context-engine/src/tool_result_trim.rs:54) 四级；[trim_tool_result_with](../../crates/context-engine/src/tool_result_trim.rs:151) 大/超大转 ArtifactReference 并暂存 `retained_full`；但 `ContextBuilder` 未调用，全 workspace 无消费者 |
| P5-8 Export / Import | `session-store::export_import` | 🟢（单分支）/ ⚠️（多分支） | [SessionExport](../../crates/session-store/src/export_import.rs:48) 带 `schema_version`；[import_session](../../crates/session-store/src/export_import.rs:168) 重建；往返测试仅单分支 |
| P5-9 Pi JSONL Importer | `session-store::pi_import` | 🟢 / ⚠️ | header/message/tool/model/compaction/branch 解析齐全、保留未知字段、不改原文件（测试断言 before==after）；但 V3/V4/V5 三处缺陷（见 §5） |

**门禁证据（2026-08-08 复核，基线 `67d6c4d`）**：

- `cargo test -p session-store -p compaction-engine -p context-engine`：**63 passed / 0 failed**（compaction-engine 10、context-engine 26、session-store 27）。
- `cargo clippy -p session-store -p compaction-engine -p context-engine --all-targets -- -D warnings`：干净。
- `cargo fmt --all -- --check`：干净。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `rusqlite` | 0.32（workspace） | P5-1~4/8/9（session-store 全模块） | 分支树、租约、搜索、导入导出全走 SQLite Actor 绑定层 | **保留** |
| `tiktoken-rs` | 0.6（workspace） | P5-7 所在的 `context-engine::token` | OpenAI 系精确计数（[TiktokenEstimator](../../crates/context-engine/src/token.rs:120)），与基线「仅对 OpenAI 系精确」一致 | **保留** |
| `serde` / `serde_json` / `thiserror` / `tokio` | 基线版本 | 全局 | 基础设施 | **保留** |

#### 3.2 自实现判断

基线把 **Compaction 引擎（P5-5/6）** 与 **JSONL 流式解析（P5-9，serde_json 逐行）** 列为完全自实现，落地完全吻合：

- `retention::apply` 是纯函数、确定性（BTreeSet 按 `EventId` 排序输出）、无 IO，保留语义完全可控——正确选择。
- `pi_import` 逐行 `serde_json::from_str` + 宽松字段识别，未引入额外 JSONL 框架——符合基线「serde_json 逐行即可」。
- `tool_result_trim` 的分级裁剪是 Pawork 特定语义（小/中/大/超大 + ArtifactReference 占位），无对应现成包，自实现正确。

**结论：Phase 5 范围内没有任何「引用面小、自实现更划算」的第三方包，不需要自实现替换，也不需要新增依赖。** 唯一可商榷的是 §5 V6 的中文 token 启发式（属参数调优，不涉及换包）。

### 4. 基线偏差清单

**Phase 5 三个 crate 引入的偏差：零。** 所有依赖均为基线已登记项。

REVIEW.md §4 记录的 workspace 级历史偏差在本基线仍存在（属 Phase 1/6/7 范畴，不在本次修复目标内，仅同步现状）：

| 类型 | 项 | 现状 | 备注 |
| --- | --- | --- | --- |
| 声明未引用 | `uuid`、`tracing-appender`、`similar` | 仍零引用（`similar` 唯一命中是 [parser.rs:8](../../crates/diff-service/src/parser.rs:8) 注释里的单词 "similarity"，非 crate 使用） | 与 REVIEW.md 一致，未恶化 |
| 引入未登记 | `parking_lot`、`tempfile`、`base64`、`rand`、`sha2`、`url` | 仍仅在各 crate Cargo.toml，未回填 workspace 基线 | 与 REVIEW.md 一致 |

**建议**：沿用 REVIEW.md §6 的「一次性基线清理小任务」处理，不与 Phase 5 混改。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V10）。

#### V1 [正确性·中] 压缩引擎跨分支读取事件

[engine.rs:93](../../crates/compaction-engine/src/engine.rs:93) 调用 `replay_events(session_id, 1, usize::MAX)`，而 `replay_events` 的查询不带 `branch_id`（[event_store.rs:229](../../crates/session-store/src/event_store.rs:229)，仅按 `session_id + sequence`），读出的是**全 session 所有分支**的事件。但 `compact` 的入参 `branch_id` 同时用作 recovery branch 的 parent（[engine.rs:112](../../crates/compaction-engine/src/engine.rs:112)）与命名，`replaced_range` 也据此计算。多分支会话下被折叠的事件集合与 `branch_id` 不对应，recovery branch 的 fork 点也未必在目标分支上。当前因无消费者未触发，但这是多分支压缩的正确性隐患。**建议**：压缩读取改用 `events_by_branch(session_id, branch_id, …)`，或为 `replay_events` 增加 branch 过滤重载。

#### V2 [正确性·中] Export/Import 多分支往返丢失事件→分支归属

[import_session](../../crates/session-store/src/export_import.rs:168) 在重建分支树后，对导出的**全部事件**统一执行 `append_event(export.active_branch.clone(), event.clone())`（[export_import.rs:201](../../crates/session-store/src/export_import.rs:201)）。导出侧 `export_session` 是跨分支读取事件（按 sequence 升序），因此非 active 分支的事件在导入后全部被写入 active branch，`session_events.branch_id` 与导出前不一致。「往返等价」验收在多分支下不成立；现有 `export_round_trips_through_json_and_import` 仅构造单分支会话，故未暴露。**建议**：导入时按事件原始 `branch_id`（需在导出 schema 中携带每事件的 branch，或按分支分组重建）分派；并补一个多分支往返测试。

#### V3 [正确性·中] Pi 导入器未知字段收集互相覆盖

[pi_import.rs:412](../../crates/session-store/src/pi_import.rs:412) 的未知字段收集循环执行 `report.unknown_entries.insert(0, format!("{}={}", k, v))`，key 恒为 `0`（BTreeMap），多条未知记录互相覆盖，最终只保留最后一条。与 P5-9 验收项「保存未知字段」相悖；测试 `parse_recognizes_known_kinds_and_preserves_unknown_fields` 只检查单条 `unknown_fields`，且 `import_pi_*` 测试从未断言 `report.unknown_entries` 内容，故未捕获。**建议**：key 改用行号或递增序号；补多条未知记录的导入断言。

#### V4 [正确性·低] ModelSwitch 只计数不持久化

[pi_import.rs:369-370](../../crates/session-store/src/pi_import.rs:369) 对 `PiPayload::ModelSwitch` 仅 `report.imported_model_switches += 1`，不追加任何事件，模型切换信息未真正还原进会话（plan「还原会话结构」目标部分落空）。根因是 `AgentEvent` 无对应变体。**建议**：若需保留，扩一个 `ModelSwitched` 事件或在 message metadata 中标注；否则在报告里明确「未持久化」。

#### V5 [阻塞异步·中] Pi 导入同步读取整个文件

[import_pi_jsonl](../../crates/session-store/src/pi_import.rs:270) 用 `std::fs::read_to_string` 一次性读入全部内容，再走 `import_pi_jsonl_lines`。该调用位于 async 方法内、且其后所有 DB 写入都经 Actor 异步化，唯独文件读取是同步阻塞；大 Pi 文件（历史长会话）会阻塞 runtime 工作线程，且无大小上限（内存压力）。**建议**：改 `tokio::fs::read_to_string` 或 `spawn_blocking`，并对超大文件改逐行流式读取（与基线「JSONL 流式解析」语义一致）。

#### V6 [估算偏差·中] 启发式 token 估算对 CJK 严重低估

[HeuristicEstimator](../../crates/context-engine/src/token.rs:167) 默认 `chars_per_token = 4`（[token.rs:188](../../crates/context-engine/src/token.rs:188) `chars.div_ceil(chars_per_token)`），压缩引擎的 [estimate_text_tokens](../../crates/compaction-engine/src/engine.rs:160) 同样是 `chars().count() / 4`。中文字符约 1–2 token/字，按 4 字/token 估算会把 token 数低估约 4–6 倍 → 预算/压缩触发判定偏乐观，非 OpenAI 模型有上下文溢出风险。tiktoken 路径（OpenAI 系）BPE 正确，不受影响。**建议**：对启发式路径按脚本（CJK/拉丁）分流设 ratio，或保守取 `chars_per_token ≈ 1.5`；压缩统计的 `estimate_text_tokens` 复用 `TokenEstimator` 而非硬编码 /4。

#### V7 [搜索精度·低] 内容搜索命中原始 JSON

[search.rs:205](../../crates/session-store/src/search.rs:205) 内容匹配用 `m.message_json LIKE ?1`，会对 `message_json` 的字段名/`role`/`metadata` 等结构噪声误命中（如搜 "content"/"role" 命中所有消息），且 snippet 是 `substr(m.message_json,1,120)`（[search.rs:203](../../crates/session-store/src/search.rs:203)）即原始 JSON 片段，可读性差。模块注释已说明暂未用 FTS5 的理由（sessions 主键为 TEXT、无整数 rowid）。**建议**：内容匹配改为抽取 `Text` 部分后再 LIKE（或在 projection 里冗余一份纯文本列），snippet 同源；迁移到整数 rowid 后再上 FTS5。

#### V8 [正确性·低] replay/tail 不感知分支

[replay_events](../../crates/session-store/src/event_store.rs:229) 与 [tail_events](../../crates/session-store/src/event_store.rs:257) 查询均不带 `branch_id`，会混排多分支事件。P5-1 已新增分支感知的 `events_by_branch`，但旧的 session 级重放 API 未收敛，调用方易误用（V1 即为其下游表现）。**建议**：明确 replay/tail 的「整 session」语义并文档化，或提供分支感知重载，避免上下文重建误混分支。

#### V9 [健壮性·低] 死错误变体

[SessionStoreError::EventSessionMismatch](../../crates/session-store/src/lib.rs:130) 在全仓库无任何构造点（rg 仅命中声明本身），属死代码。**建议**：移除或补上 append 路径的 session 一致性校验并配测试。

#### V10 [正确性·低] Tool Result 裁剪不计二进制内容

[byte_len_of_tool_result](../../crates/context-engine/src/tool_result_trim.rs:111) 对 `Image` 等非文本 part 以 `0` 计（[tool_result_trim.rs:118](../../crates/context-engine/src/tool_result_trim.rs:118)），故「图片为主、文本很少」的结果会被判为 `Small` 而原样进入上下文，与「超大输出不无限进入上下文」验收项在二进制场景下存在缺口。注释说明二进制由调用方在写 Blob 时另行管理，但裁剪入口本身未设防。**建议**：在分类时对二进制 part 也给一个估算权重（如按 base64 长度或固定成本），或由调用方在传入前先把二进制转 Artifact。

### 6. 优化建议（按优先级）

#### P0（多分支能力正式上线前处理）

1. **V1 + V8**：压缩读取改 `events_by_branch`，并收敛 replay/tail 的分支语义——这是「分支即一等公民」能否成立的关键，当前是潜伏正确性 bug。
2. **V2**：导入按原始 branch 分派事件并补多分支往返测试，否则 export/import 不能作为可信迁移/备份通道。

#### P1（近期排期）

3. **V3**：Pi 未知字段收集 key 改行号（一行改动）+ 断言。
4. **V5**：Pi 导入改异步/流式读取，与基线「JSONL 流式解析」对齐。
5. **接线补齐**：把 `trim_tool_result` 接入 `ContextBuilder`（或 agent-engine 调用点）、把 `CompactionEngine` 接入 agent loop 的超限处理路径——当前两个 crate 零消费者，属「已实现未集成」，需在对应阶段补端到端验证。
6. **V6**：启发式 token 估算针对 CJK 调参，压缩统计复用 `TokenEstimator`。

#### P2（顺手/评估项）

7. **V4**：明确 ModelSwitch 的持久化策略（新事件 or metadata）。
8. **V7**：内容搜索改按抽取文本匹配，snippet 同源；评估 FTS5 迁移窗口。
9. **V9**：移除死变体 `EventSessionMismatch`，或补 session 一致性校验。
10. **V10**：Tool Result 裁剪对二进制 part 给估算权重。
11. **文档同步**：[context-engine/compaction.rs](../../crates/context-engine/src/compaction.rs) 注释仍写「压缩引擎位于 compaction-engine（尚未实现）」，已过时；`context-engine::CompactionReason` 与 `compaction-engine::CompactionReason` 同名异构（后者多一个 `Manual`），建议统一或明确映射，避免调用方混淆。

### 7. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V2 立项（多分支正确性，影响压缩与迁移两条主线）。
2. V3/V5 作为 Pi 导入器的小修复合并提交。
3. 评估 `compaction-engine` / `context-engine` 接入 `agent-engine` 的时机与端到端测试方案。
4. 中文 token 启发式调参（V6）作为 Provider 接线（Phase 6）的前置项。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 3 个 Phase-5 crate 的测试与静态门禁；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更，未修改任何代码/配置。*

---

## 修复记录（review-remediation）

> Phase 5 · Session、Branch 与 Compaction · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P5-1 ~ P5-9

**最终目的**：消除 [REVIEW.md](../../REVIEW.md) §5（Phase 5）评审发现的多分支正确性隐患、Pi 导入器缺陷、CJK token 估算偏差与文档/死代码漂移——让压缩与 export/import 在多分支下保持事件→分支归属正确，Pi 导入器未知字段与异步读取正确，启发式 token 估算对 CJK 不严重低估。

**涉及范围**：`session-store`（event_store/export_import/pi_import/search/lib）、`compaction-engine`、`context-engine`（token/tool_result_trim/compaction）、`docs/features/`

### 细分步骤（分组）

#### A. 多分支正确性（V1 / V2 / V8）

1. **V1 压缩跨分支读取**：`compaction-engine` `compact()` 读取改用 `events_by_branch(session_id, branch_id, …)`，或为 `replay_events` 增加 branch 过滤重载，使折叠事件集合与 `branch_id` 对应。目的：多分支压缩不偏移 recovery fork 点。
2. **V2 export/import 分支归属**：`import_session` 按事件原始 `branch_id` 分派（导出 schema 携带每事件 branch，或按分支分组重建），补多分支往返测试。目的：export/import 作为可信迁移/备份通道。
3. **V8 replay/tail 分支语义**：明确 `replay_events`/`tail_events` 的「整 session」语义并文档化，或提供分支感知重载。目的：避免上下文重建误混分支（V1 的下游表现）。

#### B. Pi 导入器修复（V3 / V4 / V5）

4. **V3 未知字段覆盖**：`pi_import` 未知字段收集 key 改用行号或递增序号（不再恒为 0），补多条未知记录的导入断言。目的：兑现「保存未知字段」验收。
5. **V4 ModelSwitch 持久化策略**：明确 ModelSwitch 还原策略——扩 `ModelSwitched` 事件或在 message metadata 标注；若不持久化则在报告明确「未持久化」。目的：模型切换信息如实还原或如实标注。
6. **V5 异步/流式读取**：`import_pi_jsonl` 改 `tokio::fs::read_to_string` 或 `spawn_blocking`，超大文件逐行流式读取。目的：与基线「JSONL 流式解析」对齐，不阻塞 async 线程。

#### C. token 估算（V6）

7. **V6 CJK 启发式调参**：启发式路径按脚本（CJK/拉丁）分流设 ratio，或保守取 `chars_per_token ≈ 1.5`；压缩统计的 `estimate_text_tokens` 复用 `TokenEstimator` 而非硬编码 /4。目的：消除 CJK 文本 token 数 4–6 倍低估，避免非 OpenAI 模型上下文溢出。

#### D. 搜索与裁剪（V7 / V10）

8. **V7 内容搜索噪声**：内容匹配改抽取 `Text` 部分后再 LIKE（或冗余纯文本列），snippet 同源；迁移整数 rowid 后再评估 FTS5。目的：消除字段名/role 误命中与原始 JSON snippet。
9. **V10 二进制裁剪**：tool result 分类对 `Image` 等非文本 part 给估算权重（按 base64 长度或固定成本），或由调用方在传入前转 Artifact。目的：二进制为主的结果不被误判 Small。

#### E. 死代码（V9）

10. **V9 死变体**：移除无构造点的 `SessionStoreError::EventSessionMismatch`，或补 append 路径 session 一致性校验并配测试。目的：消除死代码或补齐其用途。

#### F. 文档漂移

11. **过时注释/同名异构**：修正 `context-engine/compaction.rs`「压缩引擎尚未实现」过时注释；明确 `context-engine::CompactionReason` 与 `compaction-engine::CompactionReason`（多一个 `Manual`）的映射，避免调用方混淆。目的：文档与实现一致。

### 主要产出物

- 压缩读取改 `events_by_branch`；import 按原始 branch 分派 + 多分支往返测试；replay/tail 语义文档化
- Pi 未知字段 key 修正 + ModelSwitch 策略 + 异步读取；CJK 启发式调参 + estimate 复用 TokenEstimator
- 内容搜索抽取文本匹配；二进制裁剪权重；死变体移除；过时注释/同名异构订正

### 验收标准（保留 REVIEW 追踪编号）

- [x] **V1**：多分支会话压缩后 `replaced_range` 与 recovery fork 的 `branch_id` 与折叠事件集合对应（多分支测试）
- [x] **V2**：多分支 export→import 往返后事件 `branch_id` 与导出前一致（多分支往返测试）
- [x] **V8**：`replay_events`/`tail_events` 语义文档化或提供分支感知重载
- [x] **V3**：Pi 导入多条未知记录全部保留（断言 `report.unknown_entries` 内容）
- [x] **V4**：ModelSwitch 策略明确（持久化事件/metadata 或报告标注未持久化）
- [x] **V5**：`import_pi_jsonl` 不在 async 中同步读整文件（异步/流式）
- [x] **V6**：CJK 文本启发式 token 估算不再 4–6 倍低估；压缩统计复用 `TokenEstimator`
- [x] **V7**：内容搜索不命中字段名/role 噪声；snippet 为可读文本（用例）
- [x] **V10**：二进制为主的 tool result 不被误判 Small（分类测试）
- [x] **V9**：`EventSessionMismatch` 移除或补一致性校验 + 测试
- [x] **文档**：`compaction.rs` 过时注释修正；`CompactionReason` 同名异构映射明确
- [x] **快速验证**：只运行 session/branch/compaction/import 受影响 crate 的定向测试与最小重放 smoke；workspace 全量门禁延后到 Core 主干 L2

### 验证记录（2026-08-09）

- `cargo test -p session-store -p compaction-engine -p context-engine`
- `cargo clippy -p session-store -p compaction-engine -p context-engine --all-targets -- -D warnings`
- Export schema v2 多分支往返、v1 读取迁移、Pi 多未知行/ModelSwitch、分支压缩、CJK 估算与非文本裁剪均有回归测试。

**相关文档**：[REVIEW.md](../../REVIEW.md) §5 · [ADR-005 仅 Pi JSONL 导入](../../docs/adr/ADR-005-pi-jsonl-import-only.md) · [context](../../docs/features/context.md) · [ROADMAP](../../ROADMAP.md)

> 接线提示（2026-08 review）：`compaction-engine`/`context-engine` 当前零消费者，接线（trim_tool_result 接入 ContextBuilder、CompactionEngine 接入 agent loop 超限处理）属后续阶段，本任务修复其内部缺陷；接线端到端验证随 Provider Loop 接线（P3-11）一并补。
