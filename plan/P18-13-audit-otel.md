# P18-13：Canonical Audit Event / OTel Export

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢有界完成 · 交付成熟度：PartialWired（正式宿主持久化 policy / route audit；完整事件覆盖与 OTel collector 未装配） · 依赖：P18-2、P18-4～P18-10、P1-9、P1-10、P1-11

**最终目的**：把身份、policy、route、lease、Agent、permission/tool 与 Client 行为写成 tenant-scoped canonical audit event，并以脱敏 allowlist 导出 OTel/SIEM。

**涉及范围**：扩展 `audit-log`；`diagnostics` / metrics / tracing；Event Store projection；OTel exporter abstraction

## 细分步骤

1. **AuditEvent v1** —— actor/action/target/decision/tenant/principal/agent/trace/version；目的：建立不可变审计词汇。
2. **关键接线** —— identity resolve、policy allow/deny、route/fallback、lease acquire/release/rebind、Agent lifecycle、approval/tool、config change/export；目的：端到端可解释。
3. **Trace/metric 维度** —— tenant/session/agent/provider/account/client/trace；目的：跨状态机相关联。
4. **脱敏 exporter** —— OTel/SIEM 只输出 allowlist，默认排除 prompt、tool output、secret_ref/secret 与 Protected Blob；目的：审计不泄密。
5. **重放/隔离测试** —— audit projection 重建、跨 tenant query、export redaction、失败路径完整性；目的：可取证。

## 主要产出物

- versioned `AuditEventV1` 与 projection/query API
- tracing/metrics 字段规范 + exporter abstraction
- replay/isolation/redaction tests

## P14 现状与登记（2026-08-11）

P14-4/9 存在两处审计职责重叠：WebScrape adapter 内置有界内存 audit Vec（`audit_entries`，测试/诊断用），`RefreshScheduler` 另有 `AuditSink`，均无生产消费端（见 [usage-quota](../docs/features/usage-quota.md)）。canonical audit 落地后统一为外部 sink，WebScrape 审计事件交给 sink。

## 验收标准

- [x] route/fallback/lease/policy/agent/client 关键决策均有可解释 audit event
- [x] Tenant A 不能查询或导出 Tenant B 的审计
- [x] exporter/diagnostics 不含 plaintext secret、prompt、tool output、Protected Blob
- [x] trace 可关联 tenant/session/agent/provider/account/client 而不暴露敏感值
- [x] WebScrape 内置 audit Vec 移除或降为测试夹具；生产只保留 scheduler/控制面外部 audit sink
- [x] quota refresh / 告警（含脱敏 kind/source）写入 canonical audit event，可跨 tenant 隔离查询与导出
- [x] ACP 接入审计（P17-7 延期落点）：session create/resume、prompt/update、permission/tool event、cancel 与能力协商等关键决策写入 canonical audit event，可跨 tenant 隔离查询与导出

**相关文档**：[tenant-audit](../docs/features/tenant-audit.md) · [observability](../docs/features/observability.md) · [ADR-016](../docs/adr/ADR-016-core-event-persist-replay.md) · [ROADMAP](../ROADMAP.md)

## 当前进度（2026-08-13）

- 新增 workspace crate `audit-log`：`AuditEventV1`、内存/JSONL durable store、tenant query、allowlist exporter、schema/duplicate/corruption fail-closed 与回归测试。
- app-service 已桥接 policy、lease、agent、approval、client lifecycle、quota refresh、**route fallback**、**config change**、**audit export**、**QuotaAlert**；lease rebind 在 `lease.version > 2` 时发出 `LeaseRebound`，并有独立 unit path。
- `audit-log` 增加 `OtelAuditExporter` / `TracingAuditExporter` / `InMemoryOtelExporter`（无第三方 OTel SDK；生产 collector 进程不在本任务范围）。
- WebScrape 生产路径只经可选 `refresh::AuditSink`；进程内 Vec 与 `audit_entries()` 仅 `#[cfg(test)]`。
- ACP host 对 initialize（能力协商）、session/new、session/cancel 写入 canonical audit；prompt/permission 仍走既有 ClientAdapterHost → router 的 Agent/Approval 事件。
- 未做：生产 OTel collector 进程；provider-control binding reconciler 内的 rebind 事件直出（禁止改 reconciler）；Claude host 未改；WebScrape 生产 emit 需 composition 调用 `with_audit_sink`（zhipu factory 默认不注入，scheduler `AuditSink` 仍记录 refresh 周期）。
- **LeaseRebound 生产路径**：`LeaseRecord::open` 的 public lease `version` 恒为 2，因此 `lease.version > 2` 启发式永不触发，已从 `RunSupervisor` acquire 路径删除。隔离测试仍经 `record_control_event` 注入 `LeaseRebound`。真实 emit 应挂在 `BindingAcquisition.old_lease_release.is_some()`（app-service 消费 `SessionBindingService` 之后）。
