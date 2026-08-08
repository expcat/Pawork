# P17-9：IDE Host Adapter（IDE 生命周期、诊断与交互桥接）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-8、P17-8（协调 P17-4）

**最终目的**：在 `core-api`（经 P17-8 可嵌入 SDK）之上提供一个 IDE Host Adapter，把 IDE（VS Code / JetBrains 等）的编辑器生命周期、诊断与交互桥接到 Pawork Core——让 IDE 扩展能驱动 Agent、回灌 LSP 诊断、并接收 Core 状态变化。它是 `core-api` 之上的 Host Adapter，不取代 GUI Connection Protocol：独立 GUI 仍经 GUI Connection Protocol 接入，IDE 适配层是面向编辑器集成的可选通道（[ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md)）。

**涉及范围**：新增 `ide-host-adapter` crate；复用 `agent-sdk`（P17-8 嵌入 API）、`core-api`、`agent-events`，协调 `lsp-runtime`（P17-4 诊断来源）。不动 `gui-protocol` / `gui-server`。

## 细分步骤

1. **IDE 适配 trait 与生命周期映射** —— 目的：定义 IDE Host Adapter 的抽象 trait（编辑器打开/关闭/激活文件、活动选区、可见范围、保存事件），把这些生命周期事件映射为 Core 可消费的上下文/事件（经 `agent-sdk`），统一 IDE 侧差异。
2. **诊断回灌** —— 目的：把 LSP 诊断（来自 `lsp-runtime`，P17-4）与 Agent 产出（apply_patch/edit 后的 lint）桥接进 IDE，并在 IDE 显示的诊断变化时反向通知 Core，形成「编辑→诊断→Agent」闭环；诊断不绕过 Policy。
3. **交互桥接（apply/edit/diff/审批）** —— 目的：把 Core 的文件变更、Diff、Approval 请求映射为 IDE 原生交互（编辑器内展示 diff、跳转、内联审批），操作最终都落回 `core-api` 的 `AppCommand`，IDE 不持有可写权威状态。
4. **与 GUI Connection Protocol 边界隔离** —— 目的：明确 IDE Host Adapter 不走 GUI 协议帧；它经 `agent-sdk` 嵌入 Core，与 `gui-server` 共享同一 `app-service` 但互不依赖；IDE 与独立 GUI 可并存。
5. **扩展宿主契约（最小）** —— 目的：定义一个最小的「IDE 扩展 ↔ Adapter」契约（消息子集），使任意 IDE 扩展实现该契约即可接入，Adapter 不绑定具体 IDE SDK。
6. **定向 / Mock 测试** —— 目的：用 Mock IDE 扩展覆盖生命周期映射、诊断双向回灌、apply/approval 回路、边界隔离断言（IDE 通道不触达 GUI 协议帧）。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `ide-host-adapter` crate：IDE 适配 trait + 生命周期/诊断/交互桥接 + 最小扩展契约
- 定向测试（生命周期 / 诊断回灌 / 审批回路 / 边界隔离）

## 验收标准

- [ ] IDE Host Adapter 经 `agent-sdk` 嵌入 Core，是 `core-api` 之上的 Host Adapter，不取代 GUI Connection Protocol
- [ ] LSP 诊断（P17-4）可双向回灌，编辑→诊断→Agent 闭环不绕过 Policy
- [ ] 文件变更/Diff/Approval 经 IDE 原生交互展示，操作落回 `AppCommand`，IDE 不持可写权威状态
- [ ] IDE 与独立 GUI 可并存，二者通道互不触达
- [ ] 定向 / Mock smoke 覆盖生命周期、诊断回灌、审批回路与边界隔离

**相关文档**：[CLI Host](../docs/features/cli-host.md) · [GUI 连接与多客户端](../docs/features/gui-connection.md) · [ADR-017 GUI 不直连 Core](../docs/adr/ADR-017-gui-no-direct-access.md) · [ADR-030 Core 单一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [workspace-layout](../docs/architecture/workspace-layout.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增第三方依赖；复用 `agent-sdk` / `core-api` / `agent-events`，协调 `lsp-runtime`（P17-4，诊断来源）。新 crate `ide-host-adapter` 依赖方向：`core-api → agent-sdk → ide-host-adapter`，与 `gui-server` 平级。
