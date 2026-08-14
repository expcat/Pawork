# P18-15：Control Plane Contract / Security Gate

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢已落地 · 交付成熟度：MaintenanceGated · 依赖：P18-1～P18-14、P17-7、P12-1、P12-2、P12-6

**最终目的**：集中验证账号控制面、Tenant、ClientAdapter 与 AgentSupervisor 的跨 crate 不变量，作为 Phase 18 的 `MaintenanceGated` 收尾；不在每个前置任务重复 workspace 全量门禁。

**涉及范围**：`provider-control` / `tenant-service` / `usage-ledger` / `audit-log` / `client-*` / `acp-host` / `orchestration` / `app-database` / `app-service` / `core-runtime` / `pawork` / `ide-host-adapter` / `quota-service`；隔离目录 `target/gates-p18`（**不**复用其他阶段的 `target/gates`）。

**入口**：[`scripts/p18-gate.sh`](../scripts/p18-gate.sh)。`CARGO_TARGET_DIR` 默认 `target/gates-p18`；`P18_GATE_TARGET_DIR` 只接受 `$ROOT/target/` 下一个绝对、一级子目录，其他值在创建目录与安装清理 trap 前拒绝。`P18_GATE_KEEP_TARGET=1` 时保留隔离缓存；否则 `EXIT` trap 在成功、失败、INT、TERM 后仅清理校验过的目标与临时日志。全程不跑 `cargo test --workspace` / `clippy --workspace`。

## 细分步骤

1. **Selector/property gate** —— priority、weighted distribution、fill-first、affinity/rebind、fallback explanation；目的：路由可证明。
2. **Concurrency/recovery gate** —— lease/Agent 并发上限、cancel/drop/crash/reclaim、hot reload；目的：无泄漏和超配。
3. **Migration/security gate** —— legacy default migration、Secret 扫描、cross-tenant credential/session/agent/usage/audit chaos；目的：安全边界。
4. **Protocol golden gate** —— Codex Thread/Turn/Item/approval/subagent（并区分 remote compaction 与 local compaction 两类 fixture）、Claude Messages/identity/reasoning、ACP initialize/session create/resume/prompt/update/permission/tool event/cancel 与 custom model；目的：客户端版本回归可见，每条重要协议消息一个 fixture。
5. **错误/故障注入 gate** —— 401/402/provider-specific 400/429/QuotaExceeded（hard/soft）/5xx/cancel/context/protocol/stream interruption；目的：失败域不串味。
6. **回滚演练与 L2** —— 关闭 feature flags、回退 synthetic account/SingleCandidate、schema forward/rollback/restore，在独立构建目录跑相关 test/clippy；目的：可发布、可撤回。
7. **Host Route → Lease gate** —— repository picker、双 migration/hydration、run route-before-lease、tenant route audit 与缺 Provider fail-closed；目的：门禁覆盖正式宿主的第一条账号竖切，而不只覆盖库层算法。
8. **Gate 可信度与 Schema 一致性** —— 任一测试命令 0 passed 即失败；只清理受限隔离路径；changed crates 纳入 Clippy；跨 crate 断言 control-plane schema version 完全一致。目的：防止空过滤、越界清理与单点 schema bump 造成假绿。

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
| host-route-lease | repository picker + Core migration/hydration + `credential_lease` + route audit + `pawork` 缺 Provider | PASS |
| clippy-related | 见下方 clippy 集合（`--all-targets --no-deps -- -D warnings`） | PASS |

**Clippy 集合**：第一组覆盖本轮 changed crates：`provider-control` `app-service` `core-runtime` `pawork`，使用 `-D warnings -A clippy::too_many_arguments`，唯一豁免是 `provider-control/binding.rs` 的既有 8/7 参数 API；第二组严格覆盖 `tenant-service` `usage-ledger` `audit-log` `client-adapter-api` `client-codex-app-server` `client-claude-gateway` `acp-host` `orchestration` `ide-host-adapter`。两组均为 `--all-targets --no-deps`，无整 crate 静默剔除。

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

- 持久 account/credential 管理写回、resolver/factory、真实 Provider 注册与共享 model catalog → [P18-17](P18-17-production-provider-composition.md)。
- 真实 capability/Health、route winner credential 单次透传、Session Binding/`LeaseRebound`、Reconciler/Probe/Quota scheduler → [P18-18](P18-18-runtime-control-loop.md)。**不**重新引入 lease version 启发式。
- Codex/Claude `pawork` 入口、完整 durable audit、WebScrape audit sink 与生产 OTel 生命周期 → [P18-19](P18-19-client-observability-host.md)。
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
- [x] 相关 crates 的 test/clippy/schema 与 Host Route → Lease 回归在独立目录通过并完成清理（未跑 workspace fmt）
- [x] ACP golden gate（P17-7 延期落点）：initialize / session create/resume / prompt/update / permission / tool event / cancel 与 custom model 每条关键协议消息一个 versioned fixture，unsupported-field 行为可回归
- [x] IDE Host Adapter gate（P17-9 延期落点）：IDE 生命周期映射、诊断双向回灌、apply/diff/approval 回路与可选 LSP 输出的 mock/contract 矩阵通过，断言不经 GUI 协议帧、不构造第二 Core

**相关文档**：[testing](../docs/quality/testing.md) · [security-acceptance](../docs/quality/security-acceptance.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)
