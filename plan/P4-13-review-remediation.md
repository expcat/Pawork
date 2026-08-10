# P4-13：Phase 4 评审修复（REVIEW remediation）

> Phase 4 · 核心工具与权限 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P4-1 ~ P4-12、P3-11 V8（scheduler 上下文注入先于本任务的策略接线）

**最终目的**：消除 [REVIEW.md](../REVIEW.md) §4（Phase 4）评审发现的安全边界未接线、上下文断链、数据完整性缺陷与基线卫生问题——把 `PolicyEngine::decide()` 与 `allowed_in_untrusted_workspace` 接入执行路径，消除调度器 `"default"` 假值，补全 apply_patch 部分失败回滚，让 checkpoint 可崩溃恢复，并清理 crate 内死依赖与缺位的匹配器 fuzz 测试。

**涉及范围**：`policy-engine`、`builtin-tools`、`checkpoint-service`、`process-runtime`、`tool-runtime`（scheduler）、根 `Cargo.toml`、ROADMAP「依赖选型基线」、各 `plan/P4-*.md`

## 细分步骤（分组）

### A. 安全边界通电（V1 / V4）

1. **V1 PolicyEngine 接线**：把 `PolicyEngine::decide()` 接入 `tool-runtime` 调度器，用 `PolicyDecision` 替换 `require_approval_for_writes` 布尔；强制 `allowed_in_untrusted_workspace`（未信任工作区 + false → Deny）。目的：P4-9/P4-10 的信任闸门在运行时生效。
2. **V4 危险命令硬拒绝地板**：增加无视 ApprovalMode 的 denylist 地板（`rm -rf /`、`mkfs`、`dd of=/dev/` 恒 Deny/AskUser），让 Shell 分类成为底线。目的：最宽松模式对最破坏性命令也有地板。

### B. 上下文注入（V2）

3. **V2 checkpoint 上下文断链**：调度器从真实 `ToolExecutionContext`（workspace_id/run_id/working_directory）注入（依赖 P3-11 V8 已完成），消除所有 run 的改动挂在同一 `"default"` key、回滚键全局碰撞。目的：Phase 5 Compaction / Phase 12 Worker 写隔离依赖 run 级 checkpoint 正确归属。

### C. 数据完整性（V3）

4. **V3 apply_patch 回滚补全**：错误路径对已 snapshot 的路径改用 checkpoint 内容恢复（而非 `remove_file`），或追加一次 `rollback_tool_call`；`create` 覆盖既有文件不删除原内容；补 create-over-existing / update / delete 三类回归测试。目的：消除半应用状态与原内容丢失。

### D. 健壮性/可用性（V5 / V6 / V7 / V8）

5. **V5 Windows env allowlist**：按平台分桶（Windows 额外含 SYSTEMROOT/TEMP/TMP/USERPROFILE/COMSPEC/PATHEXT），允许配置层追加透传变量。目的：env_clear 后复杂工具链行为正常。
6. **V6 edit_file 尾换行**：模糊匹配重建时按原文是否以 `\n` 结尾补回；补回归测试。目的：消除 POSIX 文本文件结尾换行被吞。
7. **V7 list_directory dangling symlink**：改用 `symlink_metadata`（不跟随）判类型/大小，跟随失败降级为「broken symlink」而非整目录失败。目的：单个失效链接不阻断目录浏览。
8. **V8 run_command 真流式**：改用 `process-runtime` 的 `spawn_stream` 边读边 emit。目的：长构建/测试用户可见增量输出。

### E. 持久化与性能（V9 / V11）

9. **V9 checkpoint 元数据持久化**：Run→change→blob/path 映射以版本化状态文件原子写入 Artifact Store 根目录，`CheckpointService::open` 在崩溃恢复时重建；避免 `checkpoint-service` 反向依赖 `session-store`，后续组合层可再投影为事件。目的：ADR-010「所有改动可撤销」在崩溃路径成立。
10. **V11 阻塞 IO**：read_file/search_text/checkpoint 改 `tokio::fs` + 流式/分块或 `spawn_blocking`，read_file 读取受预算约束。目的：避免 worker 线程阻塞与整文件入内存。

### F. 其余健壮性（V10 / V12 / V13 / V14）

11. **V10 周期 cancel**：search_text/find_files 遍历内每 N entry 检查一次 cancel。目的：大仓库长扫描可中途取消。
12. **V12 list_directory 分页**：大目录单次扫描取页，记录总数。目的：分页名副其实。
13. **V13 edit_file 滚动匹配**：模糊匹配改滚动哈希或规范化后单次扫描。目的：消除 O(L·n) 行拼接。
14. **V14 spawn_stream 句柄/上限**：句柄持有发送端或 child id 支持外部 kill；流式路径补 `max_output_bytes` 与截断标记。目的：消除死句柄与流式/缓冲语义不一致。

### G. 基线/包清理与 fuzz

15. **死依赖清理**：移除 `content-inspector` 基线声明；删 `policy-engine`/`checkpoint-service` 的 `agent-domain`、`process-runtime` 的 `bytes`/`futures` 死依赖；`builtin-tools` 三处 `atomic_write` 已下沉到 `common`，`checkpoint-service` 与 `artifact-store` 因依赖方向和持久语义不同保留各自实现。目的：基线与 crate 依赖卫生。
16. **维护期死依赖检查**：把 `cargo machete`/`cargo udeps` 放入依赖升级、发布候选或定期维护的 L3 工作流，不加入每次开发提交的阻塞链。目的：在功能簇稳定后防止死依赖再生，同时避免前期频繁依赖调整拖慢实现。（2026-08-10 更新：不在本项目配置自动执行 Actions，`dependency-hygiene.yml` 已移除；检查项保留为文档记录，随 L3 维护人工执行）
17. **匹配器属性测试**：为 edit_file/apply_patch 匹配器补 proptest 策略属性测试（随机 old_string/new_string/文件内容组合，断言不 panic、计数一致、回滚后逐字节相等），满足基线「需完整 fuzz 与审计」标准；`arbitrary` 若需使用，必须按基线流程重新引入。目的：安全关键路径覆盖属性测试。

### H. 文档同步

18. Phase 4 的 12 篇 `plan/P4-*.md` 已勾选（无 drift），本任务确保新验收项随修复勾选。目的：文档与实现一致。

## 主要产出物

- PolicyEngine 接线 + 危险命令地板 + 调度器真实上下文；apply_patch 回滚补全 + checkpoint 版本化持久状态
- Windows env 分桶、edit_file 尾换行、list_directory symlink、run_command 真流式；阻塞 IO 改造
- 死依赖清理 + machete/udeps 维护检查（2026-08-10 起为文档记录项，工作流已移除）+ 匹配器属性测试

## 验收标准（保留 REVIEW 追踪编号）

- [x] **V1**：`PolicyEngine::decide()` 被调度器调用；未信任工作区 + `allowed_in_untrusted_workspace=false` → Deny（测试）
- [x] **V4**：trusted + NeverAsk 下 `rm -rf /` 等被硬拒绝/询问（用例）
- [x] **V2**：写工具的 checkpoint 挂在真实 run_id 下，跨 run 不碰撞（测试）
- [x] **V3**：apply_patch create-over-existing 不删原文件；update/delete 部分失败可恢复（三类回归测试）
- [x] **V5**：Windows env 透传含 SYSTEMROOT/TEMP/TMP/USERPROFILE/COMSPEC/PATHEXT（用例）
- [x] **V6**：模糊匹配末行编辑后文件保留结尾 `\n`（回归测试）
- [x] **V7**：含 dangling symlink 的目录可列出（broken symlink 降级，测试）
- [x] **V8**：run_command 长命令增量输出可见（流式测试）
- [x] **V9**：进程崩溃后 checkpoint 映射可由持久状态重建、可回滚（崩溃恢复测试）
- [x] **V11**：read_file/search_text/checkpoint 不在 async 中同步阻塞读整文件（审查/基准）
- [x] **V10**：search/find 遍历中可中途取消（测试）
- [x] **V12 / V13 / V14**：list_directory 分页省成本；edit_file 模糊匹配不退化 O(L·n)；spawn_stream 句柄可 kill 且流式有输出上限
- [x] **基线**：`content-inspector` 移出；4 个 crate 死依赖删除；L3 维护工作流含 machete/udeps（不阻塞普通开发提交；2026-08-10 起工作流移除，转为文档记录项，不在本项目配置自动执行 Actions）
- [x] **fuzz**：edit_file/apply_patch 有 proptest 属性测试（不 panic、计数一致、回滚逐字节相等）
- [x] **快速验证**：安全红线、Policy、checkpoint、路径与 patch 立即跑定向回归；workspace 全量 build/test/clippy 延后到 Core 主干 L2

## 验证记录（2026-08-09）

- `cargo test -p policy-engine -p tool-runtime -p builtin-tools -p checkpoint-service -p process-runtime`
- `cargo clippy -p policy-engine -p tool-runtime -p builtin-tools -p checkpoint-service -p process-runtime --all-targets -- -D warnings`
- 属性测试覆盖模糊匹配总函数/计数一致与 apply_patch 失败后逐字节恢复；checkpoint 重开与跨 Run 同 ToolCall ID 隔离均有回归测试。

**相关文档**：[REVIEW.md](../REVIEW.md) §4 · [ADR-009 默认工作区信任](../docs/adr/ADR-009-default-workspace-trust.md) · [ADR-010 全写 Checkpoint](../docs/adr/ADR-010-checkpoint-all-writes.md) · [ROADMAP 依赖选型基线](../ROADMAP.md#依赖选型基线)

> 跨任务协调（2026-08 review）：V2 上下文注入依赖 P3-11 V8 先落地；本任务负责调度器策略接线（V1）与 checkpoint 归属验证。`tool-runtime/scheduler.rs` 由两任务序列修改，不并行。
