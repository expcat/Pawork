# B23 · 删除 `AppEvent::TeamEvent`

> 状态：实施完成（自动验证）；单独提交，未推送。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 背景与授权

[B22](b22-package-review-cleanup.md) 审查发现 GUI 协议里的 `AppEvent::TeamEvent`（18 变体）和配套 Team 镜像类型在仓内没有构造点。`teams` crate 随 R0 归档后，唯一引用是 `pawork-acp` 把它映射成种类字符串 `team_event`。该变体在冻结 schema 里，删除属于 wire 变更，所以 B22 没有动，列入待裁决。用户 2026-10-13 裁决删除，并要求单独立项、golden 先行。

## 改动

- `crates/protocol/src/app/event.rs`：删除 `AppEvent::TeamEvent`、`TeamEvent` 及 `impl`（`team_id` / `kind`）、`TeamMemberRole` / `TeamTaskState` / `TeamBoardTask` / `TeamPresence` / `TeamRecipients` / `TeamPlanStepStatus` / `TeamPlanStepSnapshot` / `TeamPlanCommentAnchor`。`AppEvent` 从 23 变体变为 22 变体，文件从约 660 行降到约 385 行。
- `crates/acp/src/map.rs`：删除 `team_event` 种类分支。
- `schemas/{core-api,gui-protocol,headless-json}/`：用 `pawork-protocol-typegen` 重新生成。每组删除 9 个 Team 声明，以及只经 Team 类型引用、因而不再生成的 `AgentId` / `CheckpointId` / `PlanId` / `PlanVersionId`；`AppEvent.d.ts` 去掉该分支和导入，`index.d.ts` 去掉对应 13 个导出。每组从 126 / 142 / 107 个 `.d.ts` 降到 113 / 129 / 94 个，diff 中没有其他声明变化。
- 文档：[architecture](../architecture.md) §3.2 记录本次删除；[protocol Spec](../spec/crates/protocol.md) 改变体数、删 §3.1 表行和 Team 注记、更新 schema 计数；[orchestration Spec](../spec/crates/orchestration.md) 与 [backlog](../spec/backlog.md) BK-WORK-01 改为「已删除，复活时重新定型 wire」。

## 兼容性

- 历史上没有任何 Host 构造或发出过 `team_event` 帧，旧客户端不会因此少收事件，所以不提升 minor，`SUPPORTED_API_VERSIONS` 不变。
- 帧 golden（`crates/protocol/tests/golden/`）里没有 Team 样本，无需改动。domain `AgentEvent`（持久化与重放事实源）、SQLite schema、headless golden 都不变。
- 恢复点：本提交的父提交保留完整形状。teams 复活时按 BK-WORK-01 重新设计事件，先改 golden，不直接还原这份镜像。

## 验证

| 命令 | 结果 |
| --- | --- |
| `cargo run -p pawork-protocol --features typegen --bin pawork-protocol-typegen` | 重新生成三组 schema；diff 只有上述 Team 删除 |
| `bash scripts/test.sh protocol acp`（含 typegen 校验与帧 golden） | 235 passed，无新警告 |
| `cargo check --offline --tests -p pawork-app -p pawork-cli -p pawork-client -p pawork-gui-server -p pawork`（feature 由脚本补齐） | 通过，警告与改前相同 |
| `cargo check -p pawork-desktop --offline --bins --tests --features gpui/runtime_shaders` | 通过，警告与改前相同 |

未跑全 workspace、真实 Provider 或真窗口：删除的类型在仓内无构造点，下游只需编译通过。

附带发现：B22 的 T3 改了 `RunStart.profile` 的文档注释，但没有重新生成 schema，导致 typegen 校验从 T3 起失败（B22 的逐提交检查只做编译）。B22 提交尚未推送，已把重新生成的三份 `AppCommand.d.ts` 并入 T3，并在新 HEAD 上单跑 typegen 测试通过。
