# pawork-gateway

> 本机 OpenAI HTTP 接入与可撤销客户端令牌。2026-09-27 从 app 中分离 HTTP 和 token 实现，仍由同一个 `pawork` 二进制启动。

## 1. 职责与边界

承载 loopback HTTP/1、OpenAI 请求和 JSON/SSE 输出、Host/Origin 校验、按客户端 token 鉴权与撤销。通过 `GatewayBackend` 调用宿主；不依赖 app、engine、providers、storage 或 control-plane。

账户选择、凭证快照、上游调用、并发租约和第三方用量结算由 [app](app.md) 实现。CLI 管监听地址和进程生命周期。没有新增独立服务二进制或远程网关能力。

## 2. 模块树

| 文件 | 职责 |
| --- | --- |
| `src/lib.rs` | 公共导出与本机时间戳 |
| `src/backend.rs` | HTTP 输入/输出类型（含 ADR-064 的 `GatewayContent` /`GatewayContentPart` 多模态正文与 `GatewayModelCapabilities`）、`web_search` 请求位、脱敏错误、`GatewayBackend` 端口（`gateway_models(purposes)` 用途过滤参数） |
| `src/server.rs` | loopback HTTP、路由（`/v1/models?purpose=` 解析：可重复、取交集、未知参数/值 400；chat 路径仍拒绝 query string）、请求限制、JSON/SSE（`ImageOutput` 事件 → `message.images` / 流式 `delta.images`，计入输出上限）、取消和连接回收 |
| `src/tokens.rs` | token 签发、摘要持久化、列表、鉴权与撤销 |

## 3. 对外 API 面

- `serve_gateway(Arc<B>, TcpListener, GatewayTokenStore, CancellationToken)`，`B: GatewayBackend`；非 127.0.0.1 listener 拒绝。
- `GatewayBackend`：`gateway_models(purposes)`（空 = v1 兼容仅 text 模型）、`prepare_gateway_completion`、`run_gateway_completion`。关联类型 `Completion: Send` 是宿主冻结的请求快照，HTTP 层不访问凭证或模型实例。
- 原生视频端口：`gateway_video_models`、`submit_gateway_video(client, GatewayVideoRequest, cancel)`、`query_gateway_video(client, id, cancel)`；三个端口均必需实现，目录 / 任务返回 domain 的 `VideoGenerationModel` / `GatewayVideoResponse`，状态为 `VideoTaskStatus`。`GatewayVideoRequest` 接受 `model`、`prompt` 与可选 `pawork_usage`。
- `GatewayChatRequest`（含 `web_search: bool`）/ `GatewayMessage`（content 字符串或 part 数组）/ `GatewayContent` / `GatewayContentPart` / `GatewayContentUrl` / `GatewayStreamOptions` / `GatewayModel`（含 `capabilities`）/ `GatewayModelCapabilities` / `GatewayError`：HTTP 子集与错误类型。
- `GatewayTokenStore::{new,issue,list,revoke,authenticate}`、`GatewayTokenInfo`、`tokens::valid_client`。
- `gateway_is_running`：基于文件锁验证存活，不能仅信 PID 文件。

### 任务日志端口（2026-10-04）

Chat / Video 请求可带 `pawork_usage: Option<TaskUsageContext>`，此元数据仅交宿主，不送上游。Chat 普通 JSON、SSE chunk 与视频响应增 `pawork_usage.call_id`；错误增可选 `error.call_id`。视频响应用 `GatewayVideoResponse` flatten 原 `VideoGenerationTask`，另附 `GatewayUsageLink`，保留既有 id/model/status/url 形状。

`GatewayBackend` 新增 `gateway_task_usage`、`report_gateway_operation` 和 completion call ID accessor，HTTP 增 `POST /v1/usage/query` 与 `POST /v1/usage/operations`，复用既有鉴权 / Host / Origin / body gate。查询只能读取 bearer token 所属 client；本地操作只接受下载 / 导入 / 导出 / 编辑的终态，不允许客户端声称 Token 或供应商费用。完整字段见 [模型网关](../model-gateway.md#任务日志与统计2026-10-04)。

## 4. 关键行为与语义

鉴权和完整请求准备成功后才返回 SSE 成功头。模型执行任务独立于响应 body，连接关闭传播取消，服务退出等待租约与账本结算收尾。此两阶段顺序与抽包前一致。

提供 `GET /v1/models`、`POST /v1/chat/completions`，以及独立的 `GET /v1/video/models`、`POST /v1/video/tasks`、`GET /v1/video/tasks/{id}`。视频提交返回 202，不自动轮询 / 重试；提示词 1–8000 字、网关任务 ID 绑定提交账号，限字母 / 数字 / 连字符 / 下划线 / 点（最多 256 字节）；客户端原样保存并查询，拒绝未知字段和查询参数。视频路由复用 token、Host / Origin、JSON 与请求体限制，宿主为上游提交 / 查询实施 60 秒上限，HTTP 后台任务与 Chat 一样登记并等待取消后的租约收尾；断连传播取消，但不宣称取消已被供应商接受的任务。普通 Chat JSON 与 SSE 继续共享 canonical 事件归一。请求体 2 MiB、输出 16 MiB、连接上限 64；令牌只落摘要，Secret 只在签发时返回一次。详细协议和限制以 [产品规格](../model-gateway.md) 为准。

## 5. 依赖与 feature

内部依赖 `domain` 与 `auth`（复用文件锁）。外部沿用现有 hyper / hyper-util / http-body-util、tokio、futures、async-trait、serde/serde_json、getrandom、blake3；无新增第三方生产依赖、无 feature。测试使用 tempfile。

## 6. 红线与不变量

不读取供应商凭证明文或数据库；不创建 Agent 会话、不执行工具。token 摘要格式、HTTP wire、状态码和安全 gate 不因抽包改变。客户端名称校验是 token 签发与宿主租户归因的共用函数。

## 7. 测试与 golden 资产

token 原回归随迁，运行 `bash scripts/test.sh gateway`。真实 `AppCore` + HTTP + 模拟上游联调保留在 `crates/app/src/gateway_tests.rs`，覆盖目录、普通/SSE、撤销、拒绝、取消、租约和用量；运行 `cargo test -p pawork-app --offline --lib gateway_`。不以单独 token 测试代替宿主集成。

原生视频回归覆盖实际路由、异步提交 / 查询、换账号后仍查询原账号、畸形状态 / 身份 / URL / 超大正文拒绝、固定生成参数、未知字段拒绝、鉴权、错误消息脱敏与不重复提交；等待上游时断连或停止 Host 均取消并释放租约，视频秒数不伪造为 token 用量。

## 8. 相关文档

[模型网关](../model-gateway.md) · [app](app.md) · [cli](cli.md) · [架构](../../architecture.md) · [验证](../verification.md)。
