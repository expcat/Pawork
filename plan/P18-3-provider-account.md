# P18-3：ProviderAccount / Credential 模型与兼容迁移

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-1、P18-2、P2-6、P6-4、P1-13

**最终目的**：把“账号资源”和“认证 secret”拆成独立实体，为一个 Provider 下的多账号、多 credential、健康与 lease 奠定安全数据模型。

**涉及范围**：新增 `provider-control`；`auth-service` metadata API；`app-database` migration；`core-api` 脱敏查询

## 细分步骤

1. **领域实体** —— 定义 versioned `ProviderAccount`、`CredentialMetadata`、priority/weight/concurrency/state 与 tenant scope；目的：账号状态不再塞进 credential。
2. **Secret 边界** —— credential 只持有 `secret_ref`、过期与 refresh state；运行时通过注入的 resolver 得到短生命周期 `ResolvedCredential`；目的：不落明文。
3. **legacy synthetic account** —— 每条旧单 credential 自动映射到 `ProviderAccount(default)` / `Credential(default)`；目的：默认 `SingleCandidate` 行为不变。
4. **脱敏管理 API** —— list/create/disable/delete/test 仅返回 opaque ID 与 masked status；目的：为 CLI/GUI 后续管理提供安全面。
5. **migration/security tests** —— 覆盖多次迁移、回滚、日志/Event/SQLite secret 扫描与跨 tenant account 查询；目的：守住安全红线。

## 主要产出物

- ProviderAccount / CredentialMetadata schema 与 repository
- Secret resolver 边界 + synthetic default migration
- 脱敏管理 API 与 migration/security tests

## 验收标准

- [ ] account 与 credential 生命周期、状态和 ID 独立
- [ ] SQLite/Event/log/diagnostics 不出现 plaintext token/API key
- [ ] 旧配置迁移后仍调用同一 Provider/model/credential
- [ ] account/credential 查询强制 tenant scope

**相关文档**：[auth](../docs/features/auth.md) · [provider-control-plane](../docs/features/provider-control-plane.md) · [ADR-014](../docs/adr/ADR-014-secret-os-keychain.md) · [ROADMAP](../ROADMAP.md)

