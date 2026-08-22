# pawork-protocol

GUI 帧、headless-json、core-api 与共享投影。依赖 `pawork-domain`。

## 职责

定义 CLI/Core 宿主与外部客户端（Desktop GUI、headless SDK、ACP）之间的连接协议：长度前缀帧编解码、握手与版本协商、命令/查询/事件信封、三通道登记表、Timeline 投影 reducer。typegen 把 TS 形状检入仓库根 `schemas/{core-api,gui-protocol,headless-json}/`。不实现传输（见 `pawork-transport`），不访问 Provider / 数据库。

## 模块树

```
src/
  lib.rs
  codec.rs  error.rs  handshake.rs  resume.rs  snapshot.rs
  client_auth.rs          # feature client-auth
  typegen.rs              # feature typegen
  app/
    command.rs  query.rs  event.rs  quota.rs  limits.rs  version.rs
    registry.rs           # 不经 app::* glob 再导出
  adapter/                # feature adapter
  headless/               # 默认 feature；stdio 另需 headless
  projection/
  bin/typegen.rs          # pawork-protocol-typegen
```

## 对外入口/API 面

`pub mod`：`app`、`codec`、`error`、`handshake`、`headless`、`projection`、`resume`、`snapshot`；可选 `adapter` / `client_auth` / `typegen`。crate 根再导出 `app::*` 与编解码、握手、resume 辅助。

要点（形状以 golden / `schemas/` 为准）：

- **帧**：`ClientFrame` / `ServerFrame`（serde `tag = "type", content = "data"`）；`MAX_PROTOCOL_FRAME_BYTES = 1 MiB`。
- **版本**：`API_VERSION = 1.2`；`SUPPORTED_API_VERSIONS` 含 1.0 / 1.1 / 1.2。
- **app**：`AppCommand`（19）/ `AppQuery`（11）/ `AppEvent` / `AppResponse` 及对应信封。
- **registry**（`pawork_protocol::app::registry`，**不** glob 到根）：`command_entry` / `query_entry`；未登记 wire 名 fail-closed。三通道（GUI / headless / ACP）可用性由登记表派生。
- **projection**：`project_event`、`TimelineProjection`——纯内存，无 serde，不在线上。
- **headless**：`HeadlessRequest` / `HeadlessResponse`、stdio `run_loop`；与 GUI 帧正交。
- **handshake**：`HandshakeRequest` / `HandshakeResponse`；`GuiCapability`（`ArtifactStreaming` 枚举保留、默认不宣告）。

## 依赖与被依赖

- **依赖**：`pawork-domain`。可选 `ts-rs`（`typegen`）、`async-trait`（`adapter`/`headless`）、`getrandom`（`client-auth`）。
- **features**：`default = ["adapter", "client-auth", "headless"]`；另有 `typegen`。
- **被依赖**：`pawork-app`、`pawork-client`、`pawork-cli`（`features = ["adapter"]`）；`pawork-storage` 仅 dev-dep（adapter）。
- **不依赖本包**：`pawork-transport`、`apps/desktop`（经 client）。

## 红线与注意事项

- 冻结契约：帧 / headless JSON / typegen schemas；改形状须 golden 先行。R6/R7 之外不要静默升版本。
- 宣告 = 授权 = 实现：新命令只改 registry，禁止在 GUI/headless/ACP 再写一份名字表。
- `WorkspaceRelativePath` 拒绝绝对路径与 `..`。
- `ClientAuthentication.proof` 与 token **不入日志**；handshake JSON 不得含 crate semver。
- `DegradeEvent` 上线为 `AppEvent::Diagnostic`（26 帧 golden）。
- `GuiCapability::ArtifactStreaming` 保留枚举、不宣告（K-08）。

## 相关文档

- [docs/design.md](../../docs/design.md) §3.2 GUI 协议行
- [docs/headless-json-migration.md](../../docs/headless-json-migration.md)
- [docs/gui-design.md](../../docs/gui-design.md)
- [plan/R3-protocol-unification.md](../../plan/R3-protocol-unification.md)
- [代码地图总索引](../../docs/code-map/README.md)
