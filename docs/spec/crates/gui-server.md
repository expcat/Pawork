# pawork-gui-server

> GUI Connection Protocol 的服务端运行时；业务通过 `GuiHost` 注入，不依赖 AppCore。

## 1. 职责与边界

负责连接、握手、身份戳、协商能力门控、订阅、心跳、背压和重放调度。业务命令、持久事件、审批与查询实现在 [app](app.md) 的 `GuiHostAdapter`；socket/进程生命周期装配在 [cli](cli.md)。

## 2. 模块树

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~180 | `GuiHost` trait（snapshot/timeline/query/command）、`GuiHostError`、`GuiServer`/`GuiServerConfig`（bind endpoint、accept 循环、按连接 spawn 会话任务）；re-export 连接层常量 |
| `src/connection.rs` | ~550 | `ConnectionManager`：客户端注册/心跳（`DEFAULT_HEARTBEAT_TIMEOUT` 30s idle 清理）/事件订阅；每连接有界 mpsc 队列（`DEFAULT_QUEUE_CAPACITY` 1024），慢客户端标记 `lagged` 丢新事件不阻塞发布者；断连**不**取消 run |
| `src/session.rs` | ~1100 | 单连接握手与帧循环：协议版本检查、command 盖 client 戳、capability 门（未授予在宿主前拒绝）、Resume 三态调度（replay / SnapshotRequired / up-to-date）、Heartbeat→Pong、订阅确认、lagged→ReplayUnavailable 帧；R-11：`SessionHandle` 覆盖 `wait_done`（done watch 在会话 run 结束时置位），`receive` 在 close 后复用同一信号返回 ConnectionClosed，供 cli 登记集合与有序关闭等待；ADR-045 `deliverable_to_negotiated` 按协商 minor 门控推送——`TerminalExited`（since 1.3）不推给协商 <1.3 的连接（老客户端 serde 遇未知变体会 decode 失败断流），该连接仍可从快照 `terminal_sessions` 的 `state` 获知终态；`host_error_to_protocol` 把宿主 `not_found` 映射为既有 `RequestNotFound` 码（其余维持 Internal），ADR-045 的幂等边界在 wire 上可观察；RV-10 起会因上游 I/O 阻塞的查询（远程账户额度 HTTP、ModelList 目录探测）经 `query_waits_on_upstream` 判入连接级 `JoinSet` 独立任务集合，不在串行主循环饿死其后的轻量配置读（响应按 request_id 关联、乱序安全，无 request_id 的 Snapshot 仍走串行主路径），命令与其它查询保持串行 |

## 3. 对外 API 面

- trait `GuiHost`：`instance_id`、`snapshot` / `timeline` / `query` / `command` 提供业务视图与执行；`subscribe_events`、`current_sequence` / `earliest_available` / `replay` 提供同源事件与重放；`publish_event_stream_lagged` 返回经宿主排序的诊断信封。
- `GuiServer::new(GuiServerConfig)` 后 `bind(endpoint)` 返回 listener，`accept` 为每连接启动独立会话任务；配置携 `host`、`handshake`、`transport` 与可选 `connections`。
- `ConnectionManager` / `GuiSubscription` / `ManagerError`；常量 `DEFAULT_HEARTBEAT_TIMEOUT`（30s）与 `DEFAULT_QUEUE_CAPACITY`（1024）。

listener 返回的 `GuiConnection` 提供 `receive` / `close` / `wait_done`；私有 `SessionHandle` 实现该接口，CLI 用完成信号回收连接，测试可注入 MockHost。

## 4. 关键行为与语义

连接先握手，经 protocol 完成认证与版本/能力协商，再请求宿主快照。后续命令盖鉴权客户端身份戳并经过能力门；事件按连接订阅投递。慢查询在连接级任务集合执行，断开即取消；账户查询分类共用 `QuotaOverviewQuery::is_account_query`。Resume 委派宿主返回 replay、snapshot required 或已追平，随后送入同一合法事件序。

## 5. 依赖与 feature

生产内部依赖：domain、protocol、transport；外部 async-trait、thiserror、tokio、tracing。测试开启 transport local/memory，并使用 serde_json、tempfile。下游 app 实现宿主端口、cli 装配服务；client 仅 dev 依赖本包测试协议往返。

## 6. 红线与不变量

- GUI wire 由 [protocol](protocol.md) 定义，本次迁包不改变 schema 或版本。
- 连接断开、慢消费、心跳超时不取消已进入 Core 的 Run。
- 未授予能力先于宿主执行被拒绝；新事件按协商 minor 门控。
- Lagged 不伪造 seq-0 事件，返回 ReplayUnavailable；发布者不被慢客户端阻塞。
- 运行时不得依赖 app、Provider、数据库、GUI framework 或工具执行。

## 7. 测试与 golden 资产

`bash scripts/test.sh gui-server`；原 app 的连接测试整体迁入，测试语义保留。

| 资产 | Target | 覆盖点 |
| --- | --- | --- |
| `tests/gui_server/session.rs` | 具名 `[[test]]` `gui_server_session` | 握手往返、非握手首帧拒绝、command 盖戳与版本校验、SessionGet 字段透传、resume 三态与 ack、Heartbeat→Pong、断连不取消 run、lagged→ReplayUnavailable、慢消费不阻塞宿主、client_context 替换拒绝、capability 先于宿主拒绝、terminal-streaming capability 全路径、ADR-045 `TerminalExited` 按协商 minor 门控（1.2 连接跳过且不断流、1.3 连接送达）、R-11 客户端断开后 `wait_done` 有界就绪（`wait_done_fires_after_client_disconnect`） |
| `tests/gui_server/multi_gui_runtime.rs` | 具名 `[[test]]` `gui_server_multi_gui_runtime` | 三 GUI 收到相同事件序、重连 replay 缺失事件、replay 不可用回退 snapshot、慢客户端不拖累其它 GUI、断连/心跳超时均不触发 RunCancel |

## 8. 相关文档

服务只提供连接运行时，正式宿主仍是 `pawork gui serve`。真实 Desktop 的像素与 GUI 交互不由 MockHost 协议测试证明。

[架构](../../architecture.md) · [app](app.md) · [cli](cli.md) · [protocol](protocol.md) · [验证](../verification.md)。
