# ADR-043:会话工作区归属持久化(schema v13)

- **状态**:Accepted(用户 2026-08-31 确认)
- **日期**:2026-08-31

## 背景

P1(项目与会话生命周期)第一片要求 Session→Workspace 归属跨 Host 重启持久化(ROADMAP §1 下一动作)。定位实态:

- 归属仅存于进程内:`SessionService.workspaces: Mutex<HashMap<String, WorkspaceId>>`(`crates/app/src/services/session.rs`),代码注释自述「不改 S1 sessions DDL;重启后历史 session 进 Unassigned」。Host 重启后 Host 快照与会话列表(`gui_host/mod.rs`、`gui_host/handlers/query.rs`)丢失归属,Task 落入 Unassigned;`SessionOpen` / `SessionFork` 的兼容响应还会在缺绑定时回退到当前附着 workspace,造成归属展示失真。
- 既有持久化设施均不合用:`session_tags` 走 import/export 且语义是用户标签;`session_bindings` 是 control-plane 租户绑定;`runs.run_json` 按 Run 记录,空会话无 Run;Desktop 无本地持久态,workspace 登记本身靠重连后重发 `workspace_add` 恢复。
- 约束:会话存储 schema(`CURRENT_SCHEMA_VERSION = 12`)是冻结契约(architecture.md §3.2),演进须 ADR Accepted + 版本化迁移。

## 决策

### D1 — `sessions` 增 `workspace_id TEXT` 可空列(v13,纯追加)

- 迁移仅 `ALTER TABLE sessions ADD COLUMN workspace_id TEXT`;不回填,历史 NULL 继续诚实落入 Unassigned。
- 不加 FK:workspace 登记是 Host 进程内/按实例恢复的状态,跨 Host 不保证存在,FK 会制造悬空约束;归属列为弱引用,NULL 按 Unassigned,尚未登记的 canonical id 原样保留(与既有 SessionCreate→snapshot 契约一致)。
- 写路径:`create_session_with_workspace` 在同一 SQLite 事务内创建 session、main 分支与 workspace 归属,成功后才更新进程内缓存;`set_session_workspace` 为既有 session 提供 fail-closed 写穿。AppCore 启动时读取全部非 NULL 绑定(含 archived),并原子替换进程内缓存;读路径(`session_workspace*`)签名与语义不变。
- 不进 import/export:归属是本地宿主状态而非会话内容,沿用 v11 `command_ledger`「纯新增不进 export」先例。

### D2 — 否决支

- 借用 `session_tags`:污染用户标签语义并泄入 export。
- data_dir 侧车 JSON:引入第二持久化轨道,会话删除留孤儿绑定,生命周期一致性靠手工维护。
- AgentEvent / wire 新增变体:信封 32 变体为冻结契约,且宿主归属不是 Agent 事件语义。

## 后果

- `CURRENT_SCHEMA_VERSION` 12 → 13;升级 golden 链扩展至 v13。
- `SessionRecord` 增 `workspace_id: Option<String>`;GUI 会话列表/快照的归属列改由持久层背书。
- 重复 `open_store` 不保留旧库缓存;初始绑定写入失败时 session 与 main 分支一并回滚,不产生孤儿会话。
- 恢复边界不变:本片只持久化绑定;Terminal 进程与 Run 实时状态仍不恢复,Unassigned 对无绑定会话保持诚实显示。
