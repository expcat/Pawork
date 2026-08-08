# P18-15：Control Plane Contract / Security Gate

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-1～P18-14、P17-7、P12-1、P12-2、P12-6

**最终目的**：集中验证账号控制面、Tenant、ClientAdapter 与 AgentSupervisor 的跨 crate 不变量，作为 Phase 18 的 `MaintenanceGated` 收尾；不在每个前置任务重复 workspace 全量门禁。

**涉及范围**：`provider-control` / `tenant-service` / `usage-ledger` / `audit-log` / `client-*` / `acp-host` / `orchestration`；独立 `target/gates`

## 细分步骤

1. **Selector/property gate** —— priority、weighted distribution、fill-first、affinity/rebind、fallback explanation；目的：路由可证明。
2. **Concurrency/recovery gate** —— lease/Agent 并发上限、cancel/drop/crash/reclaim、hot reload；目的：无泄漏和超配。
3. **Migration/security gate** —— legacy default migration、Secret 扫描、cross-tenant credential/session/agent/usage/audit chaos；目的：安全边界。
4. **Protocol golden gate** —— Codex Thread/Turn/Item/approval/subagent（并区分 remote compaction 与 local compaction 两类 fixture）、Claude Messages/identity/reasoning、ACP initialize/session create/resume/prompt/update/permission/tool event/cancel 与 custom model；目的：客户端版本回归可见，每条重要协议消息一个 fixture。
5. **错误/故障注入 gate** —— 401/402/provider-specific 400/429/QuotaExceeded（hard/soft）/5xx/cancel/context/protocol/stream interruption；目的：失败域不串味。
6. **回滚演练与 L2** —— 关闭 feature flags、回退 synthetic account/SingleCandidate、schema forward/rollback/restore，在独立构建目录跑相关 fmt/test/clippy/schema；目的：可发布、可撤回。

## 主要产出物

- 账号池 property/concurrency/error test suite
- Codex/Claude/ACP versioned golden matrix
- migration/isolation/redaction/chaos suite 与 rollback runbook
- Phase 18 L2 门禁脚本（完成后清理隔离缓存）

## 验收标准

- [ ] 单 Provider + 单 credential、`local/default` 行为保持兼容
- [ ] Secret 不落 SQLite/Event/log；跨 tenant 的 credential/session/agent/usage/audit 均不可观察
- [ ] lease/Agent 并发、affinity、cancel、error/fallback 不变量全部通过
- [ ] Codex/Claude/ACP 关键协议 golden 与 unsupported-field 行为通过
- [ ] feature-off/schema rollback/runtime fallback 演练成功
- [ ] 相关 crates 的 fmt/test/clippy/schema L2 在独立目录通过并完成清理

**相关文档**：[testing](../docs/quality/testing.md) · [security-acceptance](../docs/quality/security-acceptance.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)
