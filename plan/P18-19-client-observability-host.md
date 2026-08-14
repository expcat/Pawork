# P18-19：External Client / Observability Host Composition

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-10～P18-13、P18-17、P18-18

**最终目的**：为 Codex / Claude adapter 提供正式 `pawork` 入口，并让 client、policy、route、lease、binding 与 quota 的 canonical audit 进入可持久化、可导出的统一链；不把 adapter crate、内存 audit 或 exporter trait 等同于生产可用。

**涉及范围**：`apps/pawork` CLI/composition、Codex App-Server / Claude Gateway adapters、`client-adapter-api`、`app-service` audit sinks、`audit-log` OTel exporter、Quota WebScrape adapter。

## 细分步骤

1. **外部 Client 入口** —— 增加 Codex app-server 与 Claude Messages/SSE 的正式 stdio/transport 入口，复用 `ClientAdapterHost`、Session Registry、identity 与 ownership epoch。目的：adapter 不再只有 fixture/host seam。
2. **完整 canonical audit** —— client negotiation、policy、route、health、lease、binding/rebound、quota refresh 都写同一 versioned audit sink；重启后可回放且跨 tenant 查询受策略约束。目的：所有控制决策可取证。
3. **生产 exporter 生命周期** —— 宿主配置并管理 OTel collector/exporter start/flush/shutdown；WebScrape quota factory 注入同一 allowlist audit sink。目的：观测出口真实可用且不泄露 Secret。

## 主要产出物

- Codex / Claude 正式宿主入口与协议 E2E
- 完整控制面 durable audit coverage matrix
- OTel collector/exporter 与 WebScrape audit sink 生命周期
- allowlist、redaction、restart/replay 与 tenant-isolation tests

## 验收标准

- [ ] Codex 与 Claude 至少各一条真实 `pawork` 入口经 `ClientAdapterHost` 完成会话/Run，不绕过 app-service
- [ ] negotiation/policy/route/health/lease/binding/quota 事件均可持久回放，序列与 tenant boundary 可证
- [ ] `LeaseRebound` 只来自真实 binding acquisition，不使用 lease version 启发式
- [ ] OTel collector/exporter 可配置 start/flush/shutdown，失败 fail loud 或产生明确降级诊断
- [ ] WebScrape quota factory 使用正式 allowlist audit sink；日志、trace、metric 与 audit payload 均无 Secret

**相关文档**：[P18 Review](../docs/review/p18-review.md) · [client adapters](P18-10-client-adapter-framework.md) · [audit](P18-13-audit-otel.md) · [tenant-audit](../docs/features/tenant-audit.md) · [ADR-016](../docs/adr/ADR-016-core-event-persist-replay.md)
