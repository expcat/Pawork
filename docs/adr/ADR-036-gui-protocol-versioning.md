# ADR-036：GUI Connection Protocol 版本化与兼容策略

- **状态**：Accepted
- **日期**：2026-08-10

## 背景

GUI 是独立进程、独立发布（Phase 19 的 GPUI Desktop 与 CLI/Core 不同步发版），
两者经 GUI Connection Protocol 通信（[ADR-022](ADR-022-gui-connects-via-cli.md)、
[ADR-027](ADR-027-local-remote-same-protocol.md)）。协议类型在 P0-8 冻结后，
线上 schema 仍需要可演进：旧客户端不能因新字段、新 minor 而无法连接，新增
能力也不能被旧服务端误读。semver 面向库消费者，不能直接约束跨进程线上契约，
因此需要显式的版本模型、协商与兼容规则。

## 决策

版本模型为 `ApiVersion { major: u16, minor: u16 }`（`core-api` 定义）。major
不兼容，minor 向后兼容。当前版本为 `API_VERSION`，宿主支持的全部版本收录在
`SUPPORTED_API_VERSIONS` 常量表（P13-10 落地）。

- **协商**：客户端在 `HandshakeRequest.supported_api_versions` 提交候选；服务端
  在 `negotiate_api_version_with` 中取 major 交集的最高共同 minor，无交集则
  `HandshakeResponse::Rejected` + `IncompatibleVersion`。协商结果写入
  `ApiHandle.api_version` 与 `HandshakeResponse::Accepted.selected_api_version`。
- **信封校验**：入站 Command/Query 与出站 Response/Event 信封必须满足
  `envelope.major == negotiated.major && envelope.minor <= negotiated.minor`；
  不满足即产生 `IncompatibleVersion`（`decode_*_frame_checked` 路径）。服务端只
  发送协商 minor 内定义的帧与事件，客户端只发送协商 minor 内的信封。
- **minor 只增**：同 major 内只能发布更高 minor；已发布的 minor 必须继续支持
  （保留在 `SUPPORTED_API_VERSIONS` 表内）。演进入口为
  `ApiVersion::bump_minor`。
- **字段级演进**：新增字段必须 `#[serde(default, skip_serializing_if = ...)]`；
  旧客户端按 serde 默认忽略未知字段、旧服务端省略缺省字段，双向兼容。帧与
  信封的 `tag` / `content` / `rename_all` 是冻结格式，不得修改。
- **枚举变体**：新增或删除变体会破坏旧客户端解码（serde 对未知变体报错），
  因此只在 major bump 时进行；同 major 内变体只可废弃（文档登记 + 服务端停止
  生产），不可删除。新增能力优先走可筛选的枚举字段（如
  `GuiCapability`/`capabilities`）而非新增变体。
- **废弃流程**：字段或变体废弃时，先在
  [gui-connection](../features/gui-connection.md) 与本 ADR 附录登记废弃时间与
  计划删除的 major，保留至少一个完整 major 周期；期间旧客户端照常工作。
- **删除策略**：仅在 major bump 时可删除字段/变体，且必须已走完废弃流程。
  minor 内不允许删除。
- **格式锁定**：`gui-protocol/tests/fixtures/*.json` golden fixture 锁定线上
  JSON 形状；`cargo run -p schema-typegen -- --check` 拒绝 Rust 类型与生成
  schema 的漂移；生成的 `schemas/core-api/versions.d.ts` 向非 Rust 客户端提供
  与 `SUPPORTED_API_VERSIONS` 一致的版本基线。

## 后果

- 同 major 内可安全演进：加可选字段、加 minor、表内追加版本都不破坏旧客户端；
  服务端按协商结果裁剪行为。
- 服务端对不兼容客户端返回结构化 `IncompatibleVersion`，可诊断、可重试策略
  明确（`retryable: false`）。
- 未来 major 升级必须客户端与服务端同步升级（或经网关），并按废弃流程记录
  迁移；本 ADR 冻结的 serde 格式随 major bump 重新评审。

## 附录：废弃登记

（当前无废弃项。）

## 相关

- [GUI Connection Protocol](../architecture/api-surface.md) · [GUI 连接与多客户端](../features/gui-connection.md)
- [ADR-022 GUI 经 CLI 连接](ADR-022-gui-connects-via-cli.md) · [ADR-027 本地远程同协议](ADR-027-local-remote-same-protocol.md) · [ADR-030 Core 单一事实源](ADR-030-core-sole-source-of-truth.md)
- [P13-3 GUI Connection Protocol](../../plan/P13-3-gui-protocol.md) · [P13-10 GUI Protocol schema 版本化](../../plan/P13-10-protocol-schema-version.md)
