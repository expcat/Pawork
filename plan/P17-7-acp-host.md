# P17-7：ACP Host（Agent Client Protocol 适配宿主）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-8、P13-1、P18-10

**最终目的**：在 `core-api` 之上新增一个可替换的 Host Adapter——ACP（Agent Client Protocol）Host，使外部 ACP 客户端能通过标准 Agent 客户端协议接入 Pawork Core。ACP Host 只做协议翻译：把 ACP 的 session/task/message/tool/event 映射到 `core-api` 的 `AppCommand`/`AppQuery`/`AppEvent`，不承载业务逻辑，也不取代 GUI Connection Protocol——GUI 仍只经 GUI Connection Protocol 接入，ACP 是另一条面向生态互操作的可选接入通道（[ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md)）。

**涉及范围**：新增 `acp-host` crate；实现 `client-adapter-api::ClientAdapter`；复用 `core-api`、`app-service`、`agent-events`、`subscription-hub`（订阅事件流）；不动 `gui-protocol` / `gui-server` / `agent-engine`。

## 细分步骤

1. **ACP 协议模型与映射表** —— 目的：实现统一 ClientAdapter 契约，覆盖 initialize/capabilities、session create/resume/prompt/update、permission/tool event、cancellation；与 `core-api` 建立显式映射表，未知 method 返回 `ProtocolUnsupported`。
2. **ACP Host 适配层** —— 目的：实现一个监听 ACP 客户端的 server（stdio / 本地 socket），把入站 ACP 请求转成 `AppCommandEnvelope`（`CommandSource` 标注为互操作来源）交给 `app-service`，把 Core 事件回译成 ACP 事件流。
3. **与 GUI Connection Protocol 边界隔离** —— 目的：ACP Host 独立于 `gui-protocol`/`gui-server`，复用同一 `app-service` 与 Event Hub，但走自己的协议层；明确「GUI 不得经 ACP 接入、ACP 客户端不被当作 GUI」，避免两套接入语义耦合。
4. **能力协商与降级** —— 目的：在握手期协商 ACP 客户端支持的能力（streaming/tool result/approval），Core 不具备的能力显式降级或拒绝，协商结果写入事件来源记录，便于审计。
5. **`pawork acp serve` 接线** —— 目的：在 `cli-host` 暴露一个可选子命令启动 ACP Host（与 `serve`/`shell` 并列），不改 `cli-host` 既有装配；ACP Host 失败或关闭不影响 GUI 与 CLI 既有模式。
6. **定向 / Golden 测试** —— 目的：用 versioned fixture 覆盖「请求翻译 → Core 执行 → 事件回译」全链路、握手协商、resume/cancel、permission/tool event、unsupported method 与 GUI 边界隔离。完整矩阵由 P18-15 集中门禁。

## 主要产出物

- `acp-host` crate：ACP 类型 + 与 core-api 映射表 + Host 适配层
- `pawork acp serve` 可选子命令（不改 cli-host 既有装配）
- 定向测试（翻译全链路 / 协商降级 / 与 GUI 协议边界隔离）

## 验收标准

- [ ] ACP Host 是 `core-api` 之上的纯协议翻译，不含业务决策，不修改 `agent-engine` / `gui-protocol` / `gui-server`
- [ ] GUI Connection Protocol 仍是 GUI 的唯一接入通道，ACP Host 不取代它
- [ ] ACP 请求经映射产生合法 `AppCommandEnvelope`，Core 事件可回译为 ACP 事件流
- [ ] 能力协商结果显式记录来源，不支持的能力降级而非静默失败
- [ ] ACP adapter 复用 P18-10 Session Registry/capability snapshot，不私建 ownership 或 credential 状态
- [ ] 定向 / Mock smoke 覆盖翻译全链路与边界隔离断言

**相关文档**：[CLI Host](../docs/features/cli-host.md) · [GUI 连接与多客户端](../docs/features/gui-connection.md) · [ADR-017 GUI 不直连 Core](../docs/adr/ADR-017-gui-no-direct-access.md) · [ADR-030 Core 单一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [workspace-layout](../docs/architecture/workspace-layout.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增第三方依赖；复用 `core-api` / `app-service` / `agent-events` / `subscription-hub`。新 crate `acp-host` 依赖方向：`core-api → acp-host → cli-host`（可选接线），与 `gui-server` 平级、互不依赖。
