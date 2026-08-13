# P17-9：IDE Host Adapter（IDE 生命周期、诊断与交互桥接）

> Phase 17 · Ecosystem & Host Compatibility · 状态：✅已实现（AdapterBuilt） · 交付成熟度：AdapterBuilt（历史代码交付≠产品验收） · 依赖：P0-8、P17-8（协调 P17-4、P17-11）

**最终目的**：提供 IDE Host Adapter，把 IDE（VS Code / JetBrains 等）的编辑器生命周期、诊断与交互桥接到 Pawork。接入链路统一为：

```text
IDE Extension → IDE Host Adapter → Agent SDK / Headless Protocol → pawork Host → Core
```

即 IDE 经 Agent SDK / Headless 协议（P17-8）连接唯一正式宿主 `pawork`（Core 单一事实源），**不「通过 SDK 嵌入第二个 Core」**。它是 Host Adapter / Client Channel，与 GUI Connection Protocol、ACP（P17-7）、Mobile（P17-12）并列，互不替代；独立 GUI 仍经 GUI Connection Protocol 接入，IDE 适配层是面向编辑器集成的可选通道（[ADR-021](../docs/adr/ADR-021-cli-core-same-process.md)、[ADR-025](../docs/adr/ADR-025-cli-is-sole-host.md)、[ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md)）。

> 可选输出（item 1 收敛）：「Pawork 对 IDE 暴露 LSP Server」作为本 Adapter 的**可选 server-side 输出能力**——当 IDE 仅需消费 Pawork 聚合的代码智能时，Adapter 可向 IDE 暴露一个 LSP Server 面；它复用 [P17-4 LSP Client Runtime](P17-4-lsp-runtime.md) 已聚合的语言服务结果，是 P17-9 的可选职责，不改变 P17-4 作为 LSP Client 的主要定位。

**涉及范围**：新增 `ide-host-adapter` crate；复用 `agent-sdk` / Headless 协议（P17-8 client）、`core-api`、`agent-events`，协调 `lsp-runtime`（P17-4，诊断来源与可选 LSP Server 输出复用）。不动 `gui-protocol` / `gui-server`；`pawork` 仍是唯一正式宿主。

## 细分步骤

1. **IDE 适配 trait 与生命周期映射** —— 目的：定义 IDE Host Adapter 的抽象 trait（编辑器打开/关闭/激活文件、活动选区、可见范围、保存事件），把这些生命周期事件经 Agent SDK / Headless 协议映射为 Core 可消费的上下文/事件，统一 IDE 侧差异；IDE 不直接构造 Core。
2. **诊断回灌** —— 目的：把语言服务诊断（来自 `lsp-runtime`，P17-4 作为 LSP Client 聚合 rust-analyzer 等）与 Agent 产出（apply_patch/edit 后的 lint）桥接进 IDE，并在 IDE 显示的诊断变化时反向通知 Core，形成「编辑→诊断→Agent」闭环；诊断不绕过 Policy。
3. **交互桥接（apply/edit/diff/审批）** —— 目的：把 Core 的文件变更、Diff、Approval 请求映射为 IDE 原生交互（编辑器内展示 diff、跳转、内联审批），操作最终都落回 `core-api` 的 `AppCommand`，IDE 不持有可写权威状态。
4. **与 GUI Connection Protocol 边界隔离** —— 目的：明确 IDE Host Adapter 不走 GUI 协议帧；它经 Agent SDK / Headless 协议连接 `pawork` Host，与 `gui-server` 共享同一 `app-service` 但互不依赖；IDE 与独立 GUI 可并存。
5. **可选 LSP Server 输出** —— 目的：当 IDE 仅消费 Pawork 代码智能时，Adapter 可向 IDE 暴露一个最小 LSP Server 面，复用 P17-4 聚合结果（diagnostics / hover / definition / references 等）；该输出为可选，不影响「P17-4 是 LSP Client」的主定位。
6. **扩展宿主契约（最小）** —— 目的：定义一个最小的「IDE 扩展 ↔ Adapter」契约（消息子集），使任意 IDE 扩展实现该契约即可接入，Adapter 不绑定具体 IDE SDK。
7. **定向 / Mock 测试** —— 目的：用 Mock IDE 扩展覆盖生命周期映射、诊断双向回灌、apply/approval 回路、可选 LSP Server 输出、边界隔离断言（IDE 通道不触达 GUI 协议帧、不构造第二 Core）。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `ide-host-adapter` crate：IDE 适配 trait + 生命周期/诊断/交互桥接 + 可选 LSP Server 输出 + 最小扩展契约
- 定向测试（生命周期 / 诊断回灌 / 审批回路 / 可选 LSP 输出 / 边界隔离）

## 验收标准

- [x] IDE 经 Agent SDK / Headless 协议连接 `pawork` Host，IDE Host Adapter 不构造第二 Core、不取代 GUI Connection Protocol
- [x] 语言服务诊断（P17-4 LSP Client 聚合）可双向回灌，编辑→诊断→Agent 闭环不绕过 Policy
- [x] 文件变更/Diff/Approval 经 IDE 原生交互展示，操作落回 `AppCommand`，IDE 不持可写权威状态
- [x] IDE 与独立 GUI 可并存，二者通道互不触达
- [x] 可选 LSP Server 输出复用 P17-4 聚合结果，且不改变 P17-4 作为 LSP Client 的主定位
- [x] 定向 / Mock smoke 覆盖生命周期、诊断回灌、审批回路、可选 LSP 输出与边界隔离

## 验证记录（2026-08-13）

- 新增 `ide-host-adapter`：只依赖 `agent-sdk` / `headless-json` / `core-api` / `lsp-runtime`，不依赖 `gui-protocol` / `gui-server` / `core-runtime` / `app-service`。
- 接入链路为 IDE Extension → Adapter → `pawork headless --json-stdio` → Core；可选 LSP Server 面复用 P17-4 聚合结果。
- 定向 unit / contract / Host Mock 已绿；独立审查结论为 Built + L1。未跑 Host 簇 L2 前不标已验收。
- Validation Level：L1；Full workspace gate：NOT RUN。

**相关文档**：[P17-4 LSP Client Runtime](P17-4-lsp-runtime.md) · [P17-8 Agent SDK](P17-8-agent-sdk.md) · [CLI Host](../docs/features/cli-host.md) · [GUI 连接与多客户端](../docs/features/gui-connection.md) · [ADR-017 GUI 不直连 Core](../docs/adr/ADR-017-gui-no-direct-access.md) · [ADR-021 CLI/Core 同进程](../docs/adr/ADR-021-cli-core-same-process.md) · [ADR-025 CLI 唯一宿主](../docs/adr/ADR-025-cli-is-sole-host.md) · [ADR-030 Core 单一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [workspace-layout](../docs/architecture/workspace-layout.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增第三方依赖；复用 `agent-sdk` / Headless 协议（P17-8 client）/ `core-api` / `agent-events`，协调 `lsp-runtime`（P17-4，诊断来源与可选 LSP Server 输出复用）。新 crate `ide-host-adapter` 依赖方向：`core-api → agent-sdk → ide-host-adapter`，与 `gui-server` 平级（均为连接 `pawork` Host 的 Client Channel），不嵌入第二 Core。
