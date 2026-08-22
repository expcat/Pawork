# GUI 连接

Desktop（及 probe）如何连上 Core，而不加载 Core crate。

## 进程与字节

```
pawork-desktop  ──framed bytes──►  pawork gui serve
     │                                  │
 pawork-client                    pawork-cli → pawork-app
     │                                  │
 protocol 编解码                    GuiServer + GuiHostAdapter
 transport Local (UDS / pipe)      transport Local
```

- 传输：`pawork-transport` 只搬 `[u32 LE len][payload]`，上限 1 MiB。
- 编解码：`pawork-protocol` 的 `ClientFrame` / `ServerFrame`。
- 鉴权：`gui serve` 写 `gui.token`（`TokenStore`）；desktop `platform.rs` 读同名文件，缺 token fail-closed。
- 命令/查询：三通道可用性来自 `protocol::app::registry`，host 分发表与 `gui.available` 双射。未登记 fail-closed。

## Desktop 四层

`ui` → `controller`（只调 `GuiClient`）→ `projection`（无 gpui/tokio）→ `platform`（socket/token）。生产 `pawork-*` 依赖必须恰好 `{pawork-client}`。

## 断线

`ConnectionManager` 心跳清理连接，**不**取消进行中的 Run。Resume：`Replay` / `SnapshotRequired` / `UpToDate`（`ResumeDisposition`）。Timeline 投影 reducer 在 `protocol::projection`，host 与 desktop 同源。

## Headless / ACP

- Headless：`pawork headless --json-stdio`，stdout 仅 JSONL；SDK 在 `pawork-client::headless`。
- ACP：`pawork acp serve`；`AcpHost` 不消费 GUI 帧、不持有凭证、不构造第二个 Core。

模块图：[desktop](../../../apps/desktop/MODULE.md) · [client](../../../crates/client/MODULE.md) · [transport](../../../crates/transport/MODULE.md) · [protocol](../../../crates/protocol/MODULE.md) · [app](../../../crates/app/MODULE.md) · [cli](../../../crates/cli/MODULE.md)
