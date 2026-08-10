# P19-10：Provider / Account / Auth / Quota

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2、P19-3、P6-4、P14-1～P14-9、P15-8、P18-1～P18-9、P18-14

**最终目的**：提供脱敏的模型与账号控制面，让用户配置认证、查看能力/健康/路由/Usage/Quota 与 tenant policy，同时保证 Desktop 永远拿不到 credential 明文或 Lease secret。

**涉及范围**：Desktop Settings controller、Provider/Model catalog、ProviderAccount/Auth flows、Health/Route explanation、Usage/Quota views、Tenant policy presentation

## 细分步骤

1. **Provider/Model catalog** —— 展示 capability、context、pricing/status 与来源，选择合法 model/profile。目的：避免静态硬编码供应商表。
2. **Account 管理** —— list/create/disable/delete/test 只展示 opaque ID、masked status、tenant scope。目的：账号与 Secret 分离。
3. **Auth flows** —— API Key 通过受控一次性输入提交，OAuth PKCE/device/callback 显示 state/expiry/cancel；成功后清空输入状态（GUI 内存）。目的：Secret 不停留渲染层。
4. **Health/Route** —— 展示 cooling down/billing blocked/disabled、failure scope、affinity 与 route explanation，不在 GUI 重新选账号。目的：解释控制面而非复制逻辑。
5. **Usage/Quota** —— account/model/window/cost/刷新/告警/数据新鲜度，区分本地 Ledger 与远端 Quota。目的：口径清晰。
6. **Tenant policy** —— ACL、预算、并发、保留策略只消费 P18 API；缺权限时 fail-closed。目的：多租户安全。
7. **Secret/security tests** —— GUI 内存/本地持久化/log/screenshot/crash fixture 扫描，过期 OAuth、多账号 race、stale quota。目的：高风险验收。

## 主要产出物

- Provider/Model/Account/Auth/Quota Settings pages
- Health/route/affinity/tenant policy diagnostics
- Secret redaction、OAuth、freshness 与 permission tests

## 验收标准

- [ ] Desktop API/状态中无 plaintext credential/Lease secret/Protected Blob
- [ ] API Key/OAuth 成功或取消后敏感输入从 GUI 内存/state/log 清除
- [ ] Account health/routing 只展示 Core explanation，GUI 不做账号轮询
- [ ] Usage Ledger 与 remote Quota 的来源、窗口、freshness 与误差明确
- [ ] 跨 tenant/无权限 query 和 command fail-closed

**相关文档**：[auth](../docs/features/auth.md) · [models](../docs/features/models.md) · [usage-quota](../docs/features/usage-quota.md) · [provider-control-plane](../docs/features/provider-control-plane.md) · [tenant-audit](../docs/features/tenant-audit.md)
