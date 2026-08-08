# P5-10：Phase 5 评审修复（REVIEW remediation）

> Phase 5 · Session、Branch 与 Compaction · 状态：🟡未开始 · 依赖：P5-1 ~ P5-9

**最终目的**：消除 [REVIEW.md](../REVIEW.md) §5（Phase 5）评审发现的多分支正确性隐患、Pi 导入器缺陷、CJK token 估算偏差与文档/死代码漂移——让压缩与 export/import 在多分支下保持事件→分支归属正确，Pi 导入器未知字段与异步读取正确，启发式 token 估算对 CJK 不严重低估。

**涉及范围**：`session-store`（event_store/export_import/pi_import/search/lib）、`compaction-engine`、`context-engine`（token/tool_result_trim/compaction）、`docs/features/`

## 细分步骤（分组）

### A. 多分支正确性（V1 / V2 / V8）

1. **V1 压缩跨分支读取**：`compaction-engine` `compact()` 读取改用 `events_by_branch(session_id, branch_id, …)`，或为 `replay_events` 增加 branch 过滤重载，使折叠事件集合与 `branch_id` 对应。目的：多分支压缩不偏移 recovery fork 点。
2. **V2 export/import 分支归属**：`import_session` 按事件原始 `branch_id` 分派（导出 schema 携带每事件 branch，或按分支分组重建），补多分支往返测试。目的：export/import 作为可信迁移/备份通道。
3. **V8 replay/tail 分支语义**：明确 `replay_events`/`tail_events` 的「整 session」语义并文档化，或提供分支感知重载。目的：避免上下文重建误混分支（V1 的下游表现）。

### B. Pi 导入器修复（V3 / V4 / V5）

4. **V3 未知字段覆盖**：`pi_import` 未知字段收集 key 改用行号或递增序号（不再恒为 0），补多条未知记录的导入断言。目的：兑现「保存未知字段」验收。
5. **V4 ModelSwitch 持久化策略**：明确 ModelSwitch 还原策略——扩 `ModelSwitched` 事件或在 message metadata 标注；若不持久化则在报告明确「未持久化」。目的：模型切换信息如实还原或如实标注。
6. **V5 异步/流式读取**：`import_pi_jsonl` 改 `tokio::fs::read_to_string` 或 `spawn_blocking`，超大文件逐行流式读取。目的：与基线「JSONL 流式解析」对齐，不阻塞 async 线程。

### C. token 估算（V6）

7. **V6 CJK 启发式调参**：启发式路径按脚本（CJK/拉丁）分流设 ratio，或保守取 `chars_per_token ≈ 1.5`；压缩统计的 `estimate_text_tokens` 复用 `TokenEstimator` 而非硬编码 /4。目的：消除 CJK 文本 token 数 4–6 倍低估，避免非 OpenAI 模型上下文溢出。

### D. 搜索与裁剪（V7 / V10）

8. **V7 内容搜索噪声**：内容匹配改抽取 `Text` 部分后再 LIKE（或冗余纯文本列），snippet 同源；迁移整数 rowid 后再评估 FTS5。目的：消除字段名/role 误命中与原始 JSON snippet。
9. **V10 二进制裁剪**：tool result 分类对 `Image` 等非文本 part 给估算权重（按 base64 长度或固定成本），或由调用方在传入前转 Artifact。目的：二进制为主的结果不被误判 Small。

### E. 死代码（V9）

10. **V9 死变体**：移除无构造点的 `SessionStoreError::EventSessionMismatch`，或补 append 路径 session 一致性校验并配测试。目的：消除死代码或补齐其用途。

### F. 文档漂移

11. **过时注释/同名异构**：修正 `context-engine/compaction.rs`「压缩引擎尚未实现」过时注释；明确 `context-engine::CompactionReason` 与 `compaction-engine::CompactionReason`（多一个 `Manual`）的映射，避免调用方混淆。目的：文档与实现一致。

## 主要产出物

- 压缩读取改 `events_by_branch`；import 按原始 branch 分派 + 多分支往返测试；replay/tail 语义文档化
- Pi 未知字段 key 修正 + ModelSwitch 策略 + 异步读取；CJK 启发式调参 + estimate 复用 TokenEstimator
- 内容搜索抽取文本匹配；二进制裁剪权重；死变体移除；过时注释/同名异构订正

## 验收标准（保留 REVIEW 追踪编号）

- [ ] **V1**：多分支会话压缩后 `replaced_range` 与 recovery fork 的 `branch_id` 与折叠事件集合对应（多分支测试）
- [ ] **V2**：多分支 export→import 往返后事件 `branch_id` 与导出前一致（多分支往返测试）
- [ ] **V8**：`replay_events`/`tail_events` 语义文档化或提供分支感知重载
- [ ] **V3**：Pi 导入多条未知记录全部保留（断言 `report.unknown_entries` 内容）
- [ ] **V4**：ModelSwitch 策略明确（持久化事件/metadata 或报告标注未持久化）
- [ ] **V5**：`import_pi_jsonl` 不在 async 中同步读整文件（异步/流式）
- [ ] **V6**：CJK 文本启发式 token 估算不再 4–6 倍低估；压缩统计复用 `TokenEstimator`
- [ ] **V7**：内容搜索不命中字段名/role 噪声；snippet 为可读文本（用例）
- [ ] **V10**：二进制为主的 tool result 不被误判 Small（分类测试）
- [ ] **V9**：`EventSessionMismatch` 移除或补一致性校验 + 测试
- [ ] **文档**：`compaction.rs` 过时注释修正；`CompactionReason` 同名异构映射明确
- [ ] **门禁**：`cargo test`/`clippy -D warnings`/`fmt --check` 干净

**相关文档**：[REVIEW.md](../REVIEW.md) §5 · [ADR-005 仅 Pi JSONL 导入](../docs/adr/ADR-005-pi-jsonl-import-only.md) · [context](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)

> 接线提示（2026-08 review）：`compaction-engine`/`context-engine` 当前零消费者，接线（trim_tool_result 接入 ContextBuilder、CompactionEngine 接入 agent loop 超限处理）属后续阶段，本任务修复其内部缺陷；接线端到端验证随 Provider Loop 接线（P3-11）一并补。
