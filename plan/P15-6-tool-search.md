# P15-6：Tool Search（按需工具发现与激活）

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P0-5、P3-4、P4-9、P9-3、P15-1

**最终目的**：让 Agent 在面对大量可用工具（内置 + MCP + GUI 工具 + Provider extension）时，不必把全部工具 schema 一次性塞入上下文，而是按需搜索一个「延迟加载工具索引」并激活匹配项。降低上下文占用、提升工具选择精度，并为 P15-1 的 `ProviderExtension` 提供统一发现入口。Core 不因工具来源不同走特例。

**涉及范围**：`tool-runtime`（工具索引 + 搜索 + 激活器）、`agent-domain`（工具 manifest 搜索字段，复用 `ToolDescriptor`）；与 P9-3 MCP 能力发现、P15-1 registry 协同，不新增 crate。

## 细分步骤

1. **延迟加载工具索引** —— 目的：建立「已声明但未激活」的工具 manifest 索引（名称、描述、capabilities、来源、`requires_approval`），来源覆盖内置（P4-*）、MCP（P9-3）、GUI 工具、Provider extension（P15-1）；激活前不进入 CanonicalModelRequest 的 tools 列表。
2. **搜索 API** —— 目的：提供 `search_tools(query) -> Vec<ToolMatch>`，按名称/描述/capabilities 做轻量匹配（参考 ripgrep 同源 `ignore`/`globset` 思路的最小子集，不引入搜索引擎依赖），返回匹配项与激活状态。
3. **激活器** —— 目的：`activate_tool(id)` 把延迟工具移入活跃 registry，使其可被 scheduler 路由（P3-4）；激活是幂等的，重复激活不报错。
4. **ProviderExtension 激活审批** —— 目的：激活 `ProviderExtension` 类工具时复用 P4-9 PolicyEngine，未信任工作区默认拒绝、首次激活需显式审批；与 P15-1 §6 审计一致。
5. **与 hosted tools 的边界** —— 目的：明确 `ProviderHosted`（server tool）不参与本地搜索激活——它由 P15-8 能力协商在请求侧声明；Tool Search 只管本地可激活工具（ClientFunction / ProviderExtension）。
6. **上下文预算联动** —— 目的：与 P3-2 上下文预算协同，激活工具后其 schema 计入 tools token 预算，超限时优先保留当前轮已用工具。
7. **Mock smoke** —— 目的：构造一批延迟工具，验证搜索返回正确匹配、激活后可被 scheduler 路由、ProviderExtension 激活走审批闸门。

## 主要产出物

- `tool-runtime`：延迟加载工具索引 + 搜索 API + 激活器
- `agent-domain`：工具 manifest 搜索字段（复用 ToolDescriptor）
- Mock smoke：搜索 / 激活 / 审批闸门用例

## 验收标准

- [x] 搜索仅返回未激活且匹配 query 的工具，已激活项不重复出现
- [x] 激活后工具可被 scheduler 路由执行（ClientFunction，Mock smoke）
- [x] `ProviderExtension` 激活在未信任工作区被默认拒绝，需显式审批（用例）
- [x] `ProviderHosted` 不进入本地搜索/激活流程（边界断言）
- [x] 激活后 schema 计入 P3-2 tools token 预算（用例）
- [x] 不引入搜索引擎类第三方依赖；仅定向/Mock smoke 验收，不要求 workspace 全量门禁

## 验证记录（2026-08-12）

- 索引与搜索：四来源（内置 / MCP / GUI / Provider extension）批量声明、激活后不再出现在搜索结果、
  名称 / 描述 / capabilities 三种匹配面、大小写不敏感、空 query 不返回结果。
- 激活与路由：ClientFunction 激活后经 `ToolScheduler::execute_named` 本地执行成功；幂等重复激活为
  no-op；Extension 激活后仅可走 `authorize_provider_call`（ProviderTranscript），不进入本地执行。
- 审批闸门：Extension 激活在未信任工作区默认拒绝；信任工作区 + 自动放行器 fail closed；显式审批
  放行、显式拒绝不激活（与 P15-1 §6 一致）。
- 边界：`ProviderHosted` 声明被 `HostedNotIndexed` 拒绝；来源与执行位点不一致被拒绝。
- 预算联动：schema 计数与 context-engine 启发式口径对齐（JSON + 8 framing、CJK 1:1、其余 4:1）；
  超限时淘汰「当前轮未使用」工具（`mark_used` / `start_round`），当前轮已用工具占满预算时拒绝激活；
  被淘汰工具回到已声明集合可重新激活。
- `ToolRegistry::remove` 用于预算淘汰，移除后不再可路由。
- 未新增第三方依赖（搜索匹配与 token 计数均为自实现最小子集）。

```text
Validation Level: L1
Affected crates: tool-runtime
Validated: cargo test -p tool-runtime（27 passed）/ cargo clippy -p tool-runtime --all-targets -- -D warnings（0 warning）/ cargo check -p agent-domain -p tool-api（确认未受影响）
Targeted regressions: 既有 scheduler 全量用例回归（26 项）+ 本任务 7 项新用例；ToolRegistry::remove 定向用例
Full workspace gate: NOT RUN（未命中升级条件）
```

**相关文档**：[tools](../docs/features/tools.md) · [mcp](../docs/features/mcp.md) · [policy](../docs/features/policy.md) · [context](../docs/features/context.md) · [ADR-008 capability 分类](../docs/adr/ADR-008-builtin-tools-capability.md) · [P15-1](P15-1-canonical-tool-v2.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：搜索匹配用最小子集自实现（参考 ROADMAP「完全自实现」对工具调度的口径），不引入新依赖；若后续需更强相关性再评估。
