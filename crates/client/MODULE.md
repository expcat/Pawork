# pawork-client

GUI Connection Protocol 的 typed 连接面，以及 headless SDK（R1 原 sdk 并入）。依赖 domain / protocol / transport。

## 职责

作为外部进程（Desktop、编程驱动）唯一允许的业务 crate：握手、Command/Query、订阅、Snapshot/Resume、Ack/Heartbeat。headless 子模块经 stdio JSONL 驱动 `pawork headless --json-stdio`。生产依赖 **不含** `pawork-app`（app 仅 dev-dep，供 probe/contract）。

## 模块树

```
src/
  lib.rs                    # GuiClient
  headless/{mod,client,error,mock,stream,transport,version}.rs
examples/probe.rs
tests/
  contract.rs  client_tests.rs  spawn_e2e.rs  probe.rs
  probe/scenarios.rs        # 9 场景
```

## 对外入口/API 面

- **GuiClient**：`connect` / `connect_with_resume*`、`command` / `query`、`subscribe*`、`next_event*`、`snapshot` / `resume` / `ack` / `heartbeat` / `close`。再导出 protocol 帧类型、`projection`、`LocalTransport`。
- **`pub mod headless`**：`PaworkClient`（`spawn` / `from_transport`、session/run/fork/cancel/compat import…）、`StdioTransport`、`MockTransport`、`EventSubscription`、`SDK_API_VERSION`。默认 spawn 参数 `["headless", "--json-stdio"]`；二进制 `PAWORK_BIN` 或 `pawork`。
- probe 九场景：`session-events`、`snapshot-reconnect`、`resume-snapshot-fallback`、`three-gui-sync`、`command-idempotency`、`artifact-chunks`、`version-reject`、`disconnect-keeps-run`、`quota-alert-roundtrip`。live 模式只在 `examples/probe.rs`。

## 依赖与被依赖

- **生产依赖**：`pawork-domain`、`pawork-protocol`、`pawork-transport`。`default = []`。
- **dev-dep**：`pawork-app`、`pawork-storage`（session）、`pawork-testkit`、transport `memory`。
- **被依赖**：`pawork-cli`、`pawork-desktop`（desktop **唯一**业务依赖）。

## 红线与注意事项

- 不嵌入 Core、不实例化 Provider、不加载 GUI framework。
- Desktop 禁止再依赖 protocol/app/engine 等；需要的类型从本包 re-export。
- 错误类型结构化，意外帧不把原始字节打进日志。
- `snapshot-reconnect` 有既有偶发超时（ROADMAP §4），改 probe 时先复跑该场景。

## 相关文档

- [docs/design.md](../../docs/design.md) §2 / §4 S10
- [docs/gui-design.md](../../docs/gui-design.md)
- [docs/headless-json-migration.md](../../docs/headless-json-migration.md)
- [代码地图总索引](../../docs/code-map/README.md)
