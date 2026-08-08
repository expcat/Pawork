# P18-2：Tenant / Principal 身份基线

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-1、P1-3、P1-4、P0-2

**最终目的**：让 Session、Agent、Provider Account、Usage 与 Audit 都有稳定 tenant boundary，同时保持本地单用户零配置体验。

**涉及范围**：新增 `tenant-service`；`agent-domain` ID；`app-database` migration/projection；`core-api`

## 细分步骤

1. **canonical identity** —— 定义 `TenantId`、`PrincipalId` 与 `IdentityContext`，禁止用 API key hash 代替三类身份；目的：建立跨模块统一键。
2. **默认身份** —— 未配置用户固定映射 `tenant_id=local/default`、`principal_id=local/user`；目的：升级后行为不变。
3. **versioned migration** —— 为 legacy session/credential/event projection 增 tenant side table 或 nullable/versioned column，失败事务回滚；目的：不破坏旧库。
4. **传播与查询** —— CommandSource、Session、Agent、RouteContext、Usage、Audit 的创建与查询必须携带身份上下文；目的：阻断隐式全局查询。
5. **隔离测试** —— 覆盖默认迁移、tenant-scoped query、缺失 identity fail-closed；目的：形成后续 RBAC 基线。

## 主要产出物

- `tenant-service` identity API + default resolver
- legacy tenant migration / projection backfill
- identity propagation 与隔离单元测试

## 验收标准

- [ ] 未配置 tenant 的旧用户无感进入 `local/default`
- [ ] 新 Session/Agent/Account/Usage/Audit 不存在无 tenant 归属的持久记录
- [ ] Tenant A 的查询不能返回 Tenant B 的记录
- [ ] migration 可重复执行且失败不留下半迁移状态

**相关文档**：[tenant-audit](../docs/features/tenant-audit.md) · [sessions](../docs/features/sessions.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)

