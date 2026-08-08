# P18-12：Claude Gateway Adapter

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-10、P18-8、P15-3、P15-7、P12-1

**最终目的**：接入 Claude Code 的 Anthropic Messages wire protocol 与 session/agent identity，使并行 subagent 的用量和审计可归属，并显式处理 signed reasoning continuity 能力。

**涉及范围**：新增 `client-claude-gateway`；`client-adapter-api`；`usage-ledger` / `audit-log` 接线；protocol golden fixtures

## 细分步骤

1. **身份提取** —— 解析 `X-Claude-Code-Session-Id`、`X-Claude-Code-Agent-Id`、`X-Claude-Code-Parent-Agent-Id` 为 `ExternalAgentIdentity`；目的：无需解析 body 即可归属 agent cost。
2. **Messages streaming 映射** —— text/thinking/tool_use/tool_result/usage/error/cancel；目的：保持 canonical Provider/Agent 事件边界。
3. **权限与生命周期** —— 映射 permission、subagent start/stop、task/hook 可观察事件；最终决策仍由 Core policy；目的：adapter 不接管业务。
4. **Reasoning capability** —— signed thinking continuity 只经 capability negotiation 与 Protected Blob 引用处理；目的：不静默丢失或明文落库。
5. **Golden/attribution tests** —— 并行 parent/subagent、断流、cancel、signed continuity、header 缺失/伪造与 tenant 绑定；目的：协议与隔离可验证。

## 主要产出物

- `ClaudeGatewayAdapter` + identity extractor
- Claude ↔ canonical mapping/capability matrix
- streaming、usage attribution、security golden tests

## 验收标准

- [ ] 三个 Claude identity header 映射到 session/agent/parent-agent 并进入 usage/audit
- [ ] header 不作为跨 tenant affinity key，tenant 由受信身份上下文决定
- [ ] signed reasoning 不支持时显式失败，不丢字段、不明文落普通存储
- [ ] adapter 不持有 credential、不覆盖 Core permission decision

**相关文档**：[client-adapters](../docs/features/client-adapters.md) · [tenant-audit](../docs/features/tenant-audit.md) · [ADR-032](../docs/adr/ADR-032-protected-blob-store.md) · [ROADMAP](../ROADMAP.md)

