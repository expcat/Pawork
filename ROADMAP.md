# Pawork V2 开发路线图（增量式 · S0–S12）

> 本文档是 Pawork V2 的**任务总索引**：登记全部任务（已完成 / 未完成）的状态与粗略介绍，并链接到 [plan/](plan/) 内的详细任务文档。V2 采用「最小可用 → 逐级追加」的增量开发方式：S0 先交付一个能真实对话的 `pawork` CLI，S0–S11 在可运行的二进制上逐层追加能力并以真实冒烟 + 定向自动化验收；S12 改为只读的全项目 Code Review，把发现拆成独立后续任务，不在审查阶段直接修复或发布。
>
> **文档体系**（五份常设文档 + 三类附件）：
>
> | 文档 | 职责 |
> | --- | --- |
> | 本文 `ROADMAP.md` | 任务总索引：阶段状态、阶段外任务、未决事项、风险 |
> | [plan/S0–S12](plan/) | 每阶段任务书：目标、范围、退出标准与并行拆分；S0–S11 含冒烟/定向自动化，S12 含独立审查包与 finding 回写规则（附件 [plan/archive/](plan/archive/README.md)：旧按域计划索引；M0–M8 正文未落仓，迁移细则回退到 `docs/v1-migration-reference.md` §4.1） |
> | [docs/design.md](docs/design.md) | 设计文档：包布局与激活映射、冻结契约、各阶段功能设计与参照项目映射、候选功能 |
> | [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计：最小 Agent 壳、参照取舍、随阶段增量图（附件 [design/README.md](design/README.md)：v3 定稿视觉实施基准与三张定稿图） |
> | [docs/references.md](docs/references.md) | 参照项目手册：对标项目的目标、功能与文档链接（附件 [docs/research/](docs/research/)：专题调研全文） |
> | [docs/task-guide.md](docs/task-guide.md) | 任务实现规范：任务开启 / 进行 / 收尾的公共约定与最小启动提示词 |
> | [docs/v1-migration-reference.md](docs/v1-migration-reference.md) | V1 全量 Review 结论与 V1→V2 迁移词典（原 ROADMAP_V2.md，冻结参考） |
>
> 工作约定见仓库根 [AGENTS.md](AGENTS.md)（V2 版，2026-08-17 重建；V1 版随 V1 归档于仓库外同级目录 [../Pawork_v1/AGENTS.md](../Pawork_v1/AGENTS.md)）。V2 开发期放宽项见 [docs/task-guide.md](docs/task-guide.md) §6。

---

## 1. 计划原则

旧「按域整体迁移」计划（M0–M8；[plan/archive/](plan/archive/README.md) 仅保留索引，正文未落仓）第一个可运行物要到第 5 个里程碑才出现，此前全部是「库先行、零消费者」——正是 V1「组件齐全、主干未通电」病灶（[docs/v1-migration-reference.md](docs/v1-migration-reference.md) §1.2）在计划层的重演。现行计划的四条组织原则：

1. **每阶段交付可运行增量**：从 S0 起 `pawork` 二进制始终可编译、可运行、可被真实使用；每个阶段以「新增哪些用户可见能力」定义，而不是以「迁移了哪些包」定义。
2. **真实测试优先、低消耗默认**：S0–S11 的冒烟与模型行为评估用真实通道执行；S0–S5 期为 GLM Coding Plan + OpenCode Go 两通道，S6 接通首发通道后默认切换为 §1.1 低消耗模型矩阵，高级模型仅在用户明确指定时使用。自动化测试只做关键定向项（契约 golden、安全红线、解析器种子），当前 S0–S12 不设 Workspace Full Gate；S12 只审查和登记。全量门禁、三平台验证与发布不再挂在 S12，只有在审查整改完成且用户明确决定发布后才另立任务。
3. **追加而非重写**：用三道保险（终局包布局先行、冻结契约先行、迁移词典与「无消费者不合入」，见 [docs/design.md](docs/design.md) §3）保证后期把 V1 全部功能追加进来时，不需要推翻任何已交付阶段的代码。V1 的约 23.6 万行资产仍按「复制 + 合并 + 改名」迁移，只是从「按域一次性搬」改为「按阶段按需搬」。
4. **GUI 增量主线**：S7 起以定稿 v3 三栏工作台（[design/README.md](design/README.md)）为唯一壳，S8–S11 每阶段按 [docs/gui-design.md](docs/gui-design.md) §5 给同一壳加面（各任务书带「GUI 增量」行）；没有对应投影/命令就不做按钮，视觉验收一律对照该基准。

### 1.1 真实测试模型约定（低消耗默认）

| 通道（provider_id） | 默认测试模型 | 凭证形态 |
| --- | --- | --- |
| DeepSeek（`deepseek`） | `deepseek-v4-flash` | API key |
| GLM Coding Plan（`glm-coding`） | `glm-4.7` | API key |
| OpenCode Go（`opencode-go`） | `deepseek-v4-flash` | API key |
| xAI Grok 订阅（`xai`） | `grok-4.3` | OAuth bearer |

1. 常规冒烟、定向回归与模型评估默认只用矩阵内组合；ChatGPT、Qwen Token Plan 两通道及各通道更高档模型（如 `deepseek-v4-pro`、`glm-5.x`、`kimi-k2.x`、`grok-4.6`、ChatGPT/Codex 系列）默认不使用。
2. 仅两类例外：① 任务书明确要求的一次性通道接通验证（最小调用量，如 S6 六通道各打通一次）；② 用户为高级功能（多 Agent 编排、长上下文压缩、复杂工具链等）**明确指定**的高级模型专项评估——高级模型永远等用户指定，不得自行升级。
3. 模型名以 `pawork models` / 通道 model catalog 实际返回为准；目录中无指定名称时终止并报告，不擅自换相邻档位。
4. 凭证已在 `~/.pawork/auth.json`（env 变量按 [docs/task-guide.md](docs/task-guide.md) §5 降级为 fallback）；缺失或失效即 fail-closed 终止并向用户索取，不静默降级 mock、不换通道。

Secret 红线不变：key/token 不入日志、事件、配置样例与任何可提交文件。

---

## 2. 阶段总览（S0–S12）

状态符号：⚪未开始 · 🔵进行中 · 🟢已完成 · ⚠️阻塞。每阶段的详细任务、退出标准与并行拆分见 `plan/S*.md`；各阶段功能设计与参照项目映射见 [docs/design.md](docs/design.md) §4。

| 阶段 | 主题 | 新增用户可见能力 | 激活 / 增强的包 | GUI 增量（v3） | 真实验收要点 | 状态 |
| --- | --- | --- | --- | --- | --- | --- |
| [S0](plan/S0-minimal-chat.md) | 最小可对话 CLI | `pawork chat` 流式多轮对话、Ctrl-C 取消、`pawork models`、TOML 配置 + env key | workspace 根、domain（最小）、api（provider）、net、providers/adapters（openai-compatible）、config（最小）、engine（最小）、app（最小）、cli（最小）、apps/pawork | — | 两把真实 key 各完成流式多轮对话；401/429/超时可读呈现 | 🟢 |
| [S1](plan/S1-sessions.md) | 会话持久化与恢复 | 会话落盘、`pawork sessions list/show`、`--resume` 续聊、`--json` 事件流输出 | sqlite、session（核心）、domain（events 全量）、engine（事件化 + appender） | — | 中断/杀进程后 resume 续聊上下文连续；envelope golden 与 append-only 契约生效 | 🟢 |
| [S2](plan/S2-tool-loop.md) | Agent Loop 与只读工具 | Agent 自主调用 read/list/search/find 回答仓库问题；`@`引用前身（相对路径语义） | api（tool）、tools（只读四件）、workspace（roots）、engine（工具循环）、providers/adapters（anthropic-messages）、testkit（MockProvider） | — | 真实仓库问答任务；OpenAI/Anthropic 双协议 tool-calling 对比评估 | 🟢 |
| [S3](plan/S3-safe-edits.md) | 写入工具与审批 | write/edit/apply_patch + 终端审批交互（`--approval-mode`） | policy（整包）、tools（写三件）、engine/cli（审批位点） | — | 真实小编码任务经审批落盘；越界/symlink 拒绝；deny 后会话可续 | 🟢 |
| [S4](plan/S4-exec-sandbox.md) | 命令执行与沙箱 | run_command（进程树清理 + 沙箱 + 输出截断）——首个完整「读-改-跑」编码闭环 | exec（process/sandbox）、tools（run_command）、policy（shell 分类接线） | — | 「跑 cargo check 并修复报错」端到端；Ctrl-C 杀整棵进程树；fail-closed | 🟢 |
| [S5](plan/S5-context-usage.md) | 上下文预算与用量 | 长任务不炸上下文（预算/截断/压缩）、token 与费用统计显示 | engine（context 接线）、session（compaction feature）、provider-core（usage/registry/pricing） | — | 超长多轮任务连贯完成；token 计量与厂商侧抽查一致 | 🟢 |
| [S6](plan/S6-providers-auth.md) | 首发 Provider 与认证 | 六通道适配、`pawork models` 聚合、auth 文件、API key 与 ChatGPT/xAI OAuth | providers/adapters（六通道）、auth、diagnostics（脱敏 layer）、config（凭证解析） | — | 六通道真实使用；运行中切换 provider/model；OAuth 临期刷新；secret 不入日志回归 | 🔵 |
| [S7](plan/S7-gui-agent.md) | 最小 Agent GUI（波 0–D ✅） | 已锁定并交付 v3 三栏工作台：TaskRail 双分组与定向新建、流式对话、内嵌审批、取消、ContextMeter / RunStatusBar；跨通道 `glm-4.7`→`deepseek-v4-flash` 冒烟通过 | protocol（最小帧）、transport（local）、gui-server（单客户端）、client、apps/desktop、cli `gui serve` | v3 三栏壳整体：TaskRail 双分组与定向新建、内嵌审批、Composer+ContextMeter、RunStatusBar、InspectorToolTabs 预留 | 设计锁定；真实模型流式对话；关窗不杀 Run；跨通道切换；1440×1024 人工对照定稿图未做 | 🟢 |
| [S8](plan/S8-git-checkpoint.md) | Git、Diff 与 Checkpoint（波 A–B ✅） | 会话改动 diff 呈现、编辑前快照、`pawork rollback` 一键回滚；CLI 审批 hunk 预览 | git（git+diff）、blob-store（artifact/protected/checkpoint）、engine/app/cli 接线 | Inspector Changes（Files/Summary）+ ActivityPopover Changes 摘要（Desktop 面延期，见 §4） | 真实任务后 diff 审阅 + 回滚还原；git 注入防护回归 | 🟢 |
| [S9](plan/S9-mcp-resources.md) | MCP、资源与兼容导入（波 A–C ✅） | 外接 MCP server 工具、AGENTS.md/Skills 生效、`@file` 引用、导入 Claude/Codex 等配置；CLI `mcp`/`import`/`sessions import|export` | mcp（rmcp 收口）、resources、compat、workspace（file-index）、config（完整层级）、engine/app/cli 接线 | Composer `@` 补全；Resources 只读（MCP/规则）（Desktop 面延期，见 §4） | 真实 MCP server 工具与内置共存；本机 Claude 配置导入可用 | 🟢 |
| [S10](plan/S10-serve-clients.md) | 服务化与客户端补齐（10a ✅ · 10b ✅ · 收口 ✅ · S0–S9 回归 ✅ · Zed ACP ✅） | headless/SDK/ACP/service、多 GUI Replay、Fork、PTY；`--json` 对齐正式协议 | protocol 收口、transport 补齐、gui-server 多客户端、sdk、channels、app/cli 正式化、exec（pty）、session lifecycle、protocol-probe | 正式 Replay、Fork、Terminal tab、本机多窗口 | protocol-probe 全过；SDK e2e；两 GUI Replay 已过；S0–S9 回归已复跑；Zed 1.15 Agent Panel `pong`/`end_turn` | 🟢 |
| [S11](plan/S11-workflow-control.md) | 工作流、多 Agent 与控制面（波 A–D ✅） | Plan 整版审批 gate、`pawork tasks`、`pawork usage`、`pawork agents demo`；多账户 factory/routing 与 Desktop Workflow 面未做（§4） | workflow、memory、review、orchestration、control-plane、provider-control、quota、app/cli 接线 | Host 已接 `QuotaOverview`；Desktop Workflow / quota 条 / ActivityPopover Agent 列表未做 | plan 拦截→批准→改计划再拦；usage 与 S5 行 1:1；tasks 跨进程可见；supervisor demo 完成/cancel-tree/budget-gate；两通道短指令评估 | 🟢 |
| [S12](plan/S12-project-code-review.md) | 全项目 Code Review 与整改拆分 | —（只读审查 + finding 任务化） | 全部现有包及跨包接口 | 审查 v3 设计、投影、协议能力与现有证据的一致性；不改 UI、不启动窗口 | CR-01～CR-09 独立报告完成；安全/Bug/性能/假完成/未落地需求均有证据与置信度；Confirmed finding 逐项写入 §3.2 | 🟢 |

**关键节点**：S4 结束即达成旧计划 M4 的首要验收（真实仓库「读文件-改代码-跑命令」闭环）。S5–S6 补齐用量与首发通道后，**S7 已按 v3 定稿交付最小 Agent GUI**（正式帧真实对话/审批/取消/重连/TaskRail/跨通道切换）；S8–S11 按 [docs/gui-design.md](docs/gui-design.md) §5 在同一壳上依次点亮 Changes → `@`/Resources → Replay/Fork/Terminal/多窗口 → Workflow/quota/Agent 列表，其中 S10 把单窗口升级为多客户端服务；S12 对完整结果做只读 Code Review，并把整改拆成阶段外独立任务。发布不在当前 S0–S12 排期。WASM 插件 / Hooks / LSP / 市场同样不在当前排期，见 §4。

**依赖关系**：S0→S1→S2→S3→S4 严格串行（主干长成）；S5 与 S6 在 S4 后可并行；S7 依赖 S1–S5（会话/事件/工具/审批），S6 建议先行但不阻塞设计波；S8 依赖 S3（写工具），可与 S7 部分并行，有 Desktop 则同步 Changes；S9 依赖 S2 与 S6；S10 依赖 S7；S11 依赖 S10；S12 在 S0–S11 状态、延期项与证据完成回写后启动。S12 不实现或测试 finding；整改按 §3.2 逐项排期。GUI 各阶段加面的视觉与交互验收一律对照 [design/README.md](design/README.md) v3 基准。

---

## 3. 阶段外任务登记

### 3.1 已完成

| 任务 | 完成日期 | 产出 |
| --- | --- | --- |
| V1 全量 Review 与 V2 重构方案（原 ROADMAP_V2.md） | 2026-08-14 | [docs/v1-migration-reference.md](docs/v1-migration-reference.md)（Review 结论、目录结构、映射总表、发布与测试策略） |
| 按域迁移计划 M0–M8 登记（后被增量式取代；正文未落仓） | 2026-08-14 | [plan/archive/](plan/archive/README.md)（索引）+ [迁移映射 §4.1](docs/v1-migration-reference.md) |
| 重规划为增量式阶段计划 S0–S12 | 2026-08-14 | 本文 §2 + [plan/S0–S12](plan/) |
| 调整后续顺序：插件移出排期、GUI 提前并先设计 | 2026-08-16 | 本文 §2/§4；[docs/gui-design.md](docs/gui-design.md)；[plan/S7-gui-agent.md](plan/S7-gui-agent.md)；旧扩展任务书归档为 [plan/archive/S10-extensions-deferred.md](plan/archive/S10-extensions-deferred.md) |
| 多账户额度/切换/子 Agent 路由/输入缓存调研与方案确认（G1–G7 → F1–F6，决策 D1–D8 全部确认） | 2026-08-14 | [docs/research/](docs/research/) 三篇；候选登记 [docs/design.md](docs/design.md) §5 |
| 文档体系整合（五文档结构：索引 / 任务书 / 设计 / 参照 / 规范） | 2026-08-14 | 本文 + [docs/design.md](docs/design.md) + [docs/references.md](docs/references.md) + [docs/task-guide.md](docs/task-guide.md) + [docs/v1-migration-reference.md](docs/v1-migration-reference.md) |
| Desktop GUI v3 视觉基准定稿（三栏工作台、TaskRail 双分组、ContextMeter/RunStatusBar/InspectorToolTabs/ActivityPopover） | 2026-08-17 | [design/README.md](design/README.md) + 三张定稿图；[docs/gui-design.md](docs/gui-design.md) 同步修订 |
| ROADMAP 按 GUI 主线重设计 + 低消耗测试模型约定 | 2026-08-17 | 本文 §1.1/§2；[docs/task-guide.md](docs/task-guide.md) §5 同步 |
| V1 代码归档与 V2 升为仓库根 | 2026-08-17 | 仓库根全部 V1 资产移至仓库外同级目录 [../Pawork_v1](../Pawork_v1/)（移出 git 管理，历史仍可追溯）；`Pawork_v2/` 内容经 `git mv` 升为仓库根（staged：1280 D / 267 R / 6 同名替换）；本文头部、S12 行与 §4 同步。后续跟进：`foundation/config` 的仓库根加载路径按新布局回归 |
| 重建仓库根文件与全量引用修复 | 2026-08-17 | 重建 [AGENTS.md](AGENTS.md)（V2 版，§5 开发期验证放宽与 task-guide §6 双向同步）、[README.md](README.md)、[.gitattributes](.gitattributes)；修复迁移断链 84 处（V1 资产链接改指 [../Pawork_v1/](../Pawork_v1/)、未落仓 `archive/M0–M8` 回退 [plan/archive/README.md](plan/archive/README.md)、深度修正 2 处）与 15 处陈旧文本引用 |
| S12 从发布硬化重规划为全项目 Code Review | 2026-08-17 | 原 Release Hardening/发布任务移出当前排期；新增 [S12 审查任务书](plan/S12-project-code-review.md)，按九个独立审查包产出 finding，并逐项回写 §3.2 |

### 3.2 待执行（阶段之外）

| 任务 | 说明 | 任务书 / 依据 | 状态 |
| --- | --- | --- | --- |
| 多账户功能族并入 plan | 把已确认的 F1–F5 与 G6 增量写入 S2/S5/S6/S9/S11 计划文档，并按「少测试」约定核减非关键测试项；release 目标只保留为未来发布任务输入，不写入 S12 | [docs/research/multi-account-quota-plan-merge.md](docs/research/multi-account-quota-plan-merge.md) §4（前置条件已满足，可随时开启） | ⚪ |
| K-01 仓库根迁移后的 config 路径闭环 | 核对 `foundation/config` 在 V2 摊平后的仓库根发现、层级加载与示例路径；若不一致再用独立实现任务修复 | §3.1 的迁移后跟进项；[S9 任务书](plan/S9-mcp-resources.md) | ⚪ |
| K-02 审批请求等待前持久化 | 让 `ToolApprovalRequested` 在进入用户等待前落盘，定义崩溃/`kill -9` 后 seal、resume 与“不重复执行”语义 | [S3 任务书](plan/S3-safe-edits.md)；`host/app/src/gui_host.rs` 的现有时序注释 | ⚪ |
| K-03 S7 Desktop 人工验收 | 补中文 IME、多行粘贴、1440×1024 对照 v3 定稿图和 1080×720 可用性证据；不以 probe/源码替代真实窗口 | [S7 任务书](plan/S7-gui-agent.md)；[GUI 设计](docs/gui-design.md) | ⚪ |
| K-04 S8 Desktop Changes 面 | 实现并真实验收 Inspector Changes（Files/Summary）与 ActivityPopover Changes 摘要，复用已有 `DiffListFiles`/`DiffGet` | [S8 任务书](plan/S8-git-checkpoint.md)；[GUI 设计](docs/gui-design.md) §5 | ⚪ |
| K-05 S9 本机会话格式导入 | 在取得脱敏样本后适配 `~/.claude/projects/**/*.jsonl` 与现行 Codex rollout `{timestamp,type,payload}`，保持源文件只读与 Secret 拒绝 | [S9 任务书](plan/S9-mcp-resources.md) | ⚪ |
| K-06 S9 Desktop `@` / Resources 面 | 实现 Composer `@file` 补全与 Resources 只读 MCP/规则视图，只消费既有 Host 查询/注入能力 | [S9 任务书](plan/S9-mcp-resources.md)；[GUI 设计](docs/gui-design.md) §5 | ⚪ |
| K-07 Host 增量事件限流接线 | `host/app/src/rate_limit.rs` 已有实现与测试但没有生产热路径调用；独立任务决定接入 Event Hub/GUI 广播或删除库存能力，并验证背压与丢弃可观测性 | `host/app/src/rate_limit.rs`；`v2_plan.md` S10 波次记录 | ⚪ |
| K-08 GUI ArtifactStreaming 能力一致性 | 当前客户端/Host 宣告 `ArtifactStreaming`，但 `host/gui-server/src/session.rs` 仍固定返回 unsupported，client 读取又受 `experimental` 门控；独立任务选择完整接线或停止宣告，并补协议行为证据 | [S10 任务书](plan/S10-serve-clients.md)；`host/gui-server/src/session.rs`、`clients/gui-client/src/lib.rs` | ⚪ |
| K-09 macOS sandbox host allowlist 语义 | 当前 `NetworkMode::Enforce` 对 `network_allow_hosts` 保持全拒并明确未实现；独立任务决定引入 egress broker，或收窄/移除不可兑现配置，同时保持 fail-closed | [S4 任务书](plan/S4-exec-sandbox.md)；`execution/exec/src/os/macos.rs` | ⚪ |
| K-10 Anthropic Messages 能力收口 | 对照 S6 的完成声明与 adapter 顶部 TODO，逐项决定 prompt cache、thinking、hosted tools、signature/server_tool/citations 是实现、显式 unsupported 还是延期，并同步能力表 | [S6 任务书](plan/S6-providers-auth.md)；`providers/adapters/src/anthropic/mod.rs` | ⚪ |

S12 全项目 Code Review（2026-08-17～18 审查并收口，九份报告 + 五份交叉复核见 [docs/reviews/s12/](docs/reviews/s12/)）共产出 60 条 finding（全部 Confirmed，Needs Verification 0；裁定后 High 15 / Medium 27 / Low 18），按回写规则合并为以下 57 项任务：S12-CR02-01/02 根因与写入集相同合并为 S12-F01；S12-CR04-06 仅补证据链接 K-10，不另建行；S12-CR09-05 随 S12-F01 统一收口，不另立写入集。排队顺序：安全与数据风险 → 功能缺口与 Bug → 性能与维护性。每项整改均为独立任务，实施与验证在各自任务中另行授权。

**安全与数据风险（High，S12-F01～F14）**

| 任务 | 说明 | 任务书 / 依据 | 状态 |
| --- | --- | --- | --- |
| S12-F01 只读工具 symlink 逃逸与 S3 路径内核假完成 | 读路径（read_file/list_directory/search_text）统一走 policy 路径内核（保持 `resolve_relative_path` 签名或 `resolve_rel` 转调），canonical 复核 + `.git` 段拒绝；四套路径校验实现以 policy::path 为单一事实源收口；`.git` 只读策略先拍板；同步纠正 S3 任务书勾选。验收：fixture 内 symlink→`~/.pawork/auth.json` 与 `/etc` 三工具拒绝、`.git/config` 读取拒绝、`rg resolve_rel execution/tools/src` 无生产调用、policy symlink/.git 单测加读工具用例全绿 | [CR-02](docs/reviews/s12/CR-02-policy-tools-git.md) S12-CR02-01/02（High，交叉复核 uphold）· [CR-09](docs/reviews/s12/CR-09-traceability-consistency.md) S12-CR09-05 | ⚪ |
| S12-F02 macOS Seatbelt 整盘只读与 Hard 标签失真 | 移除无条件 `(allow file-read* (subpath "/"))` 或将 isolation 标签诚实降级；`default_secret_paths` 补 `~/.pawork/auth.json`、`~/.gnupg`、`~/.config`。验收：Seatbelt 下 `cat` 上述路径被拒，且工具 metadata 的 isolation/note 与实际一致 | [CR-03](docs/reviews/s12/CR-03-exec-cli.md) S12-CR03-01（High，uphold） | ⚪ |
| S12-F03 service stop --apply 回收单元定义与 --instance 标识校验 | `normalize_instance` 限制为 `[A-Za-z0-9._-]`；stop --apply 删除 launchd plist、disable 并删除 systemd unit（Windows SCM 另验）。验收：install→start→stop --apply 后 plist/unit 不存在；带空格/分号/换行的 instance 被拒绝 | [CR-03](docs/reviews/s12/CR-03-exec-cli.md) S12-CR03-03（High，uphold） | ⚪ |
| S12-F04 Dangerous 命令分类补齐本机 shell 高危动词 | `Remove-Item`、`cmd /c del`、`curl|sh`/`wget|sh` 远程管道、`python -c`、`osascript`、`diskpart`、`schtasks`、`launchctl` 等纳入 Dangerous。验收：`classify_command` 对报告点名命令的矩阵单测全绿，AskForDangerous 下不再静默放行 | [CR-03](docs/reviews/s12/CR-03-exec-cli.md) S12-CR03-04（High，uphold） | ⚪ |
| S12-F05 MCP SecretRef 解析域隔离 | SecretRef 限制 `pawork.mcp.*` 或独立 MCP 后端；workspace 层不得解析用户全局 auth.json；注意加重事实：MCP 子进程 `env_clear=false` 继承宿主环境（含 `PAWORK_API_KEY_*`）。验收：SecretRef 指向 `pawork.openai`/`default` 时 fail-closed 定向测试 | [CR-04](docs/reviews/s12/CR-04-secret-net-mcp.md) S12-CR04-01（High，uphold） | ⚪ |
| S12-F06 workspace proxy_url/base_url 覆盖与跨域凭证泄漏 | `proxy_url`/非回环 `base_url` 与 `trust_workspaces` 同级剥离；出站客户端跨 origin 跳转 fail-closed 或剥离全部凭证头（含 `x-api-key`）。验收：sink 代理/伪造 base_url 不见 Bearer 与 x-api-key；跨 host 302 不带出 | [CR-04](docs/reviews/s12/CR-04-secret-net-mcp.md) S12-CR04-02（High，uphold） | ⚪ |
| S12-F07 上游错误正文未脱敏进入可持久化 RunFailed | `classify_status` 只保留 status/稳定错误码，正文经 Redactor 或丢弃。验收：mock 401 body 含明文 token 时 `session_events` 与 `RunFailed` JSON 无明文，定向回归 | [CR-04](docs/reviews/s12/CR-04-secret-net-mcp.md) S12-CR04-03（High，uphold） | ⚪ |
| S12-F08 workspace MCP 自封 trusted + auto_start 任意 stdio | workspace 层剥离 `mcp.*.trusted`/`auto_start`（或整段 mcp）；stdio 命令需全局 allowlist 或用户确认；MCP trusted 不得高于宿主 `workspace_trusted`；stdio 沙箱经 SandboxSelector。验收：未信任 workspace `trusted=true`+`auto_start` 不得启动，`allowed_in_untrusted_workspace` 仍为 false | [CR-04](docs/reviews/s12/CR-04-secret-net-mcp.md) S12-CR04-04（High，uphold） | ⚪ |
| S12-F09 分支/Fork 消费面补 branch 维度投影 | messages 投影增加分支/祖先语义（可能需要 v10 附加式迁移），`resume_messages`/compaction 按 active branch 祖先链；不改事件信封 v1 与 append-only 事实表。验收：fork 后 resume 只含祖先前缀；fork 分支压缩不删 main 低水位消息（定向测试） | [CR-05](docs/reviews/s12/CR-05-persistence-ledgers.md) S12-CR05-01（High，uphold） | ⚪ |
| S12-F10 gui serve 强制认证与本机 socket 收紧 | 生产装配接 `TokenAuthenticator`（无 authenticator fail-closed）；UDS 0600/目录 0700、Windows Named Pipe DACL；Desktop/client/probe 共用 `TOKEN_SCHEME`（与 S12-F52 同批）。验收：无 token 握手 Rejected；socket mode 0600；同机另一进程无凭证不能驱动 Run/PTY/审批 | [CR-07](docs/reviews/s12/CR-07-protocol-host-clients.md) S12-CR07-01（High，uphold） | ⚪ |
| S12-F11 连接 Lagged fail-closed 通知重建 | 队列满/broadcast Lagged 时向该连接发 `ReplayUnavailable`/`SnapshotRequired` 或断开；客户端消费该帧按 gui-design §4.1 重建。验收：`queue_capacity=2` 灌事件，客户端收到 ReplayUnavailable 或被踢且 Snapshot/Resume 收敛 | [CR-07](docs/reviews/s12/CR-07-protocol-host-clients.md) S12-CR07-02（High，uphold） | ⚪ |
| S12-F12 Timeline 锚点跨分页失效 | assistant/tool 锚点改存 event_id/sequence（或插入时平移）。验收：低 sequence 页晚于 live 锚点到达的乱序单测，工具状态回填与 assistant 合并目标正确 | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-01（High，uphold） | ⚪ |
| S12-F13 RunStart 携带 provider 维度 | 协议 `RunStart` 追加 optional provider（minor bump + golden 先行）→ host 按 provider 优先解析 → Desktop 发送；三处一个任务链，不可只改 Desktop。验收：deepseek 与 opencode-go 同名 `deepseek-v4-flash` 下选择后者，Host 实际 provider 断言 | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-02（High，uphold，方向经复核修正） | ⚪ |
| S12-F14 Desktop 主路径键盘可操作与可访问名称 | 审批/取消/模型/会话/新建/Inspector/菜单补焦点链与 keybinding；角标按钮 tooltip + accessible name + 禁用原因文本。验收：键盘走查完成审批/取消/切换（并入 K-03 人工验收）；控件级焦点/tooltip 静态断言 | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-03（High，uphold） | ⚪ |

**功能缺口与 Bug（Medium，S12-F15～F40）**

| 任务 | 说明 | 任务书 / 依据 | 状态 |
| --- | --- | --- | --- |
| S12-F15 storage/session → protocol::adapter 反向依赖 trait 倒置 | 先拍板 trait 归属（session vs domain；维持现状则改写词典 §4.1 #13 并留 ADR）。验收：`rg pawork_protocol storage/session/src` 收敛于 client_adapter.rs + 「protocol 类型变更不触碰 storage」编译边界断言 | [CR-01](docs/reviews/s12/CR-01-manifests-layout.md) S12-CR01-01 | ⚪ |
| S12-F16 ApprovalMode::OnFailure 语义落空 | 实现失败后再问（至少 WorkspaceWrite/Process/GitWrite）或收窄文档/CLI 为「当前等价 NeverAsk」（二选一）。验收：on-failure 下工具失败后第二次同类调用行为定向断言 | [CR-02](docs/reviews/s12/CR-02-policy-tools-git.md) S12-CR02-03 | ⚪ |
| S12-F17 未信任 workspace 仍注入 AGENTS.md/Skills 正文 | host 注入点按 `workspace_trusted` 过滤仓库层指令，或 loader 增加 trust 开关；不实现通用注入分类器。验收：未信任 fixture 的仓库 AGENTS.md 不进 `injected_layers` | [CR-02](docs/reviews/s12/CR-02-policy-tools-git.md) S12-CR02-04 | ⚪ |
| S12-F18 GUI PTY 沙箱/审批接线与退出回收 | `TerminalCreate` 复用 run_command 隔离/审批，或显式降级为「本机不受控终端」并告知；`GuiHostAdapter::shutdown` 与 gui serve 退出路径调用 `pty.shutdown()`。验收：Ctrl-C / service stop / kill -9 后 `pgrep` 无 PTY 孤儿；若接沙箱则 Terminal 内读 auth.json 被拒 | [CR-03](docs/reviews/s12/CR-03-exec-cli.md) S12-CR03-02（交叉复核 High→Medium） | ⚪ |
| S12-F19 沙箱不可用时的回退口径拍板 | S4「绝不静默裸跑」与 ADR-031 可观测回退二选一：改选择器拒跑（显式 `--sandbox off` 除外），或把 ADR-031 写回 design/S4 退出标准并让 CLI/GUI 显示 fallback。验收：探测失败后 run_command 行为与文档一致且 fallback 对用户可见 | [CR-03](docs/reviews/s12/CR-03-exec-cli.md) S12-CR03-05 | ⚪ |
| S12-F20 macOS 协作式取消回收 setsid 后代 | Linux `/proc` 扫树语义抽到 Unix，或 macOS 用 libproc 等价实现。验收：macOS `setsid sleep 300` / `disown` 后台进程在 Ctrl-C 后无残留 | [CR-03](docs/reviews/s12/CR-03-exec-cli.md) S12-CR03-06 | ⚪ |
| S12-F21 Anthropic 认证头覆盖闸门与 HttpClientConfig Debug 脱敏 | `AnthropicProvider::new` 复用 xAI 的固定凭证头拒绝；`HttpClientConfig` Debug 对 header 值脱敏。验收：`fixed_credential_header_is_rejected` 等价用例 + Debug 无明文 | [CR-04](docs/reviews/s12/CR-04-secret-net-mcp.md) S12-CR04-05 | ⚪ |
| S12-F22 失败/取消 run 用量入账 | engine Err 携带累计 usage，或 host 按 run_id 重放已持久化 UsageUpdated 聚合入账（幂等 dedup）。验收：MockProvider 发 usage 后 error/cancel，事件流与 ledger 一致且重试/重放不重复计数 | [CR-05](docs/reviews/s12/CR-05-persistence-ledgers.md) S12-CR05-02（交叉复核 High→Medium） | ⚪ |
| S12-F23 写前快照去重路径 blob 引用计数泄漏 | `checkpoint.rs` 调整检查/put/release 顺序。验收：重复快照后 `ref_count==1`，rollback 后 gc 可回收 | [CR-05](docs/reviews/s12/CR-05-persistence-ledgers.md) S12-CR05-03 | ⚪ |
| S12-F24 engine tool artifacts 承载断链 | 拍板 canonical 承载（扩 `ToolResultContent`/`ToolExecutionCompleted` 或新 AgentEvent 变体）+ golden 先行；与 S12-F49 的 artifact 恢复协同，不顺带修 K-08。验收：MockTool 返回 artifacts golden 断言事件流/tool message 至少一者保留且 replay 可取 | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-01 | ⚪ |
| S12-F25 审批 gate 数组短缺 fail-open | engine 校验 `gates.len() == invocations.len()`，不齐 fail-closed。验收：`request_approval` 返回空数组时工具不执行并以错误/denied 收束 | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-02 | ⚪ |
| S12-F26 Plan Revised 事件无法携带修订内容 | 契约设计（Revised 带内容或以 Replaced 组合）+ golden 先行；收窄 hollow `revise()`；重复 version ID 拒绝。验收：修订 replay golden 与 history/current snapshot 一致 | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-03 | ⚪ |
| S12-F27 MemoryService replay 后 ID 碰撞 | apply 后按事件流派生 next_id 或提供 `from_events`。验收：apply `mem-7` 后 `record()` 不分配 mem-0 且不覆盖 mem-7 | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-04 | ⚪ |
| S12-F28 automation record_result 幂等 | 定义幂等键（可能需要契约决策）+ archived 去重 + failure streak 不重复累计。验收：同 task 重复 `record_result(Failed)` 不新增事件/archived + replay golden | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-05 | ⚪ |
| S12-F29 Supervisor spawn parent 准入校验 | 校验 parent 存在/同 tenant/同 session/状态可派生，失败返回 PolicyDenied。验收：不存在与跨 tenant/session parent 均被拒且不写 children/workers | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-06 | ⚪ |
| S12-F30 Supervisor recover() 语义拍板 | 重建可操作状态（WorkerEntry/children/cancel token + 孤儿 WorkerFailed 事件化），或改名 report-only 并修文档（二选一）。验收：恢复后 registry/cancel-tree/event log 可见终态 | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-07 | ⚪ |
| S12-F31 IdempotencyStore check/record 原子化 | check CAS 占位（inflight/completed）+ record 错误不吞；Automation 通道补连接前缀。验收：并发同 command_id SessionCreate 只建一条（ACP 同 id 重试窄路径回归） | [CR-07](docs/reviews/s12/CR-07-protocol-host-clients.md) S12-CR07-03（交叉复核 High→Medium） | ⚪ |
| S12-F32 SnapshotRequired 附带 Snapshot 契约归一 | 客户端消费附带 Snapshot 或服务端停止附带（二选一），两侧测试断言统一。验收：越窗 Resume 客户端 `ResumeOutcome.snapshot` 符合归一后契约 | [CR-07](docs/reviews/s12/CR-07-protocol-host-clients.md) S12-CR07-04 | ⚪ |
| S12-F33 headless 未映射命令 fail-closed | `command_capability` 未映射返回 UnsupportedCapability；补 Workspace/Terminal/Auth 能力映射或显式拒绝。验收：仅 CompatHistory 握手后发 WorkspaceAdd 被拒 | [CR-07](docs/reviews/s12/CR-07-protocol-host-clients.md) S12-CR07-05 | ⚪ |
| S12-F34 Timeline 抢滚与 Terminal 滚动串扰 | 仅用户位于底部时跟随流式；Terminal 滚动只由 Terminal 输出驱动。验收：流式期间上滚不被拉回；聊天事件到达 Terminal 视口不变 | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-04 | ⚪ |
| S12-F35 Composer 多行高度 | `text_input` 布局/排版支持多行并向上增长（常态 88–94px）。验收：粘贴三行显示三行且高度增长（K-03 项内人工证据） | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-05 | ⚪ |
| S12-F36 All projects 新建 Task 工作目录确认 | 新建流程补工作目录确认/显示；项目身份只来自 canonical workspace_id，不隐式取首项。验收：多 workspace 快照 + All projects 新建出现确认而非静默绑定 | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-06 | ⚪ |
| S12-F37 S10 本机多窗口缺口口径 | 实现应用内多窗口（窗口/会话策略先产品定义），或 ROADMAP/plan 登记延期并修正 S10 完成口径（二选一；False Completion）。验收：两窗 Replay 各自正确或文档修正落地 | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-07 | ⚪ |
| S12-F38 README/AGENTS 状态与结构清单同步 | README 状态表对齐 ROADMAP/v2_plan；结构图补 `agents/`、`control-plane/`、`workflow/`、`schemas/` 与 apps/protocol-probe、clients/sdk+compat；AGENTS.md §3 补三域。验收：rg 状态符号三文档一致、ls 对照结构图 | [CR-09](docs/reviews/s12/CR-09-traceability-consistency.md) S12-CR09-01 | ⚪ |
| S12-F39 plan/S10 与 v2_plan「删 plist」记录失真 | 两处措辞改为真话（行为补齐归 S12-F03）；冒烟 checklist 建议按 action 拆条。验收：文档与源码一致（F03 整改后复核） | [CR-09](docs/reviews/s12/CR-09-traceability-consistency.md) S12-CR09-02 | ⚪ |
| S12-F40 workflow goal/automation/monitor 零消费者 | 补消费面，或在 ROADMAP §4 为三域各登记激活条件/冻结名义。验收：`rg pawork_workflow::(goal|automation|monitor)` 生产命中或 §4 三行登记 | [CR-09](docs/reviews/s12/CR-09-traceability-consistency.md) S12-CR09-03 | ⚪ |

**性能与维护性（Low，S12-F41～F57）**

| 任务 | 说明 | 任务书 / 依据 | 状态 |
| --- | --- | --- | --- |
| S12-F41 pawork-api `plugin` feature 占位 | 加空 `plugin = []` 或改 ROADMAP/design 措辞（二选一）。验收：`cargo metadata --no-deps` feature 列表断言 | [CR-01](docs/reviews/s12/CR-01-manifests-layout.md) S12-CR01-02 | ⚪ |
| S12-F42 protocol 无条件开启 domain typegen | `typegen = ["dep:ts-rs", "pawork-domain/typegen"]` 条件化。验收：`cargo tree -p pawork -i ts-rs` 无结果、`--features typegen` 时出现 | [CR-01](docs/reviews/s12/CR-01-manifests-layout.md) S12-CR01-03 | ⚪ |
| S12-F43 workspace 依赖集中化补齐 | policy/tools/adapters 五处内联版本改 `workspace = true`。验收：`cargo metadata` 依赖解析不变 | [CR-01](docs/reviews/s12/CR-01-manifests-layout.md) S12-CR01-04 | ⚪ |
| S12-F44 control-plane/core tokio 门控口径 | 补 feature 门控或修词典 §4.1 #30 措辞（二选一） | [CR-01](docs/reviews/s12/CR-01-manifests-layout.md) S12-CR01-05 | ⚪ |
| S12-F45 `list_directory` 回传 symlink 绝对目标 | target 相对化，或对越 root 目标改写/省略。验收：`ln -s /etc/passwd link` 后工具输出无宿主绝对路径 | [CR-02](docs/reviews/s12/CR-02-policy-tools-git.md) S12-CR02-05 | ⚪ |
| S12-F46 遗留 JsonlSink 收敛 | 删除或门控，避免再次接入生产路径。验收：无生产调用点 | [CR-03](docs/reviews/s12/CR-03-exec-cli.md) S12-CR03-07 | ⚪ |
| S12-F47 无定价记录币种口径 | 无定价不静默标 USD（cost>0 才要求币种，或显式 unknown/none 附加式表示）。验收：无定价 usage record 不声明 USD，有定价保持原币种 | [CR-05](docs/reviews/s12/CR-05-persistence-ledgers.md) S12-CR05-04 | ⚪ |
| S12-F48 final blob 孤儿回收 | gc/repair 在安全延迟 + 哈希校验后回收 DB 无记录的 final blob。验收：故障注入后 gc/repair 删除孤儿且不误删有记录 blob | [CR-05](docs/reviews/s12/CR-05-persistence-ledgers.md) S12-CR05-05 | ⚪ |
| S12-F49 tool result 分级裁剪接线或登记 | 接线 engine/host 热路径（engine 不得依赖 blob store），或 feature gate + ROADMAP §4 登记；与 S12-F24 协同。验收：超大输出分级裁剪在进 provider 前生效且 artifact 引用可持久化 | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-08 | ⚪ |
| S12-F50 review anchor 路径 canonical 校验 | `safe_path` 做 canonicalize + root 前缀校验。验收：workspace 内 symlink 指向外部文件时 `resolve()`/`reanchor()` 拒绝 | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-09 | ⚪ |
| S12-F51 engine Provider 禁用名单同步 | 名单与首发通道同步，或抽共享测试清单。验收：注入新 Provider 名时该测试失败 | [CR-06](docs/reviews/s12/CR-06-engine-workflow.md) S12-CR06-10 | ⚪ |
| S12-F52 探针认证 scheme 统一 | protocol-probe 改用 `pawork_protocol::client_auth::TOKEN_SCHEME`（随 S12-F10 同批）。验收：错误/正确 scheme 各握手一次行为符合预期 | [CR-07](docs/reviews/s12/CR-07-protocol-host-clients.md) S12-CR07-06 | ⚪ |
| S12-F53 Timeline 事件保真补齐 | live `ToolOutput` 回填、历史 `approval_requested` 条目、`approval_responded` 留痕。验收：live 与历史对同一事件流投影一致 | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-08 | ⚪ |
| S12-F54 RunStatusBar 运行中定时刷新 | 运行中周期性 notify，终态停表。验收：慢 run 静默 10 秒时长仍走动 | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-09 | ⚪ |
| S12-F55 render 避免全量克隆 Timeline | 借阅/迭代渲染；长列表虚拟化可作后续项。验收：5 万条目渲染基准（任务内执行） | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-10 | ⚪ |
| S12-F56 v3 视觉基准漂移收口 | 按基准修正（Inspector 约 440px、折叠形态、Fork 收进条目操作/上下文菜单），或先更新 design/README 与 gui-design 再保留现状（二选一） | [CR-08](docs/reviews/s12/CR-08-desktop-gui.md) S12-CR08-11 | ⚪ |
| S12-F57 HunkStageService 零消费者登记 | ROADMAP §4 登记激活条件（并入 K-04 或独立 GUI 暂存任务），或列入冻结候审。验收：§4 登记行或生产调用点命中 | [CR-09](docs/reviews/s12/CR-09-traceability-consistency.md) S12-CR09-04 | ⚪ |

以上 K-01～K-10 是本次文档核对已确认的**待完善基线**，不是完整 Code Review 结论，也不等同于已确认安全漏洞。S12 新 finding 采用 `S12-Fxx` 独立行追加：只有根因、写入集和验收证据完全相同才合并；Needs Verification 只留在审查报告，不进入本表。

### 3.3 候选（未排期）

候选功能池见 [docs/design.md](docs/design.md) §5（已确认扩展功能族 G1–G7）与 §6（候选功能对照，30 项 P1–P3；2026-08-17 补入 DeepSeek Harness）。候选纳入排期时：在本节 §3.2 登记任务并把内容并入对应 `plan/S*.md`，按 §6 状态回写约定执行。

---

## 4. 未决事项

| 事项 | 说明 | 需要拍板的时点 |
| --- | --- | --- |
| License | crates.io 发布硬前置 | 任何未来发布任务开始前 |
| crates.io 占名 | 是否早期以 0.0.1 空包占位 | 不阻塞开发 |
| 冻结候审资产砍留 | quota 远端 / browser-computer / tool_search（清单见 [docs/v1-migration-reference.md](docs/v1-migration-reference.md) §4.4） | S11 前 |
| V1 目录处置 | 2026-08-17 决议并提前执行：V1 归档至仓库外同级 `../Pawork_v1` 并移出 git 管理；V2 升为仓库根（见 §3.1） | 已闭环 |
| **扩展生态整族（WASM 插件 / 市场 / 用户 Hooks / LSP）** | **移出 S0–S12 排期，待设计与决策。** 必要预留保留：`PluginId`、`ToolCapability::ExternalPlugin`、policy 对 ExternalPlugin 的审批文案、事件/工具注册面不按「无插件」裁剪、`pawork-api` 预留 `plugin` feature 但不激活、GUI 未知 capability 隐藏、resources loader 抽象可供日后 LSP 注入。实现资产见 [plan/archive/S10-extensions-deferred.md](plan/archive/S10-extensions-deferred.md)。需拍板：要不要做、WASM vs 仅 MCP、市场是否运营、Hooks 信任域。 | 不阻塞当前 S7–S12；纳入排期时走 §3.3 |
| OpenCode Go 仅走 `/messages` 的模型 | 是否在 S2 anthropic 适配器中一并覆盖 | S2 计划内决定 |
| `pawork-diagnostics` `experimental` 门控面 | metrics/bundle 两模块已随波 B 迁移但以 `experimental` feature 门控、默认不编译；激活条件：出现真实诊断导出/指标消费方（候选 S10 gui-server 指标 / diagnostic bundle 导出） | S10 |
| `pawork-control-plane` OTel audit exporter | S11 波 A 已迁 `OtelAuditExporter` / `TracingAuditExporter` 类型，无 collector、未接宿主；波 D 生产 audit 走 JSONL。激活条件：出现真实审计导出消费者 | 其后 |
| `pawork-provider-control` `account-control-v1` 生产环 | S11 波 A 已迁 account/routing/health/factory/reconciler（feature 默认开）；lease/binding/pool 始终可用。波 D demo 经 `AcquireRequest` + `InMemoryCredentialPool` 走租约路径，未接 account/routing/health/factory。激活条件：真实多账户 factory 装配 | 其后 |
| `pawork-workflow` `process-exec` | S11 波 B 已迁五合一 reducer；默认纯状态机、不拉 `pawork-exec`。波 D `pawork tasks` 只消费状态机 + `tasks.json`。激活条件：后台任务需要真实 process 时再开 | 其后 |
| `pawork-memory` 生产环 | S11 波 B 已迁 Provider 无关抽象 + Mock 召回；全仓无真实 `EmbeddingProvider`，未接 context/host。crate 默认可测（无 default-off feature）。激活条件：真实 embedder + 宿主置 `memory_available` | 其后 |
| `pawork-review` Forge 实接 | S11 波 B 已迁 re-anchor / resolution / `ForgeAdapter` + Generic 占位；无 GitHub/GitLab 实现，未接会话 diff 流。激活条件：会话内评审接线；真实平台 adapter 另立 | 其后 |
| `pawork-orchestration` teams / 真实双子 run_session | S11 波 D 已接 `pawork agents demo`（Supervisor spawn / cancel-tree / budget-gate；双子 AcquireRequest 指向 `glm-coding`/`glm-4.7` 与 `opencode-go`/`deepseek-v4-flash`）。未接 EventHub sink、`TeamEvent` CLI、两个真实 `run_session` 文件任务。激活条件：需要 teams 面或真实并行子 Agent 循环时另立 | 其后 |
| 对外账户池网关模式（F6-B） | 近期不内建（F6-A 已确认）；以 `pawork-channels` 扩展 feature 长期评估，见 [docs/design.md](docs/design.md) §5 | S12 审查与整改后按需 |
| `plan/archive` M0–M8 正文缺失 | `plan/archive/README.md` 与历史登记引用九份 M0–M8 包级细则，但文件从未落仓；当前以 `docs/v1-migration-reference.md` §4.1 为唯一迁移词典，禁止臆造细则 | 后续文档维护任务；不阻塞 S8 代码收口 |
| 全量门禁、三平台验证与发布如何重排 | 原 S12 Release Hardening 已移出当前排期；历史清单仍可从 [V1 迁移参考 §6.3](docs/v1-migration-reference.md#63-release-hardening-一次性清单原-m8) 取线索，但不得自动执行 | S12 finding 整改完成后，只有用户明确决定发布时另立任务 |

---

## 5. 增量式特有风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 最小实现偏离 V1 语义，后期迁移对不上 | [docs/design.md](docs/design.md) §3.2 契约表：激活即采用 V1 完整形状；engine 等「增量长出」的实现以 V1 测试为准绳逐步替换/并入 |
| 「先简后改」侵蚀冻结契约 | golden 先于消费实现迁移；`--json` 等未定型输出显式标注 unstable |
| 真实 API 波动导致验收不稳定 | 冒烟（人工）与自动化（Mock/golden）分离；真实 API 测试 env 门控、不进默认测试路径 |
| env 注入 key 的过渡机制被长期留存 | S6 退出标准包含「仓库外 auth 文件为主、env 降级为 fallback 且行为有回归测试」 |
| 早期包数量多、单包极薄带来的维护噪音 | 薄包只含终局布局中必然存在的包；不为增量新造任何临时包 |
| 双线漂移（V1 继续演进） | V1 冻结为只收安全修复（沿用旧计划约定）；新功能一律在 V2 做 |
| GUI 实现偏离 v3 定稿视觉/交互 | [design/README.md](design/README.md) 为验收基准；有意差异先更新设计文档再改代码；1440×1024 对照验收 + 1080×720 可用性验证 |
| 低消耗默认模型能力不足，掩盖高级功能缺陷 | §1.1 矩阵只承担常规冒烟/回归与接通验证；高级功能由用户指定高级模型专项评估（§1.1 例外②），评估记录按 [docs/task-guide.md](docs/task-guide.md) §8 留档 |
| 全项目审查范围过大，退化为无证据清单 | S12 固定拆为 CR-01～CR-09；每包列出实际覆盖与未覆盖路径，finding 必须带路径/符号/行号、置信度和后续验证建议；S12 内不边审边改 |

---

## 6. 状态回写约定

- **阶段任务**：阶段收尾时更新 §2 总览表状态列 + 对应 `plan/S*.md` 冒烟清单与退出标准打勾；experimental / 延期项在 §4 登记激活条件。开发期不做逐任务文档同步。
- **阶段外任务**：开启 / 完成时更新 §3.2 状态列；完成后移入 §3.1 并登记产出链接。
- **S12 finding**：每个审查包先写报告；Confirmed finding 以独立 `S12-Fxx` 任务追加到 §3.2，需决策项进 §4，候选能力进 §3.3。S12 只登记，不实现、不测试、不发布。
- **候选转正**：候选功能纳入排期时按 §3.3 流程登记。
- **模型评估记录**：注明所用通道与模型；默认应属 §1.1 矩阵，例外须写明属于接通验证还是用户指定的高级模型评估。
- S0–S11 的完整收尾清单（测试、冒烟、评估记录、报告格式）见 [docs/task-guide.md](docs/task-guide.md) §8；S12 例外以 [审查任务书](plan/S12-project-code-review.md) 的退出标准为准。
