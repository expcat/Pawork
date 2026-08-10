# P13-4：GUI Server 与 Local Transport

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P13-3

**最终目的**：在 CLI 进程内实现 GUI 协议服务器（`gui-server`），接受 GUI 连接、完成握手与认证、提供 Query/Command、广播 Event、提供 Snapshot 与流式终端/Artifact 传输；并通过 `transport-local` 提供 Unix Domain Socket（macOS/Linux）与 Named Pipe（Windows）端点。GUI 经此连接 Core（[ADR-022](../docs/adr/ADR-022-gui-connects-via-cli.md)/[027](../docs/adr/ADR-027-local-remote-same-protocol.md)）。

**涉及范围**：`gui-server`、`transport-local`、`transport-memory`、`client-auth`

## 细分步骤

1. **GUI Server 生命周期** —— 接受连接、握手、认证、收发帧、关闭。目的：CLI 内部协议服务器。
2. **Local Transport** —— Unix Socket / Named Pipe 绑定与监听。目的：本地 GUI 接入。
3. **进程内 Transport（测试）** —— `transport-memory`。目的：无需真实 socket 的测试。
4. **客户端认证** —— 目的：身份与权限可控。
5. **Endpoint 发现** —— `pawork gui endpoint`。目的：GUI 可发现连接点。

## 主要产出物

- `gui-server` + `transport-local` + `transport-memory` + `client-auth`

## 验收标准

- [x] 本地 GUI 可通过 Unix Socket / Named Pipe 连接 CLI
- [x] 连接需握手与认证
- [x] 进程内 Transport 可用于测试

## 实现记录（2026-08-10）

- `transport-local`：Unix Domain Socket（macOS/Linux）与 Named Pipe（Windows）端点；
  u32 LE 长度前缀分帧（与 `gui-protocol` 分帧约定一致，注释交叉引用），读取前校验
  声明长度上限（默认 1 MiB），超限帧拒绝后再分配；Unix 侧清理陈旧 socket 文件，
  Windows 侧首实例 `first_pipe_instance(true)`、客户端对 ERROR_FILE_NOT_FOUND /
  ERROR_PIPE_BUSY 重试至超时。
- `transport-memory`：内存 channel 对实现同套 traits，`ConnectionLocality::InProcess`，
  供无真实 socket 的测试使用。
- `client-auth`：`TokenStore`（生成/加载/删除，拒绝覆盖，Unix 0600/0700 权限）+ 32 字节
  token（64 hex），constant-time 比较，实现 `gui-protocol` 的 `ClientAuthenticator`
  （scheme `pawork-token`，失败返回 AuthenticationFailed）。
- `gui-server`：`GuiServer::new(GuiServerConfig)` / `bind(endpoint)` 返回监听器，每次
  `accept` 派发连接任务（握手 → 帧循环）：Command→`dispatch_envelope`、Query→
  `dispatch_query`、ArtifactRead→按 64 KiB 分片回 `ArtifactChunk`（payload 由 P13-8
  接入，当前仅元数据）、Heartbeat→Pong；Subscribe/Resume/SnapshotRequest/Ack 返回
  明确 "not wired until P13-5" 错误；帧编解码仅用 gui-protocol encode/decode。
- 定向验证：`cargo test -p transport-local -p transport-memory -p client-auth -p gui-server`
  全绿（Windows：Named Pipe 5 项；memory 6 项；client-auth 8 项；gui-server 9 项含
  握手拒绝、command/query 往返、心跳、ArtifactChunk 分片、Named Pipe 端到端）；
  `cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings` 通过。

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-022](../docs/adr/ADR-022-gui-connects-via-cli.md) · [ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ROADMAP](../ROADMAP.md)
