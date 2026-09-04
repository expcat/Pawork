# Pawork 路线图

> 2026-09-04 活动线切换为内部收敛。本文是当前任务与后续顺序的唯一计划事实源；旧阶段的逐片记录查阅 [docs/history.md](docs/history.md)，逐字内容查阅 git 历史。架构红线与冻结契约仍以 [docs/architecture.md](docs/architecture.md) 和源码/golden 为准。

## 1. 当前指针

| 字段 | 当前事实 |
| --- | --- |
| 活动线 | **CLN — 内部收敛** |
| 状态 | 🟢 CLN-0～10 已验证；SET-7 暂停 |
| 当前交付 | [CLN 任务书](plan/cleanup.md)、[ADR-052](docs/adr/ADR-052-exec-policy-path.md)（Accepted：exec→policy 复用路径 helper） |
| 下一动作 | Settings SET-7 仍暂停；无下一 CLN 片，下一活动线由用户指定 |
| 当前阻塞 | 无。Settings SET-7 真窗口矩阵与四项人工签字不在本波范围 |
| 发布 | **不在本计划内**。待功能继续完善后，由用户另行指定发布范围、License 与门禁。 |

状态：⚪ 未开始 · 🔵 进行中 · 🟢 已验证 · ⚠️ 阻塞。`已实现`、`自动门禁通过`、`真实环境通过`、`人工验收`、`已发布`必须分别记录。

## 2. 目标、范围与完成口径

### 2.1 目标

在不改变 21 包布局、不改磁盘/线上冻结形状的前提下，彻底去掉补丁式重复实现：路径内核单源、配置 RMW 单源、OAuth HTTP 单源、Settings 载荷类型单源，并拆开 Desktop/app/engine 神文件。

### 2.2 非目标

- 不合并 ADR-039 不合并清单中的包，不新增 crate 或 JS Runtime。
- 不升 GUI API minor，不改 schema v14 / 信封 v1 / PWB1 / config 字段形状。
- 不做 SET-7 真窗口收口；Settings 产品缺口仍登记在 [plan/settings.md](plan/settings.md)。
- 不删已有用户数据的读兼容别名（`on_failure`、legacy hint key、keychain 字段名）。
- 不发布。

### 2.3 完成口径

见 [plan/cleanup.md](plan/cleanup.md) §1 验收标准。用户可见 Settings/工作台行为保持；内部模块与 Rust API 允许破坏式重命名，不留双轨。

## 3. 已锁定的规则

- **ADR-052**：exec 可依赖 policy，删除 `exec/src/path.rs`；CancellationToken 仍双轨。
- **无补丁**：禁止新增 shim 模块、`#[deprecated]` 转发、旧函数名空壳。
- **冻结契约**：golden 先于任何可能改变 serde/SQL 的改动；本波默认不改那些形状。
- **Cargo**：全会话同一时刻一个 Cargo 进程；子代理不编译。

## 4. 执行顺序

| 阶段 | 状态 | 交付与退出条件 |
| --- | --- | --- |
| CLN-0 文档立项 | 🟢 | ROADMAP/任务书/ADR-052/architecture 指针落地 |
| CLN-1 workspace | 🟢 | resources 走 policy；config writer 单一 RMW；workspace 121+13+15 绿 |
| CLN-2 HTTP F06 | 🟢 | auth `http_client()`；auth 73 绿（1 ignored）+ tools 133 绿 |
| CLN-3 exec 路径 | 🟢 | 删除 path.rs；`cargo test -p pawork-exec` 64 绿 |
| CLN-4 protocol 载荷 | 🟢 | Settings DTO + ApprovalModeWire；typegen 绿；`SetApprovalMode.mode` 仍 String |
| CLN-5 desktop 拆分 | 🟢 | 神文件拆分 + typed 解析；desktop 186 |
| CLN-6 app 拆分 | 🟢 | settings handlers 子模块；Host 用 protocol 类型序列化 |
| CLN-7 engine 拆分 | 🟢 | tool_loop 按 round/exec/compaction/approval 拆 |
| CLN-8 storage | 🟢 | `atomic_write_bytes` 单源；157+1 ignored / pwb1 4 / read_range 5 |
| CLN-9 cp + git | 🟢 | `TenantPolicyDecision` + 单一 `FileStatus`；cp 204、git 57+5 |
| CLN-10 收口 | 🟢 | Spec/history 回写；`cargo tree -p pawork` 闭包 243 行未膨胀 |
| Settings SET-7 | ⚠️ | 暂停；见 [plan/settings.md](plan/settings.md) |

每阶段写入集与停止条件见 [plan/cleanup.md](plan/cleanup.md)。阶段失败先收敛当前层。

## 5. 开放决策与硬前置

| ID | 决策 | 当前状态 |
| --- | --- | --- |
| CLN-D1 | exec 是否允许依赖 policy | ADR-052 Accepted |
| SET-D1～D6 | Settings 既有拍板 | 维持；本波不重开 |

冻结 wire/config/schema、Secret 生命周期或架构边界变化必须先走 ADR；本波仅 ADR-052 已授权。

## 6. 验证与状态回写

- 纯文档任务：相对链接、状态词汇、`git diff --check`；不运行 Cargo。
- 实现任务：`cargo test -p <crate> --offline --lib --tests`；desktop 用 `--bins --features gpui/runtime_shaders`。
- 协议改动：typegen + golden 不可推迟。
- 完成一片后同步本文件、任务书与涉及包的包级 Spec；过程压缩追加到 [docs/history.md](docs/history.md)。

```text
Implemented: <生产路径/用户入口，或 none>
Validated: <实际命令/检查，或 none + 原因>
Targeted regressions: <覆盖，或 none>
Real-world evidence: <环境/账号/窗口，或 pending>
Known gaps: <剩余缺口与登记位置>
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 7. 任务约定

1. 开始前复核源码、当前 diff、相关包 Spec；不得按旧计划或记忆改代码。
2. 写清目标、非目标、验收标准和不改动范围；每片控制在数小时内可独立验证。
3. 保留用户未提交改动；只触碰当前切片必需文件。
4. 改 wire/schema/config/安全语义时先 ADR；本波除 ADR-052 外不扩边界。
5. 现有测试能证明行为时不新增测试体系。
6. 不执行全量 workspace 门禁、提交、推送或发布，除非用户另行明确要求。
