# P18-2：Tenant / Principal 身份基线

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：tenant/principal identity 与 Session/Run 隔离已闭合；Account/Lease/Ledger/Audit 的跨任务验收由 P18-3/4/8/13 收口） · 依赖：P18-1、P1-3、P1-4、P0-2

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

## P14 现状与登记（2026-08-11）

P14-7 的 run usage 归属目前是 synthetic：`RunSupervisor::record_run_usage` 固定 `tenant=local`、`account=local/default`、`credential_id=None`，principal/agent 为默认身份，本地 Ledger 进程内有效（见 [usage-quota](../docs/features/usage-quota.md)）。durable 归属由本任务与 P18-3/4/8 共同闭合。

## 验收标准

- [x] 未配置 tenant 的旧用户无感进入 `local/default`
- [ ] 新 Session/Agent/Account/Usage/Audit 不存在无 tenant 归属的持久记录（本任务已闭合 Session/Run 与 usage identity；Account/Audit 实体分别由 P18-3/P18-13 创建后最终勾选）
- [x] Tenant A 的 Session/Run 查询、命令幂等 replay 不能返回或复用 Tenant B 的记录；Account/Usage/Audit 查询在各自任务复用同一 `IdentityContext` 边界
- [x] migration 可重复执行且失败不留下半迁移状态
- [ ] `record_run_usage` 的 tenant/principal/account/credential 来自真实 IdentityContext 与 binding/lease，生产路径不再硬编码 synthetic 默认值
- [ ] usage 记录 durable 归属：CLI 进程重启后仍可按 tenant/account/credential/provider/model 查询（配合 P18-8 持久化）

## 验证记录（2026-08-12）

- Validation Level: L1
- Affected crates: `agent-domain`、`core-api`、`tenant-service`、`app-database`、`session-store`、`app-service`、`snapshot-service`、`gui-server`；关键直接消费者 `cli-host`、`core-runtime`、`provider-control`、`compaction-engine`
- Validated: `cargo fmt -p agent-domain -p core-api -p tenant-service -p app-database -p session-store -p app-service -p provider-control -p snapshot-service -p gui-server -- --check`；`cargo test -p agent-domain -p core-api -p tenant-service -p app-database -p session-store -p app-service`（219 passed）；`cargo test -p app-service -p snapshot-service -p gui-server`（116 passed，含 GUI 多连接/重连）；`cargo check -p cli-host -p core-runtime -p provider-control -p compaction-engine -p snapshot-service -p gui-server --all-targets`；`cargo run -p schema-typegen -- --check`；`git diff --check`
- Targeted regressions: 空白/缺失 identity fail-closed、`ActorIdentity` canonical principal 映射、多 principal 同 tenant、Session/Run 查询与取消/重试/审批跨 tenant 返回 NotFound、tenant-scoped idempotency、legacy session identity backfill、Session export v1/v2 安全回填与 v3 identity 严格校验、migration 幂等/事务回滚、真实 run 记账→`local/default` quota cache→下一 run 硬停、GUI 握手/请求/resume 快照按连接 tenant 隔离、EventHub replay 异步发布竞态
- Deferred closure: account/credential 真实归属（P18-3/4）、唯一持久 Usage Ledger（P18-8）、canonical audit persistence/query（P18-13）
- Full workspace gate: NOT RUN（未命中升级条件）
- Independent review: DeepSeek reviewer 首轮发现旧 quota tenant key 与未分租户 GUI snapshot；修复并补真实链路回归后复验 **PASS**。

**相关文档**：[tenant-audit](../docs/features/tenant-audit.md) · [sessions](../docs/features/sessions.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)
