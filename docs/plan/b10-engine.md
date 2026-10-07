# B10 · Engine 预留入口与上下文 API 裁决

> 状态：✅ 完成。更新：2026-10-07。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决未消费入口、工具结果裁剪模块和跨 Storage 小 API。

## 当前线索与读取范围

登记项为 `run_session_turn`、`tool_result_trim` 模块、`ContextBudget::reserved_tokens` 等小 API 和跨 storage 的 `TokenEstimator::estimator_kind`。原 ROADMAP 的零消费者判断须实施前重核。

[engine Spec](../spec/crates/engine.md)、[storage Spec](../spec/crates/storage.md)。

## 实施范围

- 核对现行 Agent loop，而非仅测试装配；裁决入口和模块的必要性。
- 与 [CU-12 截图上下文预算](cu-12-screenshot-context.md) 协调，只在请求侧裁剪，不改持久化证据。
- RV-2026-10-A 测试骨架转发收敛仍按 backlog 触发，不单独扩大本任务。

## 完成条件

- 裁决不删除正在使用或当前计划明确需要的生产行为。
- 事件顺序、工具结果和持久化/重放的受影响回归通过，engine / storage Spec 同步。

## 完成记录

2026-10-07 完成裁决。基线 a4978e71，逐项重核生产消费者（rg 全仓 `.rs`/`.toml`/scripts/docs，非仅测试装配）后裁决如下。

### 逐项裁决

| 登记项 | 裁决 | 事实依据 |
| --- | --- | --- |
| `run_session_turn` | **删除** | engine 外零消费者：app 生产入口用 `run_session` / `run_manual_compaction`（`crates/app/src/services/run.rs`），cli / desktop / gui-server / scripts 无任何引用；无任何计划文档登记需要；符合 architecture §3.3「无消费者不合入」。 |
| `tool_result_trim` 模块 | **保留**，登记为 CU-12 依赖 | 零生产消费者，但 [CU-12](cu-12-screenshot-context.md)（ROADMAP 登记活动任务）当前依据明确点名「`tool_result_trim` 未接生产循环」、实施范围要求先与 B10 裁决归属——属 architecture §3.3 的「已登记的激活条件」。不提前实现 CU-12；engine Spec §8 已标注该依赖。 |
| `ContextBudget::reserved_tokens` | **删除** | 零消费者（仅自身测试一行断言）；无计划登记；纯 accessor 方法不进 `ContextBudget` 的 serde 冻结形状，删除不影响契约。 |
| engine `TokenEstimator::estimator_kind` | **删除** | 零消费者（仅 trait 定义与 `HeuristicEstimator` 实现）；唯一实现者为 `HeuristicEstimator` 本体，无外部 trait 实现者。storage 窄口 `TokenEstimator`（`count_text` / `count_message`）本无此方法，其生产链路（app `SessionTokenEstimatorBridge` → `CompactionEngine`）不受影响、不在裁决范围。trait 收窄为 `count_text` 唯一必需方法。 |

### 实际改动

- `crates/engine/src/session_turn.rs`：删 `run_session_turn` 函数本体与 5 个专属内联测试（单轮事件序 golden、预取消 / 流中取消 / provider 错误 / persist 失败续跑）；保留 `SessionTurn` / `now_timestamp`（`run_session` 与 app 生产使用）和 `optional_usage` / `last_stream_usage`（tool_loop 生产使用）。
- `crates/engine/src/lib.rs`：re-export 收窄为 `{now_timestamp, SessionTurn}`。
- `crates/engine/src/context/budget.rs`：删 `reserved_tokens` 方法与对应测试断言。
- `crates/engine/src/context/token.rs`：删 trait `estimator_kind` 必需方法与 `HeuristicEstimator` 实现，同步文档注释。
- `docs/spec/crates/engine.md`：同批回写（§1 职责、§2 模块地图、§3.4 改「手动压缩」、§3.7、§7 测试资产、§8 注意事项与 CU-12 依赖登记）。
- `docs/spec/crates/storage.md`：核对后无需改动（窄口 `TokenEstimator` 描述准确，从未提及 `estimator_kind`）。

### 验证

- `bash scripts/test.sh engine`：60 个 lib 测试 + `domain_only` / `no_provider_branch` 红线各 2 个全部通过（exit 0）；覆盖事件顺序、工具结果（`tool_result_artifacts_reach_completed_event_and_tool_message`）、审批事件对、取消、压缩链、checkpoint 等受影响面。
- `bash scripts/test.sh storage`（含 compaction / checkpoint / protected feature）：163 个 lib 测试 + `pwb1_golden` 4 个 + `read_range` 5 个全部通过（exit 0），交叉确认「跨 storage」裁决项零影响。
- `cargo check -p pawork-app -p pawork-cli --offline`：编译通过，确认删除 pub API 对下游（仅 app / cli，desktop 本在 deny-list）零影响。
- 删除前三项各自 rg 全仓复核无外部消费者；Spec 与代码交叉核对无残留引用。

Targeted regressions: engine 定向回归（lib 60 + 红线 4）、storage 定向回归（lib 163 + golden 9）、app/cli 编译检查。
Full workspace gate: NOT RUN（当前未设置全量门禁）。

### 遗留边界

- RV-2026-10-A（engine 测试骨架 LoopContext 转发收敛）触发条件为「下次改 engine 测试骨架时顺势收敛」；本任务删除的是 `session_turn` 专属测试，未触碰 `tool_loop/tests.rs` 骨架，按 backlog 不单独扩大。
- `tool_result_trim` 的生产接线归 CU-12，本任务不提前实现；CU-12 实施时注意其文档提示：Chat / Responses 当前丢弃 Artifact 类型图像来源，不能只替换 ImageSource。
