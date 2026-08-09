# P9-5：Approval / 输出限制 / Secret 注入

> Phase 9 · MCP · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P4-9、P9-3

**最终目的**：实现每个 MCP server 独立权限配置（审批、输出限制、Secret 注入），让外部扩展可控、安全。

**涉及范围**：`policy-engine`、`mcp-client`

## 细分步骤

1. **每 server 独立权限配置** —— 目的：细粒度授权。
2. **审批接入 Policy** —— 目的：统一审批流。
3. **输出限制** —— 目的：防超大输出。
4. **Secret 注入（引用而非明文）** —— 目的：不泄漏。

## 主要产出物

- MCP 审批/输出限制/Secret 注入

## 验收标准

- [x] 每个 server 有独立权限
- [x] Secret 以引用注入，不落明文

## 验证记录（2026-08-09）

- 每 Server 独立 trust、approval mode、Tool / Workspace allowlist 与输出上限，写操作经 `PolicyEngine` 和可审计 approval channel。
- persisted config 只接受 `SecretRef`；连接 / 重连前从 `SecretBackend` 延迟解析，配置、Debug、URL、错误和 stderr 路径均有脱敏测试。

**相关文档**：[mcp](../docs/features/mcp.md) · [policy](../docs/features/policy.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
