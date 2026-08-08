# 安全验收

发布前必须通过以下全部验收项：

1. 路径穿越测试
2. symlink escape 测试
3. Windows junction 测试
4. Shell 注入测试
5. Secret 日志泄漏测试
6. MCP 权限测试
7. WASM capability 测试
8. OAuth callback 测试
9. Session 数据损坏测试
10. Tool Output 资源耗尽测试
11. 子进程树清理测试
12. Plugin 无限循环测试
13. 数据库 Migration 故障测试
14. 未信任工作区测试
15. Sandbox 逃逸测试
16. 敏感制品隔离测试（reasoning 凭证走 Protected Blob Store；不落普通 Event payload / 日志 / OS Keychain；ADR-032）
17. Hosted Tool 执行位点测试（ProviderHosted / ProviderExtension 不触发本地 `AgentTool::execute`，结果走 ServerToolEvent）
18. User Hook 策略与信任边界测试（PromptTransform 不可绕过 system/security policy；User Hook 与 WASM lifecycle hook 信任边界隔离、不重复执行；secret 不入库/日志）
19. Canonical 解耦回归（Agent Core / memory-service / user-hooks 不含 Provider 名分支；effort 走 canonical，不经 `provider_options`）
20. Provider Account Secret 隔离（ProviderAccount/Credential/Lease/Event/Audit 只保存 opaque ID / `secret_ref`，SQLite、日志、诊断包无 plaintext token）
21. Credential Lease 并发与回收（per-account 上限、cancel/drop/crash 幂等 reclaim；ClientCancelled 不降低 health）
22. 错误域隔离（ContextTooLarge / InvalidRequest / ProtocolIncompatible 不轮询账号；401/402/429/5xx 按 scope 执行动作）
23. 跨 Tenant 隔离（Tenant A 不可访问 B 的 credential/session/agent transcript/affinity/usage/audit）
24. Client Adapter 边界（Codex/Claude/ACP 不持有 credential、不绕过 policy/app-service、不与 GUI protocol frame 混用）
25. Session ownership/revision（陈旧 `ownership_epoch` 写入被拒，断线重连先重同步）

## 相关文档

- [policy](../features/policy.md) · [sandbox](../features/sandbox.md) · [plugins](../features/plugins.md) · [mcp](../features/mcp.md) · [sessions](../features/sessions.md)
- [provider-control-plane](../features/provider-control-plane.md) · [client-adapters](../features/client-adapters.md) · [tenant-audit](../features/tenant-audit.md)
- [ADR-009 默认 Workspace Trust](../adr/ADR-009-default-workspace-trust.md) · [ADR-014 Secret 存 OS Keychain](../adr/ADR-014-secret-os-keychain.md) · [ADR-020 性能与安全是发布门槛](../adr/ADR-020-performance-security-gate.md) · [ADR-032 Protected Blob Store](../adr/ADR-032-protected-blob-store.md)
- [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md) · [ROADMAP 实施波次与门禁节奏](../../ROADMAP.md#实施波次与门禁节奏) · [P18-15 Control Plane Gate](../../plan/P18-15-control-plane-gate.md) · [测试后清理](testing.md#测试后清理)
