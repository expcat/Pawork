# pawork-control-plane

控制面 core：tenant / identity、usage ledger、audit JSONL、quota、credential lease/pool。依赖 `pawork-domain`。

## 职责

单机优先的控制面（ADR-038 D1）：默认宇宙 `local/default`。提供用量记账、审计、租户策略、配额读取与凭证租约。SQLite usage ledger 自开连接（`rusqlite` optional），**不**经 `pawork-storage` Actor。account-control-v1 / binding / OTel exporter 已归档。

## 模块树

```
src/
  lib.rs
  audit.rs  decision.rs  identity.rs  rbac.rs  tenant.rs  usage.rs
  credential/{mod.rs, lease.rs}
  quota/{mod,adapter,domain,error,ledger,service}.rs   # util.rs 私有
fixtures/audit/event-v1.jsonl
```

无 `tests/` 目录；回归在 `src/` + audit JSONL golden。

## 对外入口/API 面

`pub mod`：`audit` / `credential` / `decision` / `identity` / `quota` / `rbac` / `tenant` / `usage`。crate 根再导出 audit/decision/identity/rbac/tenant/usage；**credential / quota 类型走模块路径**。

- **identity**：`IdentityContext`、`DEFAULT_TENANT = "local/default"`、`DEFAULT_PRINCIPAL = "local/user"`。
- **usage**：`UsageLedger`、`UsageRecord`（`RECORD_VERSION = 2`）；feature `sqlite` 下 `SqliteUsageLedger` + `SCHEMA_VERSION = 3`。
- **audit**：`AuditEventV1`、`AuditSink` / `FileAuditStore`；`AUDIT_SCHEMA_VERSION`（JSONL golden）。
- **tenant / rbac**：`TenantPolicyEngine`、`PrincipalRole`、`Permission`、`PermissionProfile`。
- **quota**：`QuotaService`、`QuotaAdapter`、`QuotaSnapshot` / `QuotaOverview`；远端适配器 **不在本包**。
- **credential**：`CredentialPool`（`acquire` / `release` / `reclaim_expired`）、`CredentialLease`（**无 secret 字段**）、`LeaseState`；`CONTROL_PLANE_SCHEMA_VERSION = 2`。

## 依赖与被依赖

- **依赖**：`pawork-domain`。`default = ["sqlite"]`。
- **被依赖**：`pawork-app`（默认 sqlite）；`pawork-orchestration`（`default-features = false`，不拉 rusqlite）。

## 红线与注意事项

- 归档不存在：account-control-v1 九模块、binding/schema、OTel exporter / identity_schema（ADR-038）。RBAC 三类型保留。
- `CredentialLease` 与 audit/quota 视图不得携带明文 Secret；quota 有 `mask_credential_hint`。
- usage `dedup_key` 与 audit JSONL 是冻结契约。
- 产品形态是单机哨兵宇宙，不要按多租户 SaaS 扩张 `local/default`。
- 本包自开 rusqlite 时与 storage Actor **不是**同一连接。

## 相关文档

- [docs/design.md](../../docs/design.md) §2 / §3.2 控制面行
- [docs/adr/ADR-038-inventory-and-product-shape.md](../../docs/adr/ADR-038-inventory-and-product-shape.md)
- [代码地图总索引](../../docs/code-map/README.md)
