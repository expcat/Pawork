# P18-12：Claude Gateway Adapter

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢TargetVerified（workspace member + identity hoist + host seam） · 交付成熟度：HostSeamVerified · 依赖：P18-10、P18-8、P15-3、P15-7、P12-1

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

- [x] 三个 Claude identity header 映射到 session/agent/parent-agent 并进入 usage/audit
- [x] header 不作为跨 tenant affinity key，tenant 由受信身份上下文决定
- [x] signed reasoning 不支持时显式失败，不丢字段、不明文落普通存储
- [x] adapter 不持有 credential、不覆盖 Core permission decision

**相关文档**：[client-adapters](../docs/features/client-adapters.md) · [tenant-audit](../docs/features/tenant-audit.md) · [ADR-032](../docs/adr/ADR-032-protected-blob-store.md) · [ROADMAP](../ROADMAP.md)

## 当前进度（2026-08-13）

- `client-claude-gateway` 已加入根 workspace members；嵌套 `[workspace]` / profiles 已删除；deps 归一 `.workspace = true`。
- 协议中立 `ExternalAgentIdentity` / `TrustedTenantContext` / `TenantBinding` / `bind_tenant` 上移到 `client-adapter-api`。Claude crate 保留头名、提取与 `ClaudeSessionId` / `ClaudeAgentId` 校验，并 re-export 共享类型。
- `app-service` 经现有 `ClientAdapterHost::register_factory` 注册 Claude factory；`ClaudeGatewayHost` 把 header 身份绑到受信 tenant，写入 canonical audit dimensions，并经 `apply_external_identity` 填入 usage-ledger 字段（tenant 来自宿主，session/agent/parent-agent 来自身份，不是 affinity key）。
- signed thinking：`ReasoningProtectorBridge` 把 `ProtectedBlobStoreProtector` / `InMemoryReasoningProtector` 注入 adapter seam；`InMemoryClaudeProtectorFactory` / `ProductionClaudeProtectorFactory` 按 `(provider_id, session_id)` 隔离，跨 Session 不共享 protector。
- L1：`cargo test -p client-claude-gateway`（55 passed）、`cargo test -p client-adapter-api`（15 passed）、`cargo test -p app-service --lib`（108 passed）；`clippy -p client-claude-gateway -p client-adapter-api --all-targets -- -D warnings` 通过。

## 遗留（不阻塞本任务 TargetVerified）

- **`pawork` CLI stdio 入口未做**：完整 Messages/SSE 服务器需要 P18-14 的生产宿主装配（credential lease、ProtectedBlobStore 密钥、quota runtime）。当前只提供 app-service 注册 + protector 注入 seam。
- **Codex `client-codex-app-server` 已由 orchestrator 加入根 workspace**（嵌套 `[workspace]` 已删）。
- Run supervisor 仍用 `canonical_root_agent_id(session)` 作为 live run 的 agent id；Claude subagent 身份通过 host seam 进入 ledger/audit stub，尚未改 RunRequest 装配（避免与 P18-14 冲突）。
