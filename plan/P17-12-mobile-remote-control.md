# P17-12：Mobile / Remote Control Protocol（受限控制、审批与通知）+ Host 簇收尾门禁

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-8、P13-1、P17-7～P17-11

**最终目的**：定义一个面向移动端/远程的受限控制协议——只暴露审批、通知与受限控制（查看状态、启动/取消任务），不授予完整操作权限，Core 始终是单一事实源（[ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md)）。同时作为 Phase 17「公共 Host/SDK」功能簇（ACP / SDK / IDE / Browser-Computer / Real Remote / Mobile）的收尾任务——在独立 `CARGO_TARGET_DIR` 下跑集中契约门禁并清理，不污染日常 target。

**涉及范围**：新增 `remote-control-adapter` crate（受限控制协议）；复用 `core-api`、`app-service`、`transport-api`、`transport-remote`（P17-11）；Host 簇收尾门禁覆盖 `acp-host` / `agent-sdk` / `headless-json` / `ide-host-adapter` / `browser-computer-runtime` / `transport-remote` / `remote-control-adapter`。不动 `agent-engine` / `gui-protocol`。

## 细分步骤

1. **受限控制协议模型** —— 目的：定义只读 + 受限写的能力子集（查看 Run/Plan/状态、接收通知、审批/拒绝、启动/取消任务），明确「不授予文件写入、工具执行、Provider 直连等完整权限」，把移动端定位为受控观察者而非第二权威。
2. **Remote Control Adapter** —— 目的：在 `core-api` 之上实现 `remote-control-adapter`，经 `transport-remote`（P17-11）承载，把受限请求映射为 `AppCommand`（带受限 `CommandSource`），事件回译为通知；完整权限操作被 Adapter 显式拒绝并审计。
3. **审批与通知通道** —— 目的：把 Policy 的 Approval 请求（P4-9）推送到移动端、回收决策；通知（任务完成/出错/等待审批）为推送式、可去重、断线可补；审批决策落回 Core，移动端不缓存权威状态。
4. **安全与配对** —— 目的：移动端经配对/认证绑定（复用 P17-11 认证 + P6-4），凭证可撤销；受限协议默认最小权限，权限提升需在 Core 侧显式授权并审计。
5. **Host 簇收尾门禁脚本** —— 目的：提供独立 `CARGO_TARGET_DIR=target/gates` 的门禁脚本，对 Host 簇跑集中契约/边界隔离门禁（各 Adapter 都建立在 core-api 之上、都不取代 GUI Connection Protocol、Browser/Computer 经 Policy/Sandbox、Remote 不改 Agent Core、Core 单一事实源），并在 `finally` 执行 `cargo clean --target-dir target/gates`。
6. **门禁执行与清理** —— 目的：在隔离 target 下依次跑各 Adapter 的定向契约与边界断言，汇总可复核结论（非完整日志）；无论成败用 Cargo 清理 `target/gates`。

## 主要产出物

- `remote-control-adapter` crate：受限控制协议 + 审批/通知通道 + 配对/撤销
- Host 簇收尾门禁脚本（独立 `CARGO_TARGET_DIR` + 清理）
- 定向测试（受限能力断言 / 审批回路 / 配对撤销 / Host 簇边界门禁）

## 验收标准

- [ ] 受限控制协议只暴露只读 + 受限写，完整权限操作被显式拒绝并审计
- [ ] 移动端经配对/认证绑定，凭证可撤销；Core 始终是单一事实源，移动端不缓存权威状态
- [ ] 审批/通知可推送、可去重、断线可补，决策落回 Core
- [ ] Remote Control Adapter 经 P17-11 Transport 承载，建立在 `core-api` 之上，不取代 GUI Connection Protocol
- [ ] Host 簇收尾门禁在独立 `CARGO_TARGET_DIR` 下通过：各 Adapter 均 core-api 之上、均不取代 GUI 协议、Browser/Computer 经 Policy/Sandbox、Remote 不改 Agent Core
- [ ] 门禁脚本在 `finally` 执行 `cargo clean --target-dir target/gates`，失败路径也不残留隔离构建缓存

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [policy](../docs/features/policy.md) · [auth](../docs/features/auth.md) · [ADR-014 Secret OS Keychain](../docs/adr/ADR-014-secret-os-keychain.md) · [ADR-017 GUI 不直连 Core](../docs/adr/ADR-017-gui-no-direct-access.md) · [ADR-027 本地远程同协议](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ADR-030 Core 单一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增第三方依赖；复用 `core-api` / `app-service` / `transport-api` / `transport-remote`。新 crate `remote-control-adapter` 依赖方向：`core-api → remote-control-adapter → transport-remote`，与 `gui-server` 平级。Host 簇门禁仅复用既有测试栈，独立 target 目录仅作隔离。
