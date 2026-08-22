# pawork-transport

GUI 传输层：只搬运有界字节帧。无内部 `pawork-*` 依赖。

## 职责

为本机 GUI 连接提供 `GuiTransportServer` / `GuiTransportClient`：Unix Domain Socket 或 Windows Named Pipe（feature `local`，默认开），以及进程内通道（feature `memory`，默认关）。**不**编解码 GUI Connection Protocol——那是 `pawork-protocol` 的事。帧长度上限与 protocol 对齐（1 MiB），但不依赖该 crate。

远程 TLS 生产实现已在 R0 归档；`Remote*` trait / DTO 仍留在 `api.rs`，避免生产路径依赖 mock。

## 模块树

```
src/
  lib.rs
  api.rs                 # 始终编译：帧、endpoint、trait
  local.rs               # feature local；按平台 #[path] 引入：
  local_unix.rs          #   UnixListener / UnixStream
  local_windows.rs       #   Named Pipe
  memory/mod.rs          # feature memory
```

无 `tests/` 目录；回归在各文件 `#[cfg(test)]`。

## 对外入口/API 面

- 常量：`DEFAULT_MAX_FRAME_BYTES = 1024 * 1024`
- 数据：`TransportFrame`（只持有字节）、`TransportEndpoint`（`Local` / `Remote` / `Memory`）、`ConnectOptions`、`ConnectionInfo`、`ConnectionLocality`、`TransportError`
- trait：`GuiTransportServer`、`GuiListener`、`GuiConnection`、`GuiTransportClient`
- 远程契约（无生产 impl）：`RemoteGuiTransportProvider`、`RemoteGuiConnector` 及描述/发布 DTO
- feature `local`：`LocalTransport`（server + client；只接受 `Local` endpoint）
- feature `memory`：`MemoryTransport`、`MemoryListener`

本机分帧：`[u32 LE payload_len][payload]`；分配前按 `max_frame_bytes` 校验。本 crate **不解析** JSON。

## 依赖与被依赖

- **依赖**：无 `pawork-*`。`tokio` / `async-trait` / `serde` / `thiserror`。
- **features**：`default = ["local"]`；`local`；`memory`。无 `remote` feature。
- **被依赖**：`pawork-app`、`pawork-cli`、`pawork-client`（三者生产默认 `local`；app/client 的 dev-dep 开 `memory`）。
- **不依赖本包**：`pawork-protocol`（避免环）；`apps/desktop` 经 `pawork-client` 间接使用。

## 红线与注意事项

- Adapter 不得依赖 Agent 领域类型；只搬运字节。
- GUI 不得经 transport 直连 Provider / 数据库 / 工具；只连 CLI 暴露的 GUI Connection Protocol。
- `Remote` endpoint 与 trait 存在 ≠ 远程传输已交付；复活须按当时协议版本重评（ROADMAP §3.3）。
- 改帧长时必须与 `pawork-protocol::MAX_PROTOCOL_FRAME_BYTES` 同步，且保持两 crate 无依赖边。

## 相关文档

- [docs/design.md](../../docs/design.md) §2
- [docs/gui-design.md](../../docs/gui-design.md)
- [ROADMAP.md](../../ROADMAP.md) §3.3（远程 GUI 复活条件）
- [代码地图总索引](../../docs/code-map/README.md)
