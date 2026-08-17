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
| [S11](plan/S11-workflow-control.md) | 工作流、多 Agent 与控制面（波 A ✅ · 波 B ✅ · 波 C ✅） | Plan 审批 gate、后台任务、`pawork usage` 配额、多 Agent 编排、多账户池与路由；GUI Workflow 面 | workflow、memory、review、orchestration、control-plane、provider-control、quota | Workflow 面、quota 完整面、ActivityPopover Agent 状态列表 | 多 Agent demo（§1.1 矩阵中两通道各驱动一个子 Agent）；plan gate 拦截；用量可查 | 🔵 |
| [S12](plan/S12-project-code-review.md) | 全项目 Code Review 与整改拆分 | —（只读审查 + finding 任务化） | 全部现有包及跨包接口 | 审查 v3 设计、投影、协议能力与现有证据的一致性；不改 UI、不启动窗口 | CR-01～CR-09 独立报告完成；安全/Bug/性能/假完成/未落地需求均有证据与置信度；Confirmed finding 逐项写入 §3.2 | ⚪ |

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
| `pawork-control-plane` OTel audit exporter | S11 波 A 已迁 `OtelAuditExporter` / `TracingAuditExporter` 类型，无 collector、未接宿主；激活条件：出现真实审计导出消费者 | S11 波 D / 其后 |
| `pawork-provider-control` `account-control-v1` 生产环 | S11 波 A 已迁 account/routing/health/factory/reconciler（feature 默认开，与 V1 同名）；lease/binding/pool 始终可用。生产调用点 0；激活条件：波 D host 经 Provider factory 消费 lease/binding | S11 波 D |
| `pawork-workflow` `process-exec` | S11 波 B 已迁五合一 reducer；默认纯状态机、不拉 `pawork-exec`。`start_process` / `OutputEvent` / `Sandbox` 仅 `--features process-exec`。激活条件：波 D 后台任务 CLI 需要真实 process 时再开 | S11 波 D |
| `pawork-memory` 生产环 | S11 波 B 已迁 Provider 无关抽象 + Mock 召回；全仓无真实 `EmbeddingProvider`，未接 context/host。crate 默认可测（无 default-off feature）。激活条件：真实 embedder + 宿主置 `memory_available` | S11 波 D / 其后 |
| `pawork-review` Forge 实接 | S11 波 B 已迁 re-anchor / resolution / `ForgeAdapter` + Generic 占位；无 GitHub/GitLab 实现，未接会话 diff 流。激活条件：波 D 会话内评审接线；真实平台 adapter 另立 | S11 波 D / 其后 |
| `pawork-orchestration` 生产环 | S11 波 C 已迁 supervisor 五模块 + teams；`host/app` / CLI / EventHub sink / 双通道 demo 均未接。crate 默认可测；真实 git 仅 `--features git`。激活条件：波 D 宿主装配 `AgentSupervisor`（Mock ledger budget-gate + cancel-tree）并跑多 Agent demo | S11 波 D |
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
