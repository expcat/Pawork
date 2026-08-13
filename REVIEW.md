# REVIEW — Pawork 评审记录总目录

- **性质**：本文件是各 Phase 评审的**目录索引**与**修复复核状态**记录。各 Phase 的逐任务证据、行号级证据、漏洞清单（V1–Vn）、优化建议，以及对应的**修复记录**（细分步骤、产出物、验收标准、验证记录）均归档于独立文档 `docs/review/pN-review.md` 的「修复记录（review-remediation）」章节。
- **评审日期**：2026-08-08（P1–P7 整合评审）；2026-08-09（P8–P11 评审）。
**复核日期**：2026-08-09（P1–P7 修复复核，详见 [§复核结论](#复核结论2026-08-09)）；2026-08-10（P8、P9、P10 修复复核）。
**P11 复核**（2026-08-10）：见下方整合说明。
**P12 复核**（2026-08-10）· **P13 评审**（2026-08-10）· **P13 修复**（2026-08-11）：见 [P13-11](plan/P13-11-review-remediation.md)。
**P14 评审**（2026-08-11）· **P14 修复**（2026-08-11）：见 [P14-10](plan/P14-10-review-remediation.md)。
**P15 评审**（2026-08-12）· **P15 修复**（2026-08-12）：见 [P15-10](plan/P15-10-review-remediation.md)。
**P16 评审**（2026-08-12）· **P16 修复**（2026-08-12）：见 [P16-10](plan/P16-10-review-remediation.md)。
**P17 评审**（2026-08-13）· **P17 修复**（2026-08-13）：见 [P17-14](plan/P17-14-review-remediation.md)。
- **评审基线**：P1 以 `de76839` 为基线；P2–P7 以 `67d6c4d` 为基线；P8–P11 以各自提交为基线。
- **V 编号约定**：各阶段内部独立使用 V1–Vn 编号；跨阶段引用以 `P<阶段>-V<n>` 前缀区分。
- **引用兼容**：`plan/P{1..7}-*-review-remediation.md` 中的 `[REVIEW.md](REVIEW.md) §N` 链接仍指向本文件，各 §N 对应 `docs/review/pN-review.md`。

## 评审记录目录

| Phase | 主题 | 评审文档 | 修复任务 | 复核状态 |
| --- | --- | --- | --- | --- |
| P1 | 配置系统、SQLite Actor、诊断与 CLI 骨架 | [p1-review.md](docs/review/p1-review.md) | [P1-13](plan/P1-13-review-remediation.md) 🟢 | ✅ V1–V8 + 基线全部确认修复 |
| P2 | Provider 运行时、OpenAI-compatible 适配、认证与模型目录 | [p2-review.md](docs/review/p2-review.md) | [P2-12](plan/P2-12-review-remediation.md) 🟢 | ✅ V1–V10 + 基线全部确认修复 |
| P3 | Agent Loop 主干 | [p3-review.md](docs/review/p3-review.md) | [P3-11](plan/P3-11-review-remediation.md) 🟢 | ✅ V1–V11 + 基线全部确认修复 |
| P4 | 核心工具与权限 | [p4-review.md](docs/review/p4-review.md) | [P4-13](plan/P4-13-review-remediation.md) 🟢 | ✅ V1–V14 + 基线 + fuzz 全部确认修复 |
| P5 | Session 树、Compaction 与上下文裁剪 | [p5-review.md](docs/review/p5-review.md) | [P5-10](plan/P5-10-review-remediation.md) 🟢 | ✅ V1–V10 + 文档全部确认修复（基线由跨阶段任务收口） |
| P6 | OpenAI / Anthropic / Google 三家 Provider 适配 | [p6-review.md](docs/review/p6-review.md) | [P6-14](plan/P6-14-review-remediation.md) 🟢 | ✅ V1–V8 + 基线全部确认修复 |
| P7 | Git、Diff 与 Worktree | [p7-review.md](docs/review/p7-review.md) | [P7-9](plan/P7-9-review-remediation.md) 🟢 | ✅ V1–V10 + 基线全部确认修复 |
| P8 | Skills、Prompts 与 Instructions（resource-loader） | [p8-review.md](docs/review/p8-review.md) | [P8-9](plan/P8-9-review-remediation.md) 🟢 | ✅ §3.3/§3.1/§3.5/§4.1/§3.2 全部确认修复（§2/§3.4/§4.2/§4.3 显式延后 P13） |
| P9 | MCP（mcp-client） | [p9-review.md](docs/review/p9-review.md) | [P9-8](plan/P9-8-mcp-review-remediation.md) 🟢 | ✅ §3.6/§3.3/§3.2/§3.5/§3.4/§3.8/§3.1 全部确认修复（§4.1/§4.3/§4.4/§3.7/§4.2 显式延后） |
| P10 | WASM Plugin（plugin-api / wasm-plugin-host / hook-runtime） | [p10-review.md](docs/review/p10-review.md) | [P10-7](plan/P10-7-review-remediation.md) 🟢 | ✅ §3.1/§3.4/§3.5/§3.6 死 API 删除、§4.1 Lifecycle 双路径合并、§3.2/§3.3 重复消除、§4.3a 超时约束、§3.9/§4.4 文档全部确认修复（§2/§3.7/§3.8/§4.3b,c/§4.5 显式延后） |
| P11 | Sandbox 与跨平台强化 | [p11-review.md](docs/review/p11-review.md) | [P11-9](plan/P11-9-review-remediation.md) 🟢 | ✅ §2.1/§2.2/§2.3/§2.4/§2.5/§2.6/§3.1/§3.2/§3.3 全部确认修复（§2.6 NetworkMode 合并、§3.4 文件拆分显式延后） |
| P12 | Multi-Agent 编排 | [p12-review.md](docs/review/p12-review.md) | [P12-7](plan/P12-7-review-remediation.md) 🟢 | ✅ §2.1 接线三件套 / §2.2 删 AgentConcurrency / §2.3 11 死事件生产者 / §2.4 ledger 归属+预算数据源 / §2.5 删死依赖 / §2.6 删 parent_id+AgentTree 文档 / §2.7 Drop 反模式 / §3.1 P12-1 措辞 全部确认修复（§3.1 agent-loop 接线、§3.2 主流程接入、§3.3 event store 持久化显式延后） |
| P13 | CLI Host 与多 GUI 协议 | [p13-review.md](docs/review/p13-review.md) | [P13-11](plan/P13-11-review-remediation.md) 🟢 | ✅ §2.1 宿主装配 GuiServer / §3.5 双向协议版本校验 / §3.7 删 aggregate+connection-manager+client-auth 死公开 API / §3.8 client-auth 原子创建 全部确认修复（§2.2/§2.3/§3.1/§3.2/§3.4/§3.6/§4.1/§4.6 显式延后） |
| P14 | Usage / Quota（quota-service 收口） | [p14-review.md](docs/review/p14-review.md) | [P14-10](plan/P14-10-review-remediation.md) 🟢 | ✅ §3.1/§3.2 删 capability+OAuth 通用层 / §3.3/§3.5 时间与脱敏单一事实源 / §3.4 错误归并单一实现 / §3.6 cache 压平+失败归属可空 / §2.6 告警跨边界简化 / §2.4 显式 provider / §3.8 CLI typed 解析渲染 / §6 P0 状态语义 全部确认修复（§2.1/§2.2/§2.3/§2.5/§3.7 显式延后 P18-2/3/4/8/13/14、P19-2/10） |
| P15 | Provider Native Capabilities（三家现代传输收口） | [p15-review.md](docs/review/p15-review.md) | [P15-10](plan/P15-10-review-remediation.md) 🟢 | ✅ §3.2 统一 ReasoningProtector+typed ref 三家迁移 / §3.1 删 ExecutionOwner 保留 ContinuationMode / §2.2 tool-search 默认关闭 feature 门 / §3.6 删 blob rotate/integrity/disk_usage / §3.8 稳定 capability_key 全部确认修复（§3.3「重写一遍」、§2「ServerToolEvent 仅 fixture」、§2.3「Diagnostic 无消费者」按源码证据纠正；assembler 保留 adapter-local；§2.4 宿主装配、§3.7 provider catalog、§2.1 持久化 protector 显式延后 P18-3/4/14） |
| P16 | Modern Agent Workflow | [p16-review.md](docs/review/p16-review.md) | [P16-10](plan/P16-10-review-remediation.md) 🟢 | ✅ §3.1 正式链编译闭包 / §3.2 P16-9 原子导入 / §3.3 ID session-scope / §3.4 validate_structure 名实相符 / §3.5 Goal/Memory/Review 重放字段补齐 + Automation fired_count 单一源与任务归属校验 + Monitor 重复注册与 start 顺序修正 / §2.3/§4.3 删 TaskManagerDispatcher+ExternalTrigger+FileWatchDriver / §4.5 删 pseudo-anchor 全部确认修复（Automation 完整 config/schedule/failure/inbox replay 与 Monitor config 入 state/task mapping/replay/lifecycle 未达；生产宿主接线按 [Phase 16 延期落点登记](plan/README.md) 六项映射：monitor 包驱动 → P17-2/P17-3、Plan/Goal host → P19-12、workflow core-api/EventHub → P17-6、Memory provider/SQLite/context → P17-5/P19-2、Review Forge/UI → P19-8、compat 命令入口 → P17-8/P19-2；ROADMAP/plan 已同步：Phase 16 10/10、评审当时总计 219/188） |

| P17 | Ecosystem & Host Compatibility | [p17-review.md](docs/review/p17-review.md) | [P17-14](plan/P17-14-review-remediation.md) 🟢 | ✅ §3.1 Remote 长驻生命周期 + token drop 清理 + 跨进程 connect/reconnect（独立 unpublish/revoke 无共享控制面 fail-closed，外部可达/共享控制面显式延后 P19-14）/ §3.2 假成功 fail-closed（Placeholder/PluginList/McpList/隔离 no-op 工具）/ §4.2 Profile 引用 fail-closed / §4.3 Teams 降 durable library（启动不建 teams.sqlite）/ §5 contract 归 transport-api + Browser 别名删除 + Compat export_plan / JSON stdout 契约（日志 stderr、协议帧 stdout）全部确认修复；另门禁衍生可靠性修复两项（RateLimiter `enqueue` 自动冲刷不丢不重、remote 认证同步 subscribe + carrier 顺序无关/压力 0 失败）（矩阵 HostWired P17-1/7/8 · PartialWired P17-5/11 · LibraryBuilt P17-2/3/4/6/10/13 · AdapterBuilt P17-9/12；Phase 17 14/14、总计 220/189；ACP 降级审计 → P18-13、再认定与功能簇门禁 → P18-15、Marketplace/Plugin/Profile 消费 → P19-11、Teams ingress → P19-13、远程外部可达/pairing → P19-14 显式延后） |

## 复核结论（2026-08-09）

## 复核结论（2026-08-11 · P12 + P13）

对 P12 / P13 评审发现的问题逐项核对修复落地情况。复核方法：Commander（GLM）核对 review 仍成立性 + 派发 4 个只读 `deepseek_explorer` / 5 个写集互不重叠的 `deepseek_worker` 并行执行 + 1 个 `deepseek_reviewer` 独立复核 + Commander 后处理（reviewer finding #1 `gui_clients` 死字段清理）。

**总体结论**：P13 评审中可立即落地的 4 项（§2.1 / §3.5 / §3.7 / §3.8）全部修复，workspace 全量 `cargo test`（1155 passed / 0 failed）、clippy、fmt、schema-typegen `--check` 均干净；§4.1 transport 去重经核验确认需扩大 transport-memory 公开面（与减概念冲突）显式延后，其余结构性接线项（§2.2/§2.3/§3.1/§3.2/§3.4/§3.6/§4.6）按后续阶段配套延后并登记。详见 [P13-11](plan/P13-11-review-remediation.md)。

对 P1–P7 评审发现的全部漏洞（V 项）、基线偏差与文档漂移项，在当前源码逐项核对修复落地情况。复核方法：deepseek reviewer 逐项比对 `crates/` 源码与根 `Cargo.toml`，给出 `文件:行` 或 `rg` 命中证据；主代理汇总。

**总体结论**：P1–P7 评审发现的全部问题（V1–Vn 共 75 项 + 基线偏差 + fuzz 缺口 + 文档漂移）均已在当前源码真实落地修复，且每项配有针对性回归测试（断言级）。`plan/P{1..7}-*-review-remediation.md` 的 🟢 TargetVerified 标记与源码事实一致。无「证据不足（⚠️）」或「未修复（❌）」项。

| Phase | 复核项数 | 已修复 | 关键代表证据 |
| --- | --- | --- | --- |
| P1 | V1–V8 + 5 基线项 | 全部 | V2 `event_store.rs:297` redaction + 7-secret 契约测试；V1 `loader.rs:220` 剥离 workspace 层 trust_workspaces |
| P2 | V1–V10 + 2 基线项 | 全部 | V1 `http.rs:103` connect+read_timeout；V3 `request.rs:15` 无条件 include_usage |
| P3 | V1–V11 + 基线 | 全部 | V9 `provider_loop.rs:827` SchedulerLoopContext 桥接真实 ToolScheduler 端到端测试；V4 `recovery.rs:67` 工具轮次重放无非法迁移 |
| P4 | V1–V14 + 基线 + fuzz | 全部 | V1 `scheduler.rs:241` 生产路径调用 policy.decide()；V9 checkpoint 版本化状态文件持久化+崩溃重建 |
| P5 | V1–V10 + 文档 | 全部 | V1 `engine.rs:106` events_by_branch 多分支压缩；V6 `token.rs:188` CJK 1字符/token 分流估算 |
| P6 | V1–V8 + 基线 | 全部 | V1 `provider.rs:100` Google key 改 x-goog-api-key 头；V4 `oauth.rs:657` auto-refresh + 轮换回写 |
| P7 | V1–V10 + 4 基线项 | 全部 | V1 `stage.rs:163` NamedTempFile 独占创建；V2 `process.rs:44` validate_position_arg 拒绝前导 `-` |

> 注：P5 的基线偏差项（uuid/tracing-appender/similar、parking_lot/tempfile、base64/rand/sha2/url）按计划分工由 P1-13/P6-14/P7-9 跨阶段统一清理，已在 P1/P6/P7 复核中确认 ✅，故 P5-10 范围内不重复处理，符合评审 §0.4 的「一次性基线清理」建议。

## 复核结论（2026-08-11 · P14）

对 P14 评审发现的问题逐项核对修复落地情况。复核方法：Commander 核对 review 仍成立性 + 按实际 diff 定向核对 `quota-service` / `app-service` / `core-api` / `cli-command` / `cli-host` / `cli-renderer` / `gui-protocol` / `protocol-test-gui` 写集落地 + `rg` 残留与 diff 写集合确定性核验 + `gui-server` forwarder 协议竞态（Hub receiver 注册时序）修复核验；独立 `deepseek_reviewer` 复核结论 VERDICT: PASS，其 4 类 findings 已全部处理。

**总体结论**：P14 评审中可立即落地的复杂度与契约项全部修复——§3.1/§3.2 删 capability matrix 与 OAuth 通用层（净删约 1000 行）、§3.3/§3.5 时间与脱敏收敛 `util.rs` 单一事实源、§3.4 双端点错误归并收敛 `error.rs` 单一优先级表、§3.6 cache 压平与失败归属可空化、§2.6 告警跨边界模型简化（删 `Alert.suggestions`，补冻结 `QuotaAlertKind` + 脱敏 `source` 透传，schema 重生成）、§2.4 `QuotaOverview` 显式 provider 必填、§3.8 CLI typed 解析/渲染、§6 P0 状态语义标注（Phase 14「库级收口、生产接线待 P18」，遵循既有 L0/L1 规则）。验证：9 crate 联合 `cargo test`（452 passed / 0 failed）、`protocol-test-gui --self-test` 协议竞态修复后连续 5 次 9/9 且最终再 1 次 9/9（含新 quota-alert-roundtrip）、9 成员 clippy（0 warning）、fmt / schema-typegen / `git diff --check` 均 PASS。`gui-server` forwarder 的 Hub receiver 改为 spawn 前同步注册（broadcast 不补历史，回归测试确认），竞态消除。独立 `deepseek_reviewer` 复核 VERDICT: PASS，其 4 类 findings 均已处理。§2.1 持久化 Ledger 与真实 attribution、§2.2 生产 refresh/audit 生命周期、§2.3/§2.5 GUI 实际投影、§3.7 WebScrape 审计 Vec 合并按评审结论显式延后至 P18-2/3/4/8/13/14 与 P19-2/10 并登记（deferred 保持）。详见 [P14-10](plan/P14-10-review-remediation.md) 与 [p14-review.md](docs/review/p14-review.md) §7.5。

## 复核结论（2026-08-12 · P15）

对 P15 评审发现的问题逐项核对修复落地情况。复核方法：Commander 核对 review 仍成立性 + 按实际 diff 定向核对 `provider-runtime` / 三家 provider / `agent-domain` / `tool-api` / `tool-runtime` / `protected-blob-store` 写集落地 + `rg` 残留与 diff 写集合确定性核验 + 独立 `deepseek_reviewer` 只读复核（VERDICT: PASS；两项低影响证据口径已校正）。

**总体结论**：P15 评审中可立即落地的复杂度项全部修复——§3.2 三家 reasoning 保护抽象统一为 `provider-runtime::reasoning::ReasoningProtector` 一份 trait（typed `ProtectedBlobRef`，`ProtectedBlobStoreProtector` 标准实现，删 `ReasoningStateBridge` ref-count 生命周期与三家本地拷贝，openai/xai/anthropic 全部迁移 `with_reasoning_protector`；OpenAI/xAI 默认 `InMemoryReasoningProtector` 提升为 provider 实例级 `Arc`，同实例跨 `stream` 回灌回归通过）、§3.1 删与 `ToolKind` 1:1 冗余的 `ExecutionOwner`（保留真实消费者 `ContinuationMode`，rg 零生产命中）、§2.2 tool_search 收口到默认关闭的 `tool-search` feature（feature 开启时 27 passed 全保留）、§3.6 protected-blob-store 删公开 rotate/RotateReport、integrity_check/IntegrityReport、disk_usage 与专属 all_rows/测试（保留 AEAD seal/open + scope + ref + key version，以及 retention、disk_budget 内部约束、crash reconcile、refcount/gc/shutdown）、§3.8 协商与 `AcceptedResponsesTools` 改经稳定 `capability_key` 类型化匹配（去 `Debug` 格式反解）。三条评审事实按源码证据纠正：§3.3「modern.rs 重写一遍」不成立（modern 复用 request helpers，现代/基线共同进入 `provider.rs::pump_messages` 并共享 auth/SSE/usage 归一）、§2「ServerToolEvent 仅 fixture 触发」不成立（三家均有真实 wire producer：OpenAI `responses.rs` Citation/Source/Program/Computer/Started/Completed、Anthropic `stream.rs` Citation/Source/Started/Completed、xAI `responses.rs` Citation/Source/Program/Started/Completed/Failed）、§2.3「Diagnostic 只 emit 无消费者」不成立（`provider_capability_negotiated` 有通用观测消费与断言）。`ResponsesStreamAssembler` 判定保留 adapter-local（openai/xai hosted tool 子集差异真实存在，下沉收益小于差异风险）。L2 定向功能簇验证：8 crate 联合 `cargo test`（18 targets / 364 passed）、tool-runtime `--features tool-search`（1 target / 27 passed）、`scripts/p15-gate.sh`（13 targets / 48 passed）；三条测试命令累计 32 target executions / 439 test executions / 0 failed（feature 与 gate 含有意复跑，非去重用例数）；8 成员及两项 feature clippy 0 warning；21 个本任务 Rust 文件 rustfmt check、`git diff --check` 与残留 rg 均 PASS；专用 gate 四类 PASS 且 `target/gates` 清理。Full workspace gate NOT RUN（8 个 `-p` + 专用 gate 已充分覆盖）。状态语义：P15-1/5/9 plain TargetVerified，P15-2/3/4/7/8 有界 TargetVerified（host composition deferred），P15-6 有界 TargetVerified（default-off/no consumer），不声称生产已装配；计数 P15 10/10、总计 218/165（逐 Phase 行机械求和；P15-10 前旧合计 217/167、旧 Phase 行实际 217/164，既有完成数漂移 +3）。§2.4 宿主装配真实 Provider（P18-3/4）、§3.7 provider catalog 统一（P18-14）、§2.1 持久化 protector 生产接线（P18-3/4/14）按评审结论显式延后并登记为各 P18 计划验收项。详见 [P15-10](plan/P15-10-review-remediation.md) 与 [p15-review.md](docs/review/p15-review.md) 修复记录章节。

## 复核结论（2026-08-12 · P16）

对 P16 评审发现的问题逐项核对修复落地情况。复核方法：Commander 核对 review 仍成立性 + 按实际 diff 定向核对 `agent-engine` / `app-service` / `session-store` / `agent-domain` / `goal-service` / `memory-service` / `review-engine` / `automation-service` / `monitor-service` / `task-manager` 写集落地 + 正式链复跑 + `rg` 残留与 diff 写集合确定性核验 + 独立 `deepseek_reviewer` 复核（VERDICT: PASS；唯一低严重度 `docs/features/workflow.md` 注解已校正，无代码 finding）。

**总体结论**：P16 评审的四组 P0 阻断项全部处置——① 正式链编译闭包：`agent-engine::recovery` 与 `app-service::supervisor` 对 7 个 Phase 16 `AgentEvent` 变体显式折叠（穷举 `match`、无通配 `_`），`cargo check -p app-service` 恢复编译，两条 `workflow_events_*` 回归通过并纳入 `scripts/p16-gate.sh` official-chain 类别；② P16-9 原子导入与 ID scope：单 SQLite transaction（`TransactionBehavior::Immediate`）一次写入 Session + branch + `compat_import_identity` + 全部事件 + projection，任一失败整体回滚零残留，run/message/tool ID 全部 session-scoped，新增并发导入、连续两会话与跨来源重复 tool ID 回归；③ `validate_batch` 改名 `validate_structure` 并纠正「replay 校验」过度声明（结构校验 + `ToolCallArgumentsDelta` 引用检查）；④ 重放状态补齐（有界）：Goal `CriterionSatisfied` 事件化、Memory `Recorded` 携带 embedding/confidence、Review `FindingOpened` 携带富字段（`SuggestedPatch` 入 canonical domain）三处完整补齐；Automation `fired_count` 单一事实源 + `TaskNotTriggeredByAutomation` 归属校验、Monitor 配置锁内重复注册拒绝 + task start 先于 `Started` 广播（消除分叉）为有界修复。Automation 完整 config/schedule/failure/inbox replay 与 Monitor config 入 state / task mapping / 完整 replay 未达（`Registered` 仍只有 trigger kind、`Started` 仍只有 source/workspace，config 未入 state），随 §2.1/§2.3 延后项登记跟踪（见 plan/README 六项）。同时删除假执行/无消费者路径：`TaskManagerDispatcher`（不再伪造 register→start 即执行）、`ExternalTrigger` 五 variant、`FileWatchDriver`、`parse_diff_anchors_owned` pseudo-anchor（raw diff 原样保留，锚点化交未来 Review consumer）。验证：`scripts/p16-gate.sh` 全类别 PASS（11 crate 联合 test 225 + 2 条 workflow_events 回归 = **227 tests / 0 failed**、11 crate clippy `--all-targets -D warnings` 0 warning、`cargo check -p app-service`、schema-typegen `--check`），隔离 `target/gates` 已清理；31 个现存改动 Rust 文件 rustfmt `--check`；另 2 个 Rust 文件为删除项，不适用 rustfmt；`git diff --check` 均 PASS。Full workspace gate NOT RUN（11 个 `-p` + 专用 gate 已覆盖 P16 写集与正式链，未命中升级条件）。最终独立 `deepseek_reviewer` 复核 **VERDICT: PASS**：独立复跑 `scripts/p16-gate.sh` 全类别 PASS（225 + 2 = **227 tests / 0 failed**、11 crate clippy `--all-targets -D warnings` 0 warning、`cargo check -p app-service`、schema-typegen `--check`、rustfmt `--check`、`git diff --check` 均 PASS），唯一低严重度 `docs/features/workflow.md` 注解已校正，无代码 finding。状态语义：P16-1～P16-9 全部有界 TargetVerified（domain + services verified、host composition deferred；P16-5/6 为有界修复，完整重放未达），P16-10 review-remediation 🟢 完成，计数 P16 10/10（P16-1～P16-9 + P16-10；与 Phase 16 行 10/10 有界完成一致）；生产宿主接线按 [Phase 16 延期落点登记](plan/README.md) 六项映射：monitor 包驱动与 §4.2 lifecycle → P17-2/P17-3、§2.1/§2.2 Plan/Goal host 与 Plan approval gate/Goal steering → P19-12、workflow core-api/EventHub → P17-6、§2.4 Memory 生产化 → P17-5/P19-2、真实 Forge adapter → P19-8、Compat 导入 CLI 入口与历史查询 → P17-8/P19-2；Automation 完整 replay 与 Monitor config 入 state 未达项随上述登记同步跟踪，均按评审结论显式延后并登记为对应计划验收项；ROADMAP/plan 已同步（Phase 16 10/10、评审当时总计 219/188）。详见 [P16-10](plan/P16-10-review-remediation.md) 与 [p16-review.md](docs/review/p16-review.md) §11 修复记录章节。

## 复核结论（2026-08-13 · P17）

对 P17 评审发现的问题逐项核对修复落地情况。复核方法：Commander 核对 review 仍成立性 + 按实际 diff 定向核对 `pawork` / `app-service` / `cli-host` / `transport-api` / `transport-remote` / `transport-remote-placeholder` / `compat-loader` / `browser-computer-runtime` / `remote-control-adapter` 写集落地 + 定向门禁复跑 + `rg` 残留与 diff 写集合确定性核验。

**总体结论**：P17 评审的两组 P0 与状态失真项全部处置——① §3.1 P17-11 remote 生命周期：由执行 remote publish 的长驻 pawork 进程持有，publish 长驻至 SIGINT，`EndpointState::drop` 幂等删除自建 token 文件，跨真实进程 publish → connect → reconnect → SIGINT → token 清理与同名再发布 e2e（`apps/pawork/tests/remote.rs`）；独立 unpublish/revoke 无共享控制面时 fail-closed（跨进程操控运行中 publish 进程、外部可达地址/relay 与共享控制面显式延后 P19-14）；② §3.2 假成功消除：Placeholder 命令 / PluginList / McpList 返回 Unavailable、隔离 profile 下 no-op 工具返回失败 ToolResult，CLI 非零退出 + `--json` `ok=false`。状态失真收敛：§4.2 Profile skills/mcp/permissions/hooks 任一非空 → run 解析 fail-closed；§4.3 `team_db_path` 默认 None、正常启动不创建 teams.sqlite，Teams 降 durable library。冗余删除：§5 remote provider/connector 契约收回 `transport-api`（placeholder 收缩为短期 re-export 兼容层 + Mock）；Browser 纯别名 helper 删除；Compat `apply` → `export_plan`（只写计划不应用资源，幂等指纹）。JSON stdout 契约固化：日志一律 stderr，`--json`/ACP/Headless stdout 可整体解析为纯 JSON。门禁衍生可靠性修复两项：RateLimiter 新增 `enqueue`，窗口到期 / 容量触发的自动冲刷结果重排入就绪队列、由下一次 `flush` 发出（不丢不重、队列与合并缓冲同界）；remote-control-adapter 首次认证成功后同步 `subscribe()` 再 spawn 通知泵（消除认证成功与订阅之间的 spawn 调度窗口，认证后 Core 发布的事件不再错过），transport-remote carrier e2e 改响应/通知联合捕获、到达顺序无关（压力复跑 0 失败）。验证：9 crate 联合 `cargo test`（38 summaries、**371 passed / 0 failed / 0 ignored**，含 remote/teams_state/cli 回归）、同 9 crate clippy `--all-targets -D warnings` 0 warnings、26 个 Rust 文件 `rustfmt --edition 2021 --check --config skip_children=true`、`git diff --check` 均 PASS；文档链接检查：git diff/未跟踪共 27 个变更 Markdown、733 条本地相对链接、0 broken。状态语义：P17-14 `🟢已完成 · TargetVerified`（有界：八项 review 修复 + 两项门禁衍生可靠性修复 + 定向门禁）；矩阵 HostWired P17-1/7/8 · PartialWired P17-5/11 · LibraryBuilt P17-2/3/4/6/10/13 · AdapterBuilt P17-9/12；计数 Phase 17 14/14、总计 220/189。延期按五项映射显式登记：ACP 降级审计 → P18-13、host-wired 再认定与功能簇门禁 → P18-15、Marketplace/Plugin 纵向 + Profile 引用消费 → P19-11、Teams canonical ingress → P19-13、远程外部可达/pairing → P19-14。详见 [P17-14](plan/P17-14-review-remediation.md) 与 [p17-review.md](docs/review/p17-review.md) §11 修复记录章节。

## 目录（评审当时结构）

- 跨阶段总览（评审当时整合视角）：门禁完成度 · 主干未通电 · 安全索引 · 基线偏差 · plan 同步 · 测试可信度
- Phase 1–11 各阶段评审详见 `docs/review/pN-review.md`
- 整合说明

## 0. 跨阶段总览

本节为 P1–P7 整合评审（2026-08-08）当时的跨阶段汇总视角，原样保留以记录评审快照；各阶段的逐任务证据与行号级证据见 `docs/review/pN-review.md`。

### 0.1 门禁与完成度总览

| Phase | 主要交付 crate | 测试（2026-08-08 复跑） | 静态门禁 | plan 同步 |
| --- | --- | --- | --- | --- |
| P1 配置/数据层 | config-service、app-database、session-store、artifact-store、workspace-service、file-index、diagnostics、cli-host(+命令/渲染)、apps/pawork | **80 passed / 0 failed** | clippy/fmt/schema-typegen 干净 | ✅ 全部勾选 |
| P2 Provider 运行时 | provider-runtime、provider-openai-compatible、auth-service、model-registry、test-support | **120 passed / 0 failed** | 干净 | ❌ 11 篇全 🟡未开始，19 框未勾 |
| P3 Agent Loop | agent-engine、context-engine、tool-runtime、agent-events | **89 passed / 0 failed** | 干净 | ❌ 10 篇全 🟡未开始，18 框未勾 |
| P4 工具/权限 | builtin-tools、policy-engine、checkpoint-service、process-runtime | **99 passed / 0 failed** | 干净 | ✅ 12 篇全勾（纠正 P2/P3 偏差） |
| P5 Session/Compaction | session-store（复用）、compaction-engine、context-engine（复用） | **63 passed / 0 failed**（3 crate） | 干净 | 多数已勾 |
| P6 三家 Provider | provider-openai、provider-anthropic、provider-google、provider-api、auth-service(+)、model-registry(+) | Phase-6 自有 94 / 含共享层 187 passed | 干净 | 各 plan 已勾 |
| P7 Git/Diff | git-service、diff-service | **72 passed / 0 failed** | 干净 | ✅ 已勾选 |

> 说明：P5/P6 与早期阶段共享 crate（如 session-store、context-engine、auth-service、model-registry），测试计数不可简单相加；各阶段复跑时仅统计其直接交付/复用 crate。

### 0.2 系统性问题：组件齐全、主干未通电

跨 P2–P6 反复出现同一模式：模块实现质量高、单测充分，但未接入主干循环，「测试绿」不等于「系统可用」。这是本轮评审最重要的系统性发现，建议作为 Phase 13 CLI Host 装配的前置/并行任务集中收口。

| 组件 / 能力 | 阶段 | 现状 | 详见 |
| --- | --- | --- | --- |
| `PolicyEngine::decide()` | P4 | 全仓库零生产调用，仅 13 处自测 | P4 §2 / V1 |
| `allowed_in_untrusted_workspace` | P4 | 全仓库零强制点 | P4 V1 |
| tool-runtime 调度器策略 | P4 | 仅用 `require_approval_for_writes` 布尔替代整套策略引擎 | P4 §2 |
| ToolScheduler ↔ ProviderLoop 桥接 | P3 | 不存在，两套独立实现从未组合 | P3 V9 |
| MessageQueue / RetryController / CancelHandle | P3 | ProviderLoop 零引用（用裸 CancellationToken） | P3 V3 / V7 |
| LoopSink 流式 delta 广播 | P3 | 整轮缓冲、从不广播 token 流 | P3 V2 |
| 多维预算（cost/duration/concurrency/artifact） | P3 | loop 中 4 维零记录，soft_warnings 不发事件 | P3 V5 / V6 |
| compaction-engine | P5 | 全 workspace 零消费者 | P5 §1-5 |
| context-engine / `trim_tool_result` | P5 | ContextBuilder 未调用 | P5 §1-5 |
| OAuth auto-refresh | P6 | `needs_refresh`/`refresh_access_token` 零消费者，轮换 token 不回写 | P6 V4 |
| `trust_workspaces` | P1 | 未消费（一旦接线存在自我提权面） | P1 V1 |

### 0.3 跨阶段「安全/红线」问题索引

下表汇总各阶段涉及安全与架构红线的项（Agent 红线「Secret 不落库」、「信任闸门」、「不可信输入执行」等），建议优先于「通电」之前处理。

| 编号 | 主题 | 阶段 | 类型 |
| --- | --- | --- | --- |
| P1-V1 | `trust_workspaces` 自我提权攻击面（未消费） | P1 | 安全·高 |
| P1-V2 | Event Store 持久化整个信封，Secret 可能落库（红线） | P1 | 安全·高 |
| P2-V6 | `provider_options` 无键保护，可覆盖 canonical 关键字段 | P2 | 安全·中 |
| P4-V1 | PolicyEngine 未接线，信任闸门运行时不存在 | P4 | 安全·高 |
| P4-V2 | 调度器硬编码上下文，checkpoint 跨 run 键碰撞 | P4 | 安全/正确性·高 |
| P4-V3 | apply_patch 回滚不完整，create 覆盖既有文件丢原内容 | P4 | 数据完整性·高 |
| P4-V4 | NeverAsk/OnFailure 无危险命令硬拒绝地板 | P4 | 安全·中 |
| P4-V5 | Windows env allowlist 缺 SYSTEMROOT/TEMP/TMP 等 | P4 | 安全/正确性·中 |
| P6-V1 | Google API key 写入 URL query 而非请求头 | P6 | 安全·中 |
| P6-V4 | OAuth refresh token 轮换不持久化 | P6 | 功能完整性·中 |
| P7-V1 | hunk stage 用可预测临时文件（符号链接竞争/源码外泄） | P7 | 安全·中 |
| P7-V2 | git 参数注入（位置参数未防前导 `-`） | P7 | 安全·中 |

### 0.4 跨阶段基线偏差总表

**声明未引用（基线声明但全仓库零引用，建议移出基线）**

| 依赖 | 声明位置 | 来源阶段 | 说明 |
| --- | --- | --- | --- |
| `uuid` | workspace 基线 | P1 | ID 均为 newtype，唯一性靠 DB 主键 |
| `tracing-appender` | workspace 基线 | P1 | 日志仅存内存 ring buffer，无落盘 |
| `similar` | workspace 基线 | P1 声明 / P7-3 未落地 | diff-service 实际解析 git 结构化输出，word-level diff 未实现 |
| `backon` | workspace + provider-runtime | P2 | 生产重试由 agent-engine 自实现，provider-runtime `ExponentialBackoff` 为死代码 |
| `arbitrary` | workspace 基线 | P2 | 无 `fuzz/` 目录，属性测试由 proptest 承担 |
| `content-inspector` | workspace 基线 | P4 | read_file 实际用 chardetng+encoding_rs |
| `oauth2` | workspace 基线 | P6 | OAuth 手写实现（PKCE/Device/refresh），零引用 |

**引入未登记（各 crate 引入但未回填 workspace 基线）**

| 依赖 | 位置 | 来源阶段 |
| --- | --- | --- |
| `futures` / `bytes` | workspace 根 Cargo.toml | P2 |
| `parking_lot` / `tempfile` | git-service / diff-service | P7（tempfile 亦用于 P2） |
| `base64` / `rand` / `sha2` / `url` | auth-service | P6（手写 OAuth） |

**crate 内死依赖（声明但该 crate 源码零引用）**

| 依赖 | crate | 阶段 |
| --- | --- | --- |
| `agent-domain` | policy-engine、checkpoint-service | P4 |
| `bytes` / `futures` | process-runtime | P4 |
| `serde_json` / `thiserror` | diff-service | P7 |

> 建议：一次小型基线清理任务统一处理以上三表，并在 CI 增加 `cargo machete`/`cargo udeps` 门禁。
>
> 更新（2026-08-10）：该门禁曾以 L3 维护工作流实现，后决定**不在本项目配置自动执行的 Actions**，`.github/workflows/dependency-hygiene.yml` 已移除；machete/udeps 保留为文档记录的维护期检查项，随 L3 维护人工执行。

### 0.5 plan 文档同步状态

> 评审当时（2026-08-08）快照。P2/P3 的「未同步」状态已在 P2-12/P3-11 修复任务中纠正（复核确认全部 plan 已勾选）。

| Phase | plan 同步状态 | 说明 |
| --- | --- | --- |
| P1 | ✅ 全部勾选 | 与 ROADMAP 一致 |
| P2 | ❌ 未同步 | 11 篇全 🟡未开始，19 个验收框未勾；提交未触碰 plan/ 与 docs/ |
| P3 | ❌ 未同步 | 10 篇全 🟡未开始，18 个验收框未勾；与 P2 同病 |
| P4 | ✅ 全部勾选 | 纠正了 P2/P3 的流程偏差，ROADMAP 同步 |
| P5 | ✅ 多数已勾 | 验收项大多已勾选 |
| P6 | ✅ 已勾选 | 各 plan 验收点有对应测试 |
| P7 | ✅ 已勾选 | P7-7/P7-8 已勾选；P7-1–6 均有对应测试 |

### 0.6 测试可信度提示

跨阶段反复出现「mock / 单测全绿，但真实端点或组合会暴露问题」：

- **P2**：reqwest 总超时、select! 守卫、未发 `include_usage`、`list_models` 未带认证（V1–V4）均被 wiremock 遮蔽。
- **P3**：89 项测试几乎全为单模块自测，零「ProviderLoop + ToolScheduler + MessageQueue + 预算 + 重试」真实组合覆盖。
- **P4**：PolicyEngine 13 处调用全在自测，零生产调用。
- **P5**：export/import 往返测试仅覆盖单分支，多分支正确性缺口未暴露。
- **P6**：Anthropic thinking budget 与 max_tokens 冲突被 mock 测试漏过（V2）。

建议：针对性地补充「跨模块端到端」与「触网 mock 语义」用例，并在「通电」后建立最小真实组合测试。


## 整合说明

- **P1–P7 评审文档**：原整合于本文件的各阶段内容已拆分至 `docs/review/p1-review.md` … `docs/review/p7-review.md`（忠实抽取，仅改标题层级，逐字节一致）。
- **修复记录合并**：各 `plan/PN-XX-review-remediation.md` 的修复计划（细分步骤、主要产出物、验收标准、验证记录）已并入对应 `docs/review/pN-review.md` 的「修复记录（review-remediation）」章节；`plan/` 原件保留作为任务溯源。
- **跨阶段总览**：§0 为评审当时（2026-08-08）的整合视角，原样保留以记录评审快照。
- **修复复核**：2026-08-09 对 P1–P7 全部 V 项 + 基线偏差逐项核对，结论见 [§复核结论](#复核结论2026-08-09)。
- **各阶段内部 V1–Vn 编号独立**；跨阶段引用以 `P<阶段>-V<n>` 前缀区分。
- **P8–P11 评审**：独立评审文档。P8/P9/P11 已有对应修复任务并复核；P10 待复核。
- **P8 修复复核**（2026-08-10）：[P8-9](plan/P8-9-review-remediation.md) 已落地 §3.3 死 API 删除、§3.1 Skills 依赖引擎简化、§3.5 校验合并、§4.1 双优先级表守护测试、§3.2 deferred-consumer 文档标记；§2 零端到端消费者、§3.4 ResourceBundle 双状态、§4.2 session/run 重映射、§4.3 watch/诊断视图接线显式延后到 P13。验证：`cargo test -p resource-loader`（54 passed）、`-p context-engine`（31 passed）、clippy/fmt 干净；deepseek_reviewer 独立复核无阻塞项。
- **P10 修复复核**（2026-08-10）：[P10-7](plan/P10-7-review-remediation.md) 已落地 §3.1/§3.4/§3.5/§3.6 死 pub API 删除（qualified_name/plugin_context/into_tool_registry/registry 与 trust 查询方法）、§4.1 Lifecycle 双路径合并为单一 `invoke_with_state`（吃掉 §3.2 重复 input 预检与 §3.3 内联快照闸门）、§4.3a `HostConfig::validate` 新增 `invoke_timeout>=epoch_tick` 约束、§3.9/§4.4 文档不一致修正；§2 零端到端消费者、§3.7 四个预留 capability、§3.8 Load/Register/Unload 死事件、§4.3b,c Drop/epoch 微优化、§4.5 PluginStateStore trait 上移显式延后到 P13/P17-2/durable backend。验证：`cargo test -p wasm-plugin-host --lib`（8 passed）、`--test host_wat`（29 passed / 4 既有 trap 测试因 Windows debug 下 wasmtime 27 abort 跳过，git stash 基线复现确认非回归）、`-p plugin-api`（15 passed）、clippy/fmt 干净；deepseek_reviewer 独立复核无阻塞项。
- **P9 修复复核**（2026-08-10）：[P9-8](plan/P9-8-mcp-review-remediation.md) 已落地 §3.6 删 `McpConfig::merge`、§3.3 删单变体 `SecretValue`、§3.2 合并双 `RestartPolicy`（去 +1/×16）、§3.5 收敛 `is_loopback_url`/URL 校验、§3.4 删 `McpInvocationPolicy`（adapter 五道门禁逻辑保留）、§3.8 `error.rs`+`session.rs` 并入 `lib.rs`、§3.1 deferred-consumer 文档标记；写入集仅 `crates/mcp-client/src/`（净 +315/−376）；§4.1 adapter 门禁 vs 调度器门禁（P0，与 P15-1+ADR 协同）、§4.3 stdio→Sandbox Runtime、§4.4 McpPeer canonical DTO、§3.7 输出截断→tool-runtime、§4.2 OAuth 双 bearer 合并显式延后。验证：`cargo test -p mcp-client`（48 passed）、clippy `--all-targets -D warnings`、fmt `--check` 全绿；deepseek_reviewer 独立复核 VERDICT: PASS（七项全 PASS，门禁逻辑完整，无越界）。P11 仍无对应修复任务。
- **P11 修复复核**（2026-08-10）：[P11-9](plan/P11-9-review-remediation.md) 已落地 §2.1 删未接线的两个 `From<&ExecutionConstraints>` impl + 单测、§2.2 env/secret 单一权威来源（sandbox-runtime pub 导出，run_command 删本地副本）、§2.3 Linux `SYSTEM_READ_PATHS` 共享 const（bwrap/Landlock 共用）、§2.4 跨平台路径统一（resource-loader 复用 policy-engine 符号，删本地同构副本，移除 dunce 直接依赖）、§2.5 删 Windows `JobLimitsConfig`/`policy_to_job_limits`（AppContainer 生成器 frozen 标注）、§2.6 删 `SandboxProcessSpec.needs_network`/`SandboxProcess::kill`（`_handle` 保留为生命周期守卫）、`network_allow_hosts` 标注、run_command timeout 双写消除、§3.1 sandbox.md/process.md 新增「主流程集成边界」段、§3.2 attach_external 契约 doc-comment 补全、§3.3 PTY `dropped_events` 可观测性 + 两个测试；§2.6 NetworkMode::Off/Hint 合并（P11-1.E1）、§3.4 process-runtime 文件拆分（下次触碰）显式延后。验证：受影响 6 crate 联合 test（206 passed）、clippy `--all-targets -D warnings`、fmt `--check` 全绿。**关键实证修正**：review §2.6(b) 称私有 `handle` 零消费方，实测为 `ProcessHandle::Drop` 生命周期守卫，保留字段仅删 `kill()` 方法。
- 本文为评审记录与索引，不代表已批准的变更；所有结论均有文件与行号级证据，见各 `docs/review/pN-review.md`。
