# pawork-gateway

> 本机 OpenAI HTTP 接入与可撤销客户端令牌。2026-09-27 从 app 中分离 HTTP 和 token 实现，仍由同一个 `pawork` 二进制启动。

## 1. 职责与边界

承载 loopback HTTP/1、OpenAI 请求和 JSON/SSE 输出、Host/Origin 校验、按客户端 token 鉴权与撤销。通过 `GatewayBackend` 调用宿主；不依赖 app、engine、providers、storage 或 control-plane。

账户选择、凭证快照、上游调用、并发租约和第三方用量结算由 [app](app.md) 实现。CLI 管监听地址和进程生命周期。没有新增独立服务二进制或远程网关能力。

## 2. 模块树

| 文件 | 职责 |
| --- | --- |
| `src/lib.rs` | 公共导出与本机时间戳 |
| `src/backend.rs` | HTTP 输入/输出类型、脱敏错误、`GatewayBackend` 端口 |
| `src/server.rs` | loopback HTTP、路由、请求限制、JSON/SSE、取消和连接回收 |
| `src/tokens.rs` | token 签发、摘要持久化、列表、鉴权与撤销 |

## 3. 对外 API 面

- `serve_gateway(Arc<B>, TcpListener, GatewayTokenStore, CancellationToken)`，`B: GatewayBackend`；非 127.0.0.1 listener 拒绝。
- `GatewayBackend`：`gateway_models`、`prepare_gateway_completion`、`run_gateway_completion`。关联类型 `Completion: Send` 是宿主冻结的请求快照，HTTP 层不访问凭证或模型实例。
- `GatewayChatRequest` / `GatewayMessage` / `GatewayStreamOptions` / `GatewayModel` / `GatewayError`：既有 HTTP 子集与错误类型。
- `GatewayTokenStore::{new,issue,list,revoke,authenticate}`、`GatewayTokenInfo`、`tokens::valid_client`。
- `gateway_is_running`：基于文件锁验证存活，不能仅信 PID 文件。

## 4. 关键行为与语义

鉴权和完整请求准备成功后才返回 SSE 成功头。模型执行任务独立于响应 body，连接关闭传播取消，服务退出等待租约与账本结算收尾。此两阶段顺序与抽包前一致。

只提供 `GET /v1/models` 和 `POST /v1/chat/completions`；普通 JSON 与 SSE 共享 canonical 事件归一。请求体 2 MiB、输出 16 MiB、连接上限 64；令牌只落摘要，Secret 只在签发时返回一次。详细协议和限制以 [产品规格](../model-gateway.md) 为准。

## 5. 依赖与 feature

内部依赖 `domain` 与 `auth`（复用文件锁）。外部沿用现有 hyper / hyper-util / http-body-util、tokio、futures、async-trait、serde/serde_json、getrandom、blake3；无新增第三方生产依赖、无 feature。测试使用 tempfile。

## 6. 红线与不变量

不读取供应商凭证明文或数据库；不创建 Agent 会话、不执行工具。token 摘要格式、HTTP wire、状态码和安全 gate 不因抽包改变。客户端名称校验是 token 签发与宿主租户归因的共用函数。

## 7. 测试与 golden 资产

token 原回归随迁，运行 `bash scripts/test.sh gateway`。真实 `AppCore` + HTTP + 模拟上游联调保留在 `crates/app/src/gateway_tests.rs`，覆盖目录、普通/SSE、撤销、拒绝、取消、租约和用量；运行 `cargo test -p pawork-app --offline --lib gateway_`。不以单独 token 测试代替宿主集成。

## 8. 相关文档

[模型网关](../model-gateway.md) · [app](app.md) · [cli](cli.md) · [架构](../../architecture.md) · [验证](../verification.md)。
