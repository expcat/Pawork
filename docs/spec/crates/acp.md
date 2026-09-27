# pawork-acp

> Agent Client Protocol（ACP v1）适配与会话 actor；执行通过 `AcpCommandHost` 注入。

## 1. 职责与边界

负责 ACP wire、能力协商、canonical 映射、会话所有权、prompt/cancel/权限往返与 outbox。stdin/stdout、进程退出、`AppCore` 装配由 [cli](cli.md) 承担；本包不持有 Provider 凭证，不构造 Core，不消费 GUI frame。

## 2. 模块树

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~35 | ACP 子系统 re-export 与 `now_timestamp` |
| `src/host.rs` | ~2200 | `AcpHost` + `AcpActor`：单 actor 循环独占全部状态（occupancy / run_sessions / pending prompts / permissions / outbox / held_events）；普通信箱 + 紧急信箱；`OutboxItem`（Frame / FlushBarrier）；attach / reattach / disconnect 的 ownership 校验 |
| `src/adapter.rs` | ~660 | `AcpClientAdapter`（`ClientAdapter` 实现，纯翻译无状态）与 `AcpClientAdapterFactory`（能力协商：白名单外显式降级）；`CwdResolver` / `SessionResolver` port；registry `acp` 列准入门 `admit_acp_command` |
| `src/command_host.rs` | ~25 | `AcpCommandHost` trait（dispatch / query / subscribe）与 `AcpHostError`——ACP 触达 Core 的唯一执行面 |
| `src/map.rs` | ~285 | ACP ↔ canonical 显式映射表：错误码映射、`extract_user_message`（text / resource_link）、`translate_session_update`、权限选项（allow-once / reject-once） |
| `src/wire.rs` | ~650 | ACP v1 wire 类型：`PROTOCOL_VERSION = 1`、JSON-RPC 消息手工解析（规范精确的 -32700/-32600）、`ParamsExt::reject_unknown`（除 `_meta` 外未知字段显式拒绝）、`SessionUpdate` / `StopReason` 等 schema 类型 |
| `src/adapter.rs` tests | registry `acp` 列 = 四命令钉死；准入门放行 / 拒绝 |

## 3. 对外 API 面

- 宿主：`AcpHost`（信箱化公开 API：`handle_request` / `handle_notification` / `handle_response` / `pump_events` / `drain_and_pump` / `drain_outbox_items` / `take_outbox` / `release_drained_barriers` / `resolve_queued_prompts` / `fail_closed_all_prompts` / `has_active_runs` / `pending_run` / `degraded_capabilities` / `is_initialized` / `subscribe` / `registry` / `connection_id`）、`OutboxItem`、`PromptResolution`；
- Core 执行 port：`AcpCommandHost` trait 与 `AcpHostError`（宿主注入实现为私有 `CliAcpCommandHost`）；
- 翻译层：`AcpClientAdapter` / `AcpClientAdapterFactory` / `NegotiatedAcpAdapter` / `CwdResolver` / `SessionResolver` / `CancelTarget` / `PermissionDecision`；
- wire：`JsonRpcMessage` / `JsonRpcError` / `JsonRpcId` / `PROTOCOL_VERSION` 及 ACP v1 schema 类型。

## 4. 关键行为与语义

- `AcpHost` 内部是**单 actor**（独立 OS 线程 + current_thread runtime）独占全部可变状态：普通信箱（Mail）串行处理请求 / 事件泵 / outbox drain；`session/cancel` 与 `$/cancel_request` 走**紧急信箱**（UrgentMail，biased 优先），不被队头 prompt 或 Core dispatch 阻塞——Core 调用经 `interruptible_core_call` 挂起期间仍处理紧急件（其余邮件暂存 deferred 队列）。
- **prompt 串行**只覆盖建立临界区：`reserve_prompt_occupancy`（同 session 已有活跃 prompt → `ERROR_INVALID_REQUEST` 拒绝）→ adapter decode → `RunStart` dispatch → 绑定 run id；绑定后 turn 执行期跨会话可并发。占用窗口内到达的 cancel 记入 `early_session_cancel` / `early_request_cancel` 标志，绑定完成后立即重放。
- 事件回译：`RunChanged` 终态结算 prompt（映射见 `map.rs`），经 outbox 末尾的 `FlushBarrier` 保证此前全部帧写出后才释放；`ToolApprovalRequired` → `session/request_permission` 请求（选项固定 allow-once / reject-once），客户端响应回译为 `ToolApprove` 命令；其余可表示事件 → `session/update` 通知；`Diagnostic` 有意丢弃（不新增 update 臂）。run 归属未知的事件暂存 `held_events`，绑定后冲刷。
- 订阅滞后 → `fail_closed_all_prompts`：清空 occupancy / pending / run 映射 / held_events，全部未决 prompt 以 Failed 释放，并释放 outbox 中全部屏障；清账本前先拍下已绑定 run 与挂起权限，清空后对每个 run 补发 `RunCancel`、对每个 pending permission 补发 `ToolApprove Deny`（best-effort 补偿，避免 Core 侧悬挂）。stdin EOF 后最多 drain 30 秒（`ACP_DRAIN_TIMEOUT`）等待活跃 run 收尾，inflight 任务 join 超时 2 秒后 abort。

## 5. 依赖与 feature

内部 domain、protocol（adapter）；外部 async-trait、serde/serde_json、thiserror、tokio、tracing。dev tempfile 与 tokio 多线程运行时。生产下游仅 cli；本包无 app/cli 回依赖。

## 6. 红线与不变量

ACP `PROTOCOL_VERSION = 1`；v2、未知方法和除 `_meta` 外未知参数明确拒绝。命令只经 protocol registry 的 ACP 准入列进入 Core。Resume 必须由权威 SessionRegistry 校验所有权；取消不能被 prompt 阻塞；审批默认 fail-closed。迁包保留全部 JSON golden 字节，不增加新 wire。

## 7. 测试与 golden 资产

`bash scripts/test.sh acp`；原 CLI 的 ACP 测试及 14 个 fixture 整体迁入。

| 资产 | 覆盖点 |
| --- | --- |
| `src/adapter.rs` tests | registry `acp` 列 = 四命令钉死；准入门放行 / 拒绝 |
| `tests/fixtures.rs`（target `acp_fixtures`） | versioned golden：v1 initialize 握手响应逐字节比对、session/new / prompt / cancel fixture 解析、session/update text 与 tool_call 回译 golden、permission selected / cancelled、未知方法与 `session/set_model` 错误 golden、v2 握手拒绝、未知 params 字段拒绝、`mcpServers` 必填 vs 空数组放行、resume 缺省字段、JSON-RPC 非对象 / 坏版本拒绝 |
| `tests/floor.rs`（target `acp_floor`） | 全链路（mock host）：握手协商与降级记录、未初始化拒绝、cwd 越界拒绝与规范化别名匹配、prompt 流式回译与终态、权限请求往返、`session/cancel` / `$/cancel_request`、close → resume 重挂、跨连接 resume 走 authoritative registry claim、同 session 二 prompt 拒绝、注册窗口 early cancel 重放、fail-closed 释放 inflight、事件滞后 fail-closed、部分写出后屏障必须释放、Diagnostic 不发射 update、双客户端交错保持会话内串行且 cancel 不被阻塞 |
| `tests/common/mod.rs` | `TestHarness` / `MockScript`（脚本化 `AcpCommandHost`）与 outbox 收集工具 |
| `fixtures/v1/`（13 个 JSON） | `initialize-request/response`、`session-new-request`、`session-prompt-request`、`session-resume-minimal`、`session-cancel-notification`、`session-set-model-request`、`session-update-text`、`session-update-tool-call`、`permission-response-selected/cancelled`、`error-unknown-method`、`error-unknown-set-model` |
| `fixtures/v2/`（1 个 JSON） | `initialize-request-v2`——仅用于断言实验 v2 显式拒绝 |

## 8. 相关文档

生产进程入口仍为 `pawork acp serve`。这是外部编辑器通道，不是 GUI 协议替代品；不支持的能力明确降级或拒绝，不把未实现功能映射成成功。

[架构](../../architecture.md) · [app](app.md) · [cli](cli.md) · [protocol](protocol.md) · [验证](../verification.md)。
