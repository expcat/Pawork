# S12：全项目 Code Review 与整改拆分

> 阶段 S12 · 只读审查与任务化 · 状态：🟢已完成（2026-08-18 收口） · 依赖：S0–S11 的状态、延期项与验收证据已回写 · 规模：大（审查 + 记录，不实现、不测试、不发布）

## 目标

对当前 Pawork workspace 做一次可追溯的全项目 Code Review，系统检查安全漏洞、逻辑 Bug、持久化与并发风险、性能与维护性问题，以及“文档声称完成但生产路径未实际接线”的需求缺口。S12 本身只产出审查记录和后续任务：不修改 Rust 源码，不运行测试、构建、格式化、基准、fuzz 或真实通道冒烟，也不执行部署、发布或远程变更。

审查以当前分支、工作区差异、源码、manifest、生成物和活动文档为事实源。历史 V1 结论只作定位线索；“存在 TODO”“缺少测试”或“未搜到调用点”不能单独证明漏洞，必须记录可复核的源码证据与不确定性。

## 审查包（可独立执行）

每个审查包拥有互不重叠的主审范围，产出一份 `docs/reviews/s12/CR-xx-*.md`。跨包接口问题由最先发现者登记一个 finding，并只在其他报告中链接，禁止重复建任务。

| ID | 主审范围 | 核心问题 | 主审模型 |
| --- | --- | --- | --- |
| CR-01 | workspace `Cargo.toml`、各包 manifest、`docs/design.md` 的包布局与依赖契约 | 依赖方向、feature 组合、canonical 纯净、循环/反向依赖、manifest 与冻结契约漂移 | GLM |
| CR-02 | `execution/policy`、`execution/tools`、`workspace/core`、`vcs/git` | 路径越界、symlink/TOCTOU、命令与参数注入、审批绕过、写入与回滚边界 | Grok |
| CR-03 | `execution/exec`、`host/cli` 的进程/服务路径 | 沙箱 fail-closed、平台降级、进程树/PTY 生命周期、取消与资源泄漏、危险命令边界 | Grok |
| CR-04 | `providers/auth`、`providers/core`、`providers/adapters`、`net/net`、`foundation/config`、`foundation/diagnostics`、`extensions/mcp` | Secret 泄漏、OAuth/并发刷新、端点与代理边界、SSRF/重定向、MCP 子进程与配置注入、协议能力诚实性 | Grok |
| CR-05 | `foundation/sqlite`、`storage/session`、`storage/blob`、`foundation/domain` 的事件/存储类型 | append-only、事务与迁移、重放/恢复、compaction、导入导出、blob/checkpoint 完整性与并发一致性 | GLM |
| CR-06 | `engine/engine`、`foundation/api` | Agent Loop、tool-calling、上下文预算、取消、审批时序、错误传播、重复执行与 Provider 无特例约束 | GLM |
| CR-07 | `foundation/protocol`、`host/app`、`host/gui-server`、`host/transport`、`host/channels`、`clients/gui-client`、`clients/sdk`、`clients/compat`、`apps/protocol-probe` | 协议/schema 漂移、鉴权、幂等、Replay/Resume、背压、连接生命周期、能力声明与真实实现一致性 | Grok |
| CR-08 | `apps/desktop`、`docs/gui-design.md`、`design/` | 四层边界、异步/线程生命周期、状态投影、可访问性、视觉与交互需求是否真实落地；只审源码与既有证据，不启动 GUI | GLM |
| CR-09 | `ROADMAP.md`、`plan/`、`docs/`、`README.md`、`v2_plan.md` 与全仓 TODO/feature/call-site 索引 | S0–S11 需求追踪、假完成/零消费者、dead code、重复实现、可优化热点、延期项与状态/证据矛盾 | GLM |

补充主审范围（2026-08-17 主代理登记，修复任务书未列 S9/S11 新增包的缺口；不改变各包既有范围与模型分工）：

| 追加到 | 补充范围 | 理由 |
| --- | --- | --- |
| CR-02 | `workspace/resources` | AGENTS.md/Skills 加载的路径越界与注入面，与 CR-02 核心问题同源 |
| CR-04 | `control-plane/provider-control` | CredentialPool/lease 属凭证边界；`account-control-v1` feature 下未接宿主的 account/routing/health/factory/reconciler 允许降采样，未深审部分列入「未覆盖路径」 |
| CR-05 | `control-plane/core`、`control-plane/quota` | UsageLedger/audit JSONL/LocalLedger 的持久化与并发一致性 |
| CR-06 | `workflow/core`、`workflow/memory`、`workflow/review`、`agents/orchestration`、`foundation/testkit` | 状态机/reducer/多 Agent 编排与 MockProvider 的逻辑审查；testkit 采样即可 |

Grok 负责对抗性与高风险边界，GLM 负责契约、数据流与需求追踪。只有 Critical/High finding 才由另一模型做一次交叉复核；确定性证据检查先于模型复核，不为同一普通 finding 重复调用审查者。

## Finding 记录格式

每条 finding 使用稳定编号 `S12-CRxx-NN`，并包含：

1. 类别：Security / Bug / Requirement Gap / Performance / Maintainability / False Completion。
2. 严重度：Critical / High / Medium / Low；置信度：Confirmed / Needs Verification。
3. 证据：精确到仓库相对路径、符号与行号；说明实际行为、期望行为和影响面。
4. 验证建议：后续任务应运行的最小复现、定向测试或真实界面/平台证据；S12 内不执行。
5. 整改边界：最小写入集、依赖关系、不可顺带处理的相邻问题。

`Needs Verification` 只能留在审查报告，不能作为已确认漏洞写入结论。若文档与源码冲突，以源码和真实产物为准，同时登记文档漂移。

## ROADMAP 回写规则

- 每条已确认且接受整改的 finding 都在 [ROADMAP](../ROADMAP.md) §3.2 新增一个独立任务；只有根因和写入集完全相同的 finding 才允许合并。
- 安全漏洞与数据损坏风险优先登记；功能缺口、性能和维护性问题分别排队，不用一个“修复全部审查问题”的总任务吞并。
- 需要产品/架构决策的 finding 写入 ROADMAP §4；纯候选能力写入 §3.3；仍属于未完成阶段退出标准的项目回写原阶段，不重复建账。
- 后续任务必须写明验收证据，实施、测试、三平台验证或发布均在各自任务中另行授权和执行。

## 已知基线

开始审查前先读取 ROADMAP §3.2 的“已知待完善基线”。这些条目来自当前文档挂账或源码中的显式未接线标记，不算 S12 新发现，也不得因为已登记而跳过对应审查包。

## 退出标准

- [x] CR-01～CR-09 均有独立报告，列明实际审查路径、未覆盖路径与 finding。（[docs/reviews/s12/](../docs/reviews/s12/)，2026-08-17～18；另附补充主审范围登记，见上文 CR 表后注）
- [x] 所有 finding 均有类别、严重度、置信度、源码证据和后续验证建议。（60 条，全部 Confirmed；18 条 High 均经另一模型交叉复核，3 条裁定降 Medium、4 处证据/表述修正已回写主报告）
- [x] Confirmed finding 全部按规则回写 ROADMAP；Needs Verification 没有被冒充为确定结论。（57 项 S12-F01～F57 任务，见 [ROADMAP](../ROADMAP.md) §3.2；CR04-06 链接 K-10、CR09-05 随 F01 收口、CR02-01/02 同根因合并）
- [x] S0–S11 的需求与状态逐项建立“计划 → 生产调用点/用户界面 → 既有证据”追踪，未落地项已登记。（[CR-09](../docs/reviews/s12/CR-09-traceability-consistency.md) §2 追踪表）
- [x] 本阶段没有修改生产代码、运行测试/构建/冒烟、执行发布或远程操作。（工作区差异仅 docs/reviews/s12/、本任务书、ROADMAP、v2_plan.md）

## 明确不做

- 不修复 finding，不新增测试，不重构，不做依赖升级。
- 不执行 Workspace Full Gate、clippy/fmt、fuzz、基准、三平台矩阵或真实 API/GUI 冒烟。
- 不做 crates.io dry-run/发布、打包、签名、部署、tag 或 V1 归档操作。
- 原 Release Hardening 清单不再属于 S12；未来若决定发布，须在 S12 finding 整改完成后另立并明确授权。

## 参考

- [../ROADMAP.md](../ROADMAP.md) §2/§3.2/§4 · [../docs/design.md](../docs/design.md) §2–§4 · [../docs/gui-design.md](../docs/gui-design.md)
- [../docs/task-guide.md](../docs/task-guide.md) · [../docs/v1-migration-reference.md](../docs/v1-migration-reference.md)（历史迁移与旧 Release Hardening 参考，不是当前执行任务）
