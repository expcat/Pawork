# CLN 内部收敛任务书

> 状态：CLN-0～10 已验证。Settings SET-7 暂停，缺口见 [settings.md](settings.md)。过程记录见 [history.md](../docs/history.md)。

## 1. 开工合同

### 目标

消除 SET-1～6 与历史阶段堆出的补丁层、神文件、重复内核与 F06 HTTP 缺口：包内重写模块边界，路径/配置/HTTP/Settings 载荷各只留一套实现。

### 非目标

- 不合并 ADR-039 不合并清单中的包，不新增 crate；
- 不改 GUI API minor、SQLite schema、事件信封、PWB1、config schema 字段形状；
- 不完成 SET-7，不改用户可见 Settings 文案/导航/gate 语义（内部结构可全拆）；
- 不删磁盘读兼容别名（`on_failure`、legacy hint key、keychain 字段）；
- 不发布、不提交、不推送，除非用户另行要求。

### 验收标准

1. `resources/io.rs` 与 `exec` 不再自写 canonicalize / within-root；只调用 `pawork_policy`。
2. Global 配置四入口共用一个 RMW 内核。
3. OAuth/MCP 测试与可注入的默认 HTTP 客户端均为 `redirect(Policy::none())`。
4. Settings 查询/命令 Data 有 protocol 类型；Host 与 Desktop 不再各维护一套手写 JSON 字段名。
5. Desktop/app/engine 神文件按域拆分，无空壳 shim。
6. 每片定向测试绿；`cargo tree` 无环；`-p pawork` 闭包不膨胀。

## 2. 切片状态

| 片 | 状态 | 验证 |
| --- | --- | --- |
| CLN-0 文档立项 | 🟢 | ADR-052 Accepted；architecture 指针 |
| CLN-1 workspace 路径 + RMW | 🟢 | `pawork-workspace` 121+13+15 |
| CLN-2 HTTP F06 | 🟢 | auth 73（1 ignored）+ tools 133 |
| CLN-3 exec 路径 | 🟢 | exec 64；删除 `path.rs` |
| CLN-4 protocol Settings DTO | 🟢 | typegen 绿；`SetApprovalMode.mode` 仍为 String |
| CLN-5 desktop 拆分 | 🟢 | desktop 186；神文件拆分 + typed 解析 |
| CLN-6 app handlers / 门面 | 🟢 | app 192+6+15+2 |
| CLN-7 engine tool_loop | 🟢 | engine 66+2+2 |
| CLN-8 storage 原子写 | 🟢 | storage 157+1 ignored；pwb1 4；read_range 5 |
| CLN-9 control-plane + git | 🟢 | control-plane 204；git 57+5 |
| CLN-10 收口 | 🟢 | `cargo tree -p pawork` 闭包 243，与 Wave 3 前基线相同 |

### CLN-8 — storage 原子写

**写入集**：`crates/storage/src/blob/`；可选拆 `event_store.rs`（零 SQL/serde 变化）；同批 storage Spec。

**验证**：`cargo test -p pawork-storage --offline --lib --tests --features compaction,checkpoint,protected`（157 + 1 ignored；pwb1 4；read_range 5）。`event_store.rs` 未拆（测试绑私有 redact helper）。

### CLN-9 — control-plane 命名 + git FileStatus

**写入集**：`crates/control-plane/` 撞名类型、`crates/git/src/{status,diff}` 的 `FileStatus` 合一；`merge_dual_failures` 保留（G2 复活面）。同批两包 Spec。

**验证**：`cargo test -p pawork-control-plane -p pawork-git --offline --lib --tests`（204 + 57+5）。

### CLN-10 — 收口

**写入集**：architecture / ROADMAP / history、触及的包级 Spec、`cargo tree` 记录。

**验证**：`cargo tree -p pawork --offline --prefix none` 去重后 243 行，与 Wave 3 前基线一致；`pawork-*` 包集合未增；无环（workspace 包仅钻石依赖 `(*)`）。

## 3. 指挥约定

- 主代理只写文档闸门、派发、串行 Cargo、审查回写。
- 实现用 grok-4.6 子代理；写入集互不重叠；子代理不跑 Cargo、不提交。
- 失败先收敛当前片，不扩大写入集。
