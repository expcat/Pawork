# pawork-transport

> GUI 传输层：只搬运有界字节帧（`[u32 LE payload_len][payload]`，上限 1 MiB）的业务无关抽象与本机 / 进程内实现。无任何 `pawork-*` 依赖，位于依赖图最底层之一；被 `pawork-client`、`pawork-app`、`pawork-cli` 消费。

## 1. 职责与边界

- 为本机 GUI 连接提供 `GuiTransportServer` / `GuiTransportClient` 抽象及两套实现：Unix Domain Socket / Windows Named Pipe（feature `local`，默认开）与进程内通道（feature `memory`，默认关，测试用）。
- 只搬运字节：**不**编解码 GUI Connection Protocol（编解码在 [protocol.md](protocol.md)），不解析 JSON，不引用任何 Agent 领域类型。
- 帧长度上限与 `pawork-protocol::MAX_PROTOCOL_FRAME_BYTES`（1 MiB）对齐，但**不依赖**该 crate（两侧各自定义常量，人工保持同步，避免依赖环）。
- 远程传输（TLS / 中继）的生产实现已在 R0 归档；`RemoteGuiTransportProvider` / `RemoteGuiConnector` trait 与 DTO 仍保留在 `api.rs` 作为唯一契约来源，无生产 impl、无 `remote` feature。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~25 | crate 文档、feature 门控（`local` / `memory`）、re-export：`api::*`、`LocalTransport`、`MemoryTransport` / `MemoryListener` |
| `src/api.rs` | ~215 | 始终编译的抽象层：`DEFAULT_MAX_FRAME_BYTES`、`TransportFrame`、`TransportEndpoint`、`ConnectOptions`、`ConnectionInfo`/`ConnectionLocality`、四个核心 trait、`TransportError`/`TransportErrorKind`、远程契约 trait 与 DTO |
| `src/local.rs` | ~300 | feature `local`：`LocalTransport`（同一类型兼任 Server 与 Client）、帧编解码核心 `StreamConnection<R, W>`（读写各持一把 `tokio::sync::Mutex`）、错误构造辅助；按平台 `#[path]` 引入下两文件 |
| `src/local_unix.rs` | ~320 | Unix Domain Socket 的 `bind`/`connect`：陈旧 socket 文件清理、`0o600` 权限收紧、`UnixSocketListener`（close 时删除 socket 文件） |
| `src/local_windows.rs` | ~330 | Windows Named Pipe 的 `bind`/`connect`：owner-only DACL 管道创建、逐连接重建 pipe instance |
| `src/memory/mod.rs` | ~430 | feature `memory`：`MemoryTransport`（channel 名注册表）、`MemoryListener`、`MemoryConnection`（`tokio::sync::mpsc` 无界通道对，locality = `InProcess`，帧上限仍校验） |

无 `tests/` 目录；回归全部在各文件 `#[cfg(test)]`。

## 3. 对外 API 面

**帧与端点（`api.rs`，始终可用）**

- `DEFAULT_MAX_FRAME_BYTES: u64 = 1024 * 1024`：默认单帧上限。
- `TransportFrame`：只持有 `Vec<u8>`（`new` / `as_bytes` / `into_bytes`），无任何协议语义。
- `TransportEndpoint`（serde `tag = "kind"`, snake_case）：`Local { address }`（socket 路径 / pipe 名）、`Remote { address, adapter }`（契约保留）、`Memory { channel }`。
- `ConnectOptions { timeout_ms, client_label: Option<String>, max_frame_bytes }`：客户端侧单帧上限来自此处；服务端上限来自 `LocalTransport::new(max)`。
- `ConnectionInfo { connection_id, locality, peer_label, encrypted, max_frame_bytes }`；`ConnectionLocality`：`Local` / `Remote` / `InProcess`。

**核心 trait（全部 `async_trait`、`Send + Sync`）**

- `GuiTransportServer::bind(endpoint) -> Box<dyn GuiListener>`；`GuiListener::{accept, close}`。
- `GuiTransportClient::connect(endpoint, options) -> Box<dyn GuiConnection>`。
- `GuiConnection::{send(frame), receive() -> TransportFrame, close, info}`：`&self` 并发安全；`close` 幂等。

**错误**

- `TransportError { kind, message, retryable }`（可序列化）；`TransportErrorKind`：`InvalidEndpoint` / `BindFailed` / `ConnectionFailed` / `ConnectionClosed` / `Timeout` / `FrameTooLarge` / `ProtocolViolation` / `AuthenticationFailed` / `Unsupported` / `Internal`。

**实现（feature 门控）**

- feature `local`（默认）：`LocalTransport`——`new(max_frame_bytes)` / `Default`（1 MiB）；只接受 `TransportEndpoint::Local`，其余端点返回 `InvalidEndpoint`。
- feature `memory`：`MemoryTransport`（`Clone` 共享同一 channel 注册表）、`MemoryListener`；只接受 `Memory` 端点；重复 bind 同名 channel 返回 `BindFailed`，向未 bind 的 channel connect 返回 `ConnectionFailed`。
- 远程契约（无生产 impl）：`RemoteGuiTransportProvider::{describe, publish, unpublish, revoke}`、`RemoteGuiConnector::connect` 及 `RemoteTransportDescription` / `RemotePublishRequest` / `RemotePublishHandle`。

## 4. 核心行为与数据流

**本机 framed 连接（`local.rs` + 平台模块）**

1. 服务端 `bind(Local { address })`：Unix 下若路径已存在且是 socket 文件则视为陈旧遗留并删除，存在但非 socket 则 `BindFailed`；bind 成功后将 socket 文件权限收紧为 `0o600`。Windows 下创建 owner-only DACL 的 named pipe。
2. 客户端 `connect`：按 `timeout_ms` 限时建立流（超时 → `Timeout`，失败 → `ConnectionFailed`）；连接两端各自持有 `ConnectionInfo`（服务端 id 形如 `connection-N`，客户端形如 `client-N`）。
3. `send(frame)`：先在写入前按 `info.max_frame_bytes` 校验长度（超限 → `FrameTooLarge`，不写任何字节），再写 `[u32 LE payload_len][payload]`。对端已断（BrokenPipe / ConnectionReset 等）→ 标记关闭并返回 `ConnectionClosed`。
4. `receive()`：先读 4 字节长度前缀——帧边界上的干净 EOF 视为对端正常关闭（`ConnectionClosed`），前缀读到一半断流为 `ProtocolViolation`；声明长度超过上限时**在分配缓冲区之前**拒绝（`FrameTooLarge`）并标记连接关闭（流已错位不可恢复）；然后 `read_exact` 读满 payload。
5. `close()`：幂等；首次关闭 shutdown 写半部。Unix listener `close` 额外删除 socket 文件。

**进程内通道（`memory/mod.rs`）**

6. `MemoryTransport` 内部是 `channel 名 → listener 入站队列` 的注册表；`connect` 构造两条 `mpsc` 无界通道组成连接对，把服务端一半推给 listener，`accept` 弹出即完成"握手"。帧上限仍按 `max_frame_bytes` 校验，保证测试与线上行为一致。

## 5. 契约与不变量

- **分帧格式冻结**：`[u32 LE payload_len][payload]`，与 `pawork-protocol::codec`（`FRAME_LENGTH_PREFIX_BYTES = 4`）一致；上限 1 MiB 与 `MAX_PROTOCOL_FRAME_BYTES` 对齐。改帧长必须两 crate 同步修改且保持无依赖边（有 `default_max_frame_matches_protocol_limit` 定向回归钉住）。
- **有界性**：send / receive 双向校验；声明长度先于内存分配校验（拒绝恶意超大头导致的 OOM）。
- **无协议语义**：本 crate 任何类型不解析 payload 字节；`TransportEndpoint` 的 serde 形态（`kind` tag）可独立于 protocol crate 往返。
- **本机安全默认**：UDS socket 文件 `0o600`；Windows pipe owner-only DACL。
- `TransportFrame` 只持有字节，无 Debug 泄漏 payload 的路径以外的额外持有。

## 6. 依赖关系

- **上游**：无 `pawork-*` 依赖；外部仅 `tokio`（net / io-util / sync / time / rt）、`async-trait`、`serde`、`thiserror`。
- **features**：`default = ["local"]`；`local`；`memory`。**无** `remote` feature。
- **下游**：`pawork-client`（[client.md](client.md)）、`pawork-app`（[app.md](app.md)）、`pawork-cli`；三者生产默认 `local`，app / client 的 dev-dep 额外开 `memory`。`apps/desktop` 经 `pawork-client` 的 re-export 间接使用，不直接依赖本包。
- `pawork-protocol` 与本包互不依赖（避免环）。

## 7. 测试与验证资产

全部为源文件内联 `#[cfg(test)]`：

- `api.rs`（2）：`TransportEndpoint` serde 往返不需要 protocol 类型；`TransportFrame` 只持有字节。
- `local.rs`（2）：默认帧上限 = 1 MiB（与 protocol 对齐的钉子测试）；非 `Local` 端点被拒。
- `local_unix.rs`（5）：bind 后 socket 权限 `0o600`；双向帧往返；超限 send 在写前被拒；伪造超限长度头在分配前被拒；对端关闭 → `ConnectionClosed`、关闭后的 listener 拒绝 accept。
- `local_windows.rs`（3）：Windows 侧对应回归（round trip / 权限 / 关闭）。
- `memory/mod.rs`（6）：bind/connect/accept 配对、重复 bind 拒绝、未 bind connect 拒绝、帧上限校验、关闭语义。

默认验证命令：`cargo test -p pawork-transport --offline --lib --tests`。

## 8. 注意事项与已知限制

- `Remote` 端点与 trait 存在 ≠ 远程传输已交付；复活须按当时协议版本重评（见 [../../../ROADMAP.md](../../../ROADMAP.md) §5）。
- GUI 不得经 transport 直连 Provider / 数据库 / 工具，只连 CLI 暴露的 GUI Connection Protocol（架构红线，见 [../../design.md](../../design.md) §2）。
- `memory` 通道为 mpsc 无界队列：不模拟背压与半关闭细节，仅供进程内装配测试。
- 声明长度超限后连接被单方面标记关闭，调用方需重连而非重试同一连接。
- 更多跨包流程见 [../flows.md](../flows.md)（GUI Connection Protocol 一节）与 [../../architecture.md](../../architecture.md)。
