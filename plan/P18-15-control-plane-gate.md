# P18-15：Control Plane Contract / Security Gate

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢已落地 · 交付成熟度：MaintenanceGated · 依赖：P18-1～P18-14、P17-7、P12-1、P12-2、P12-6

**最终目的**：集中验证账号控制面、Tenant、ClientAdapter 与 AgentSupervisor 的跨 crate 不变量，作为 Phase 18 的 `MaintenanceGated` 收尾；不在每个前置任务重复 workspace 全量门禁。

**涉及范围**：`provider-control` / `tenant-service` / `usage-ledger` / `audit-log` / `client-*` / `acp-host` / `orchestration` / `app-database` / `app-service` / `ide-host-adapter` / `quota-service`；隔离目录 `target/gates-p18`（**不**复用其他阶段的 `target/gates`）。

**入口**：[`scripts/p18-gate.sh`](../scripts/p18-gate.sh)。`CARGO_TARGET_DIR` 默认 `target/gates-p18`，可用 `P18_GATE_TARGET_DIR` 覆盖；`P18_GATE_KEEP_TARGET=1` 时保留隔离缓存。`trap EXIT INT TERM` 在成败后均 `cargo clean --target-dir` + `rm -rf`。全程不跑 `cargo test --workspace` / `clippy --workspace`。

## 细分步骤

1. **Selector/property gate** —— priority、weighted distribution、fill-first、affinity/rebind、fallback explanation；目的：路由可证明。
2. **Concurrency/recovery gate** —— lease/Agent 并发上限、cancel/drop/crash/reclaim、hot reload；目的：无泄漏和超配。
3. **Migration/security gate** —— legacy default migration、Secret 扫描、cross-tenant credential/session/agent/usage/audit chaos；目的：安全边界。
4. **Protocol golden gate** —— Codex Thread/Turn/Item/approval/subagent（并区分 remote compaction 与 local compaction 两类 fixture）、Claude Messages/identity/reasoning、ACP initialize/session create/resume/prompt/update/permission/tool event/cancel 与 custom model；目的：客户端版本回归可见，每条重要协议消息一个 fixture。
5. **错误/故障注入 gate** —— 401/402/provider-specific 400/429/QuotaExceeded（hard/soft）/5xx/cancel/context/protocol/stream interruption；目的：失败域不串味。
6. **回滚演练与 L2** —— 关闭 feature flags、回退 synthetic account/SingleCandidate、schema forward/rollback/restore，在独立构建目录跑相关 test/clippy；目的：可发布、可撤回。
7. **Schema 版本一致性** —— 跨 crate 断言 `core-api` / `provider-control` / `app-database` 的 control-plane schema version 完全一致；目的：防止单点 bump 导致 wire、domain 与 migration 漂移。

## 本次实跑结果（`./scripts/p18-gate.sh`）

隔离目录：`/Volumes/SSD/Code/Lib/Pawork/target/gates-p18`。结束后已清理（`P18_GATE_KEEP_TARGET` 未设置）。

| 类别 | 命令摘要 | 结果 |
| --- | --- | --- |
| selector-property | `cargo test -p provider-control --lib routing::` + `binding::` | PASS |
| concurrency-recovery | `cargo test -p orchestration --lib` + `app-service --test tenant_policy` | PASS |
| migration-security | `app-database control_plane::` + `tenant-service` + `audit-log` + `usage-ledger --lib` + `app-service --test control_plane_schema` | PASS |
| protocol-golden | Codex `--test golden/handshake/lifecycle/capabilities` + `client-claude-gateway` + `acp-host` + `ide-host-adapter --test contract --test host_mock` | PASS |
| error-fault | `provider-control --test error_matrix` + `quota-service --lib both_endpoints_401` / `both_endpoints_429` | PASS |
| rollback | `app-database rollback` + `provider-control --no-default-features --lib legacy` | PASS |
| clippy-related | 见下方 clippy 集合（`--all-targets --no-deps -- -D warnings`） | PASS |

**Clippy 集合**：`tenant-service` `usage-ledger` `audit-log` `client-adapter-api` `client-codex-app-server` `client-claude-gateway` `acp-host` `orchestration` `ide-host-adapter`。`--no-deps` 避免 path 依赖被 `-D warnings` 误伤。

**Clippy 剔除（既有告警，本任务未改对应 `src/`）**：

- `provider-control`：`binding.rs` `clippy::too_many_arguments`（`commit_rebind` / `rebind_after_release`，8/7）。未给既有代码加 `allow`，未改 binding。
- `app-service`：`claude_gateway.rs` unused imports（`PrincipalId` / `TenantId`）。未 drive-by 修复。

未跑 `cargo fmt --all`（本任务只 fmt 了新增/编辑的测试文件）。

**Schema 一致性**：`crates/app-service/tests/control_plane_schema.rs` 断言 `core_api::CONTROL_PLANE_SCHEMA_VERSION == provider_control::CONTROL_PLANE_SCHEMA_VERSION == app_database::CURRENT_CONTROL_PLANE_SCHEMA_VERSION == 2`。**不**与 `session-store::CURRENT_SCHEMA_VERSION`（9，另一 schema 族）做等式。

## ACP golden 补齐（P17-7 延期落点）

新增 v1 fixture（均有 `tests/fixtures.rs` 加载测试）：

| 消息 | Fixture |
| --- | --- |
| session create | `crates/acp-host/fixtures/v1/session-new-request.json` |
| prompt | `crates/acp-host/fixtures/v1/session-prompt-request.json` |
| tool event | `crates/acp-host/fixtures/v1/session-update-tool-call.json` |
| cancel | `crates/acp-host/fixtures/v1/session-cancel-notification.json` |
| custom model | `session-set-model-request.json` + `error-unknown-set-model.json`（v1 首轮未映射 `session/set_model`，fail-closed `-32601`） |

既有：initialize、session resume、session-update-text、permission selected/cancelled、error-unknown-method（unsupported-field）、v2 initialize 拒绝。未发明第二套 ACP 协议。

## IDE Host Adapter（P17-9 延期落点）

`ide-host-adapter --test contract --test host_mock` 纳入门禁。`contract.rs` 增补：协议名为 `ide-host`（非 GUI / ACP）、帧方法 `ide.*` 而非 `gui.*`、契约子集含 lifecycle / diagnostics / diff / approval。完整回路仍由既有 `host_mock.rs` 覆盖。未改写 adapter。

## 回滚（短节，不另写 runbook 文件）

- Schema：`app-database` `rollback_via_backup_removes_control_plane_tables`（及 lease 同名测试）——`existed=true` 先备份，restore 后控制面表消失、legacy 数据保留。
- Feature-off：`cargo test -p provider-control --no-default-features --lib legacy` —— `account-control-v1` 关闭时仍可取得 ADR-033 synthetic `local/default` + `single_candidate`。
- 关闭 feature 后的运行时回退路径是既有 `provider_control::legacy`，不是一次完整生产 restore 演练。

## 已知 leftovers（本 gate 不假装完成）

- `pawork` Claude stdio CLI / 完整 Messages server（需要 host composition）
- Codex CLI 入口（CoreDispatcher 已在；无 pawork stdio）
- `QuotaRuntime::production()` 尚未注册六家 remote factory、未启动 scheduler
- WebScrape `with_audit_sink` 未被 zhipu factory 注入
- **LeaseRebound 生产发射**：`LeaseRecord::open` 始终设公开 `version=2`，`lease.version > 2` 启发式已从 app-service acquire 删除。隔离测试仍经 `record_control_event` 注入。真实发射等待 app-service 消费 `SessionBindingService` / `BindingAcquisition.old_lease_release`。**不**重新引入 version 启发式。
- 无生产 OTel collector 进程
- 既有 `wait_for_probe_dedups_repeated_poll_of_same_waker` 失败点在 **`model-registry`**（Waker::noop），不在 `provider-control`。本门禁未跑该 crate，故未 filter-skip。

## 主要产出物

- 账号池 property/concurrency/error test suite（既有，由门禁脚本集中调度）
- Codex/Claude/ACP versioned golden matrix（ACP 缺口已补）
- migration/isolation/redaction suite（tenant_policy 复用，不重复）与 rollback 短节
- Phase 18 L2 门禁脚本（完成后清理隔离缓存）

## 验收标准

- [x] 单 Provider + 单 credential、`local/default` 行为保持兼容（migration baseline + feature-off legacy + tenant_policy）
- [x] Secret 不落 SQLite/Event/log；跨 tenant 的 credential/session/agent/usage/audit 均不可观察（control_plane 无明文列 + tenant_policy 隔离/redaction）
- [x] lease/Agent 并发、affinity、cancel、error/fallback 不变量全部通过（orchestration --lib + routing/binding + error_matrix）
- [x] Codex/Claude/ACP 关键协议 golden 与 unsupported-field 行为通过
- [x] feature-off/schema rollback/runtime fallback 演练成功（legacy `--no-default-features` + backup restore；非完整生产 restore runbook）
- [x] 相关 crates 的 test/clippy/schema L2 在独立目录通过并完成清理（clippy 剔除见上；未跑 workspace fmt）
- [x] ACP golden gate（P17-7 延期落点）：initialize / session create/resume / prompt/update / permission / tool event / cancel 与 custom model 每条关键协议消息一个 versioned fixture，unsupported-field 行为可回归
- [x] IDE Host Adapter gate（P17-9 延期落点）：IDE 生命周期映射、诊断双向回灌、apply/diff/approval 回路与可选 LSP 输出的 mock/contract 矩阵通过，断言不经 GUI 协议帧、不构造第二 Core

**相关文档**：[testing](../docs/quality/testing.md) · [security-acceptance](../docs/quality/security-acceptance.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)
