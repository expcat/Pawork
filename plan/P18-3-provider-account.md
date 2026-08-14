# P18-3：ProviderAccount / Credential 模型与兼容迁移

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢有界完成 · 交付成熟度：PartialWired（正式宿主启动加载持久 account / credential metadata；管理写回、resolver / Provider factory 仍未装配） · 依赖：P18-1、P18-2、P2-6、P6-4、P1-13

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

## 实施与验证记录（2026-08-12）

- `provider-control` 已落地独立的 versioned `ProviderAccountRecord` / `CredentialMetadata`、稳定 DB/serde 状态值、`secret_ref`、expiry/refresh fail-closed gate、tenant-scoped repository 与脱敏管理摘要。
- Provider factory 以 registry 扩展点装配 `ModelProvider`，先校验 tenant/account/provider、descriptor、builder 与 credential 可用性，再在宿主边界短时解析 secret；Core 不按 Provider 名称分支，`provider-api` 契约未被扩张。
- `app-database` control-plane schema 已升级至 v2，覆盖 v1→v2、幂等、事务回滚、备份恢复、跨 tenant 隔离与无 plaintext 列；legacy synthetic default 保留 `local/default` 与原 schema version。
- 定向门禁通过：`cargo test -p provider-control -p auth-service -p app-database -p pawork`、`cargo test -p provider-control --no-default-features`、相关 Clippy、`core-api/orchestration/core-runtime` check、schema typegen 与 diff check。独立 GLM reviewer 结论为 `PASS`。
- P18-3 的领域模型与安全边界已验收，可作为 P18-4 后续依赖；真实 Provider/builtin models、生产 persistent protector 与 `app-service::register_provider` 的宿主闭环属于 P18-14 的 registry/reconciliation/hot-reload 组合职责，并保留为 Phase 18 最终收口项，不视为跨 Phase 延后。

## P14 现状与登记（2026-08-11）

P14-8 的 `QuotaOverview` 查询必须显式 provider_id（缺省即拒绝）；多 provider/多模型聚合语义待 binding enumeration 成为事实源后由 app-service 批量查询（见 [usage-quota](../docs/features/usage-quota.md)）。Quota refresh target 的账号/凭据绑定来源同样是本任务的 ProviderAccount/CredentialMetadata。

## 验收标准

- [x] account 与 credential 生命周期、状态和 ID 独立
- [x] SQLite/Event/log/diagnostics 不出现 plaintext token/API key
- [x] 旧配置迁移后保留同一 Provider/model/credential 的 synthetic binding；真实 Provider 调用闭环随 P18-14 宿主接线复核
- [x] account/credential 查询强制 tenant scope
- [x] ProviderAccount/Credential binding 枚举（按 provider 列出绑定 account/credential）成为 `QuotaOverview` 批量聚合的事实源；无绑定时不做默认 provider 推测
- [x] quota refresh target 的 account/credential 绑定来自 ProviderAccount/CredentialMetadata，不再从零散 credential 数组选择
- [ ] 宿主经 Provider factory / `app-service::register_provider` 装配真实 Provider 并消费 provider `builtin_models()`（Phase 15 host composition deferred 项，见 [P15-10](P15-10-review-remediation.md)）
- [ ] 生产 `ProtectedKeyResolver` 与持久 `ProtectedBlobStoreProtector` 注入正式宿主，兑现 ADR-032「encrypted-at-rest / crash 恢复」；protector 必须按实际 `(provider_id, session_id)` / run scope 构造或选择，禁止把捕获单一 `BlobScope` 的实例注册为跨 Session 共享 Provider 全局状态，并覆盖同 Session 跨轮可回灌、跨 Session fail-closed（Phase 15 持久化 protector 接线延后项，见 [P15-10](P15-10-review-remediation.md)）

**相关文档**：[auth](../docs/features/auth.md) · [provider-control-plane](../docs/features/provider-control-plane.md) · [ADR-014](../docs/adr/ADR-014-secret-os-keychain.md) · [ROADMAP](../ROADMAP.md)
