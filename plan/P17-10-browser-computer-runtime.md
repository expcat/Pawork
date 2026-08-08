# P17-10：Browser / Computer Runtime（可替换后端）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-5、P3-4、P4-9、P11-1（协调 P15-1）

**最终目的**：为 Agent 提供统一的 Browser / Computer 使用运行时——一个 `AgentTool`，背后是可替换的执行后端（local 浏览器 / Playwright / MCP browser server / provider-hosted computer use）。所有浏览器与桌面操作都经 `policy-engine` 审批与 `sandbox-runtime` 隔离执行，把「电脑/浏览器使用」收敛为 canonical tool，Agent Engine 不因后端不同走分支（[ADR-002](../docs/adr/ADR-002-agent-engine-provider-decoupled.md)/[031](../docs/adr/ADR-031-sandbox-backend-architecture.md)）。

**涉及范围**：新增 `browser-computer-runtime` crate；复用 `tool-api`/`tool-runtime`（P0-5/P3-4）、`policy-engine`（P4-9）、`sandbox-runtime`（P11-1）、`provider-api`（provider 后端，协调 P15-1 canonical tool v2）。不动 `agent-engine` 的分发逻辑。

## 细分步骤

1. **统一工具接口与 capability** —— 目的：在 `browser-computer-runtime` 定义统一的 Browser/Computer tool（导航/点击/输入/截图/读 DOM/桌面坐标操作等），作为 `AgentTool`（P0-5），后端差异封装在 trait 之后；Agent 只见 canonical 描述与结果。
2. **可替换后端 trait** —— 目的：定义 `BrowserComputerBackend` trait（local / Playwright / MCP / provider 四档实现），统一 `spawn`/`act`/`snapshot`/`teardown`，按 ADR-031 的「分层 + 探测回退」思路选择可用后端，回退可观测。
3. **Policy 与 Sandbox 约束** —— 目的：所有操作经 `policy-engine`（P4-9）审批——网络/文件/坐标操作按 capability 约束；浏览器/驱动子进程经 `sandbox-runtime`（P11-1）的 `SandboxBackend::spawn` 执行，不绕过沙箱直接起进程。
4. **Provider 后端（canonical）** —— 目的：provider-hosted computer use 经 `provider-api` canonical 域接入（协调 P15-1 hosted tool v2），不按 Provider 名分支；本地后端失败时可降级到 provider 后端并显式记录。
5. **结果归一与 artifact 引用** —— 目的：截图/DOM/大输出经 artifact-store 引用（避免大 payload 进上下文，[ADR-018](../docs/adr/ADR-018-large-payload-artifact-id.md)），结果归一为统一结构；操作可取消、可审计。
6. **定向 / Mock 测试** —— 目的：用 Mock 后端覆盖「工具调度 → policy 审批 → sandbox 执行 → 结果归一」全链路、后端探测与降级、provider 后端不分支断言、大输出走 artifact。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `browser-computer-runtime` crate：统一 tool + 可替换后端 trait + 四档后端（local/Playwright/MCP/provider 占位或最小实现）
- Policy/Sandbox 集成 + 结果归一/artifact 引用
- 定向测试（全链路 / 后端探测降级 / provider 不分支 / artifact 引用）

## 验收标准

- [ ] Browser/Computer 是统一 `AgentTool`，后端可替换（local/Playwright/MCP/provider），Agent 不感知后端
- [ ] 所有操作经 `policy-engine` 审批、经 `sandbox-runtime` 执行，不绕过沙箱
- [ ] provider-hosted computer use 走 canonical 域，Agent Engine 不按 Provider 名分支
- [ ] 后端选择有探测与可观测回退，provider 后端降级显式记录
- [ ] 截图/DOM/大输出经 artifact 引用，操作可取消可审计
- [ ] 定向 / Mock smoke 覆盖全链路与 provider 不分支断言

**相关文档**：[policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [tools](../docs/features/tools.md) · [providers](../docs/features/providers.md) · [ADR-002 Agent Engine 与 Provider 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-018 大 payload 走 artifact](../docs/adr/ADR-018-large-payload-artifact-id.md) · [ADR-031 沙箱分层](../docs/adr/ADR-031-sandbox-backend-architecture.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：local 后端可用 `fantoccini`（WebDriver/CDP）或自有 CDP 封装；MCP 后端复用 `mcp-client`；Playwright 后端经子进程、由 sandbox 执行。新增依赖需回填 ROADMAP「依赖选型基线」并在 sandbox 约束下运行；provider 后端不新增依赖（复用 `provider-api`）。
