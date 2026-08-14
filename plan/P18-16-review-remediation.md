# P18-16：Phase 18 评审修复（REVIEW remediation）

> Phase 18 · Account Control Plane & Client Adapters · 状态：🔵进行中 · 交付成熟度：Implemented（等待定向 test / clippy / rustfmt / gate 与独立复核） · 依赖：P18-1～P18-15

**最终目的**：按 [Phase 18 Review](../docs/review/p18-review.md) 把「库层已实现」与「正式宿主已接线」重新对齐：补一条真实 account route → credential lease 竖切、让生产库运行 account migration 并回读 metadata、持久化控制面 policy/route audit、修复 picker 热路径阻塞和 P18 Gate 假绿/清理风险，同时把尚未进入宿主的 Provider Factory、Health/Binding/Reconciler、Codex/Claude 与 OTel 明确拆到 P18-17～P18-19，禁止继续以统一绿点声称 Product Ready。

**涉及范围**：`provider-control` credential picker、`app-service` run route/tenant policy、`core-runtime` control-plane migration/hydration、`apps/pawork` durable audit composition、`scripts/p18-gate.sh`；成熟度与延期登记涉及 `ROADMAP.md`、`REVIEW.md`、`plan/P18-*.md`、`plan/README.md`、`docs/review/p18-review.md`。不引入新生产依赖，不扩 `ModelProvider` tenant/account 契约。

## 处置策略

- **本任务直接修复**
  - `CredentialPicker` 改为 async object-safe trait；`RepositoryCredentialPicker` 在 acquire 热路径直接 await repository，删除每次新建 OS thread + Tokio runtime 的 `block_on` 桥。
  - `open_control_plane_runtime` 同时执行 account 与 lease migration，安全复用单次 pre-migration backup；严格解析 SQLite account/credential metadata 并 hydrate 共享 repository，坏行/未知枚举 fail loud。
  - 注入 repository 后，run 在 lease 前执行 `RoutingPolicy` 与 tenant `RouteCandidate` gate；使用持久 account/credential、账号 routing strategy 与 pool 的 tenant-scoped active lease，未知候选 fail-closed。
  - 正式 `pawork` 在构造 CoreRuntime 前先打开 `FileAuditStore`，失败 fail loud；成功后注入共享 policy/route audit sink。删除未被 Provider Factory 消费的 resolver 占位变量。
  - P18 Gate 拒绝零测试假绿、限制隔离 target 清理范围，并把本轮 changed crates 纳入 clippy。
- **明确不在本任务伪闭环**
  - route winner credential 直传、真实 model capability、Health feedback、Session Binding/Reconciler/Quota scheduler → [P18-18](P18-18-runtime-control-loop.md)。
  - `BackendCredentialResolver` → `ProviderFactory` → 真实 Provider 注册、持久管理写回、共享 model catalog → [P18-17](P18-17-production-provider-composition.md)。
  - Codex/Claude 正式入口、完整 durable audit coverage、WebScrape audit 与 OTel collector → [P18-19](P18-19-client-observability-host.md)。

## 细分步骤

1. **事实复核与状态校准** —— 以当前源码、diff、测试与真实 migration 调用为准，纠正 Review 中「account migration 已运行」的旧判断。目的：先修当前缺口，不复述过时结论。
2. **Route → Lease 竖切** —— 持久 metadata hydration、repository picker、RoutingPolicy/tenant gate 与 account-aware lease 串联。目的：正式 run 不再无条件回退 `local/default`。
3. **热路径与启动安全** —— picker 全异步、migration backup 顺序可证、audit store fail loud。目的：避免阻塞/资源放大、备份覆盖和不可取证降级。
4. **Gate 可信度** —— 零命中失败、安全清理边界、changed-crate clippy 与宿主回归。目的：PASS 必须代表真实测试执行。
5. **成熟度与延期任务** —— ROADMAP/plan/review 统一 HostWired / PartialWired / LibraryBuilt / AdapterBuilt / HostSeam / MaintenanceGated，并新增 P18-17～P18-19。目的：历史完成计数与产品成熟度分离。

## 主要产出物

- account/credential SQLite migration + hydration 与 restart 回归
- async repository picker 与 route-before-lease 回归
- durable policy/route audit 正式宿主装配
- 强化后的 `scripts/p18-gate.sh`
- Phase 18 成熟度矩阵、修复记录与三个有界后续任务

## 验收标准

- [ ] account 与 lease migration 均由正式控制面启动路径运行，原库升级最多生成一份不被覆盖的 pre-migration backup
- [ ] 自定义持久 account/credential 重启后被 route/picker/lease 使用；坏行、未知 account、无 Active credential fail-closed
- [ ] picker 热路径无 per-acquire thread/runtime；repository picker 定向测试通过
- [ ] route 在 lease 前执行 tenant policy，使用 account routing strategy 与真实 active lease；策略冲突 fail-closed
- [ ] 正式宿主持久 audit store 在 CoreRuntime 前打开，失败不静默降级；未消费 resolver 占位删除
- [ ] P18 Gate 对零测试、越界 target 清理和 changed-crate clippy fail-closed
- [ ] 定向 tests、changed-crate clippy `-D warnings`、本任务 Rust 文件 rustfmt check、`git diff --check` 与文档链接检查通过
- [ ] 独立 Grok reviewer 无未处置 P0/P1；未闭环项均有 P18-17～P18-19 验收落点
- [ ] ROADMAP / REVIEW / plan / p18-review 同步，P18-16 仅在以上证据齐全后标记 TargetVerified

## 验证记录

等待最终定向门禁与独立复核后回填；Workspace Full Gate 不属于本任务默认验收。

**相关文档**：[Phase 18 Review](../docs/review/p18-review.md) · [ROADMAP Phase 18](../ROADMAP.md) · [plan/README Phase 18 延期登记](README.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [测试体系](../docs/quality/testing.md)
