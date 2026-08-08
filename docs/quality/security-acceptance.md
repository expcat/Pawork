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

## 相关文档

- [policy](../features/policy.md) · [sandbox](../features/sandbox.md) · [plugins](../features/plugins.md) · [mcp](../features/mcp.md) · [sessions](../features/sessions.md)
- [ADR-009 默认 Workspace Trust](../adr/ADR-009-default-workspace-trust.md) · [ADR-014 Secret 存 OS Keychain](../adr/ADR-014-secret-os-keychain.md) · [ADR-020 性能与安全是发布门槛](../adr/ADR-020-performance-security-gate.md)
- [ROADMAP 实施波次与门禁节奏](../../ROADMAP.md#实施波次与门禁节奏) · [测试后清理](testing.md#测试后清理)
