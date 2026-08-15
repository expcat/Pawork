# Pawork V2 开发路线图（增量式 · S0–S12）

> 本文档是 Pawork V2 的**任务总索引**：登记全部任务（已完成 / 未完成）的状态与粗略介绍，并链接到 [plan/](plan/) 内的详细任务文档。V2 采用「最小可用 → 逐级追加」的增量开发方式：S0 先交付一个能真实对话的 `pawork` CLI，此后每个阶段都在可运行的二进制上追加一层能力，阶段末尾用真实 API key 冒烟 + 定向自动化测试双重验收。
>
> **文档体系**（五份常设文档 + 两类附件）：
>
> | 文档 | 职责 |
> | --- | --- |
> | 本文 `ROADMAP.md` | 任务总索引：阶段状态、阶段外任务、未决事项、风险 |
> | [plan/S0–S12](plan/) | 每阶段任务书：目标、涉及包与 V1 资产、关键任务、冒烟与自动化验收、退出标准、并行拆分（附件 [plan/archive/](plan/archive/README.md)：已归档的旧按域计划 M0–M8，保留包级迁移细则） |
> | [docs/design.md](docs/design.md) | 设计文档：包布局与激活映射、冻结契约、各阶段功能设计与参照项目映射、候选功能 |
> | [docs/references.md](docs/references.md) | 参照项目手册：对标项目的目标、功能与文档链接（附件 [docs/research/](docs/research/)：专题调研全文） |
> | [docs/task-guide.md](docs/task-guide.md) | 任务实现规范：任务开启 / 进行 / 收尾的公共约定与最小启动提示词 |
> | [docs/v1-migration-reference.md](docs/v1-migration-reference.md) | V1 全量 Review 结论与 V1→V2 迁移词典（原 ROADMAP_V2.md，冻结参考） |
>
> 工作约定见 [../AGENTS.md](../AGENTS.md)（V2 叠加生效；V2 开发期放宽项见 [docs/task-guide.md](docs/task-guide.md) §6）。

---

## 1. 计划原则

旧「按域整体迁移」计划（M0–M8，已归档至 [plan/archive/](plan/archive/README.md)）第一个可运行物要到第 5 个里程碑才出现，此前全部是「库先行、零消费者」——正是 V1「组件齐全、主干未通电」病灶（[docs/v1-migration-reference.md](docs/v1-migration-reference.md) §1.2）在计划层的重演。现行计划的三条组织原则：

1. **每阶段交付可运行增量**：从 S0 起 `pawork` 二进制始终可编译、可运行、可被真实使用；每个阶段以「新增哪些用户可见能力」定义，而不是以「迁移了哪些包」定义。
2. **真实测试优先**：初期用两条真实通道（GLM Coding Plan、OpenCode Go，见 [docs/task-guide.md](docs/task-guide.md) §5）做每阶段冒烟与模型行为评估；自动化测试只做关键定向项（契约 golden、安全红线、解析器种子），开发期不设门禁（沿用 [docs/v1-migration-reference.md](docs/v1-migration-reference.md) §6.2，全量门禁集中在 S12）。
3. **追加而非重写**：用三道保险（终局包布局先行、冻结契约先行、迁移词典与「无消费者不合入」，见 [docs/design.md](docs/design.md) §3）保证后期把 V1 全部功能追加进来时，不需要推翻任何已交付阶段的代码。V1 的约 23.6 万行资产仍按「复制 + 合并 + 改名」迁移，只是从「按域一次性搬」改为「按阶段按需搬」。

---

## 2. 阶段总览（S0–S12）

状态符号：⚪未开始 · 🔵进行中 · 🟢已完成 · ⚠️阻塞。每阶段的详细任务、退出标准与并行拆分见 `plan/S*.md`；各阶段功能设计与参照项目映射见 [docs/design.md](docs/design.md) §4。

| 阶段 | 主题 | 新增用户可见能力 | 激活 / 增强的包 | 真实验收要点 | 状态 |
| --- | --- | --- | --- | --- | --- |
| [S0](plan/S0-minimal-chat.md) | 最小可对话 CLI | `pawork chat` 流式多轮对话、Ctrl-C 取消、`pawork models`、TOML 配置 + env key | workspace 根、domain（最小）、api（provider）、net、providers/adapters（openai-compatible）、config（最小）、engine（最小）、app（最小）、cli（最小）、apps/pawork | 两把真实 key 各完成流式多轮对话；401/429/超时可读呈现 | 🟢 |
| [S1](plan/S1-sessions.md) | 会话持久化与恢复 | 会话落盘、`pawork sessions list/show`、`--resume` 续聊、`--json` 事件流输出 | sqlite、session（核心）、domain（events 全量）、engine（事件化 + appender） | 中断/杀进程后 resume 续聊上下文连续；envelope golden 与 append-only 契约生效 | 🟢 |
| [S2](plan/S2-tool-loop.md) | Agent Loop 与只读工具 | Agent 自主调用 read/list/search/find 回答仓库问题；`@`引用前身（相对路径语义） | api（tool）、tools（只读四件）、workspace（roots）、engine（工具循环）、providers/adapters（anthropic-messages）、testkit（MockProvider） | 真实仓库问答任务；OpenAI/Anthropic 双协议 tool-calling 对比评估 | 🟢 |
| [S3](plan/S3-safe-edits.md) | 写入工具与审批 | write/edit/apply_patch + 终端审批交互（`--approval-mode`） | policy（整包）、tools（写三件）、engine/cli（审批位点） | 真实小编码任务经审批落盘；越界/symlink 拒绝；deny 后会话可续 | 🟢 |
| [S4](plan/S4-exec-sandbox.md) | 命令执行与沙箱 | run_command（进程树清理 + 沙箱 + 输出截断）——首个完整「读-改-跑」编码闭环 | exec（process/sandbox）、tools（run_command）、policy（shell 分类接线） | 「跑 cargo check 并修复报错」端到端；Ctrl-C 杀整棵进程树；fail-closed | 🟢 |
| [S5](plan/S5-context-usage.md) | 上下文预算与用量 | 长任务不炸上下文（预算/截断/压缩）、token 与费用统计显示 | engine（context 接线）、session（compaction feature）、provider-core（usage/registry/pricing） | 超长多轮任务连贯完成；token 计量与厂商侧抽查一致 | 🔵 |
| [S6](plan/S6-providers-auth.md) | 多 Provider 与认证 | 全厂商适配、`pawork models` 聚合、OS Keychain 存 key、OAuth | providers/adapters（8 厂商正式化）、auth、diagnostics（脱敏 layer）、config（凭证解析） | key 入 Keychain 后正常使用；运行中切换 provider/model；secret 不入日志回归 | ⚪ |
| [S7](plan/S7-git-checkpoint.md) | Git、Diff 与 Checkpoint | 会话改动 diff 呈现、编辑前快照、`pawork rollback` 一键回滚 | git（git+diff）、blob-store（artifact/protected/checkpoint） | 真实任务后 diff 审阅 + 回滚还原；git 注入防护回归 | ⚪ |
| [S8](plan/S8-mcp-resources.md) | MCP、资源与兼容导入 | 外接 MCP server 工具、AGENTS.md/Skills 生效、`@file` 引用、导入 Claude/Codex 等配置 | mcp（rmcp 收口）、resources、compat、workspace（file-index）、config（完整层级） | 真实 MCP server 工具与内置共存；本机 Claude/Codex 配置导入可用 | ⚪ |
| [S9](plan/S9-serve-clients.md) | 服务化与客户端 | `pawork headless/gui serve/acp serve/service`、SDK 编程驱动、GUI 多客户端 + 断线 Replay、会话分支/Fork | protocol、transport、gui-server、sdk、client、channels、app/cli（正式化）、exec（pty）、session（lifecycle/Fork）、protocol-probe | protocol-probe 自检全过；SDK e2e；acp 接真实客户端 | ⚪ |
| [S10](plan/S10-extensions.md) | 扩展生态 | WASM 插件安装→注册→撤销、市场、用户 Hooks、LSP 语义工具 | wasm-host、plugin（+market）、hooks、lsp | 示例插件闭环；pre-tool hook 短路生效；LSP 查询作为工具 | ⚪ |
| [S11](plan/S11-workflow-control.md) | 工作流、多 Agent 与控制面 | Plan 审批 gate、后台任务、`pawork usage` 配额、多 Agent 编排、多账户池与路由 | workflow、memory、review、orchestration、control-plane、provider-control、quota | 多 Agent demo（两通道各驱动一个子 Agent）；plan gate 拦截；用量可查 | ⚪ |
| [S12](plan/S12-release-hardening.md) | Release Hardening 与发布 | —（验证 + 发布 + 归档） | 全部 | 全量门禁/三平台/fuzz/依赖卫生全绿；W1–W4 波次发布；V1 归档 | ⚪ |

**关键节点**：S4 结束即达成旧计划 M4 的首要验收（真实仓库「读文件-改代码-跑命令」闭环），但路径上每一步（S0–S3）都已是可测可用的工具。S5–S8 把单机 CLI 补齐为完整 Coding Agent；S9–S11 横向扩展为多客户端、可编排系统；S12 收口发布。

**依赖关系**：S0→S1→S2→S3→S4 严格串行（主干长成）；S5、S6、S7 之间无包级交叉，S4 后可并行推进；S8 依赖 S2（工具注册面）与 S6（config 完整化）；S9 依赖 S1–S5 稳定；S10 依赖 S8（mcp）与 S9（app 注册入口正式化）；S11 依赖 S9；S12 依赖全部。

---

## 3. 阶段外任务登记

### 3.1 已完成

| 任务 | 完成日期 | 产出 |
| --- | --- | --- |
| V1 全量 Review 与 V2 重构方案（原 ROADMAP_V2.md） | 2026-08-14 | [docs/v1-migration-reference.md](docs/v1-migration-reference.md)（Review 结论、目录结构、映射总表、发布与测试策略） |
| 按域迁移计划 M0–M8 撰写（后被增量式取代） | 2026-08-14 | [plan/archive/](plan/archive/README.md)（保留包级迁移细则，供各阶段引用） |
| 重规划为增量式阶段计划 S0–S12 | 2026-08-14 | 本文 §2 + [plan/S0–S12](plan/) |
| 多账户额度/切换/子 Agent 路由/输入缓存调研与方案确认（G1–G7 → F1–F6，决策 D1–D8 全部确认） | 2026-08-14 | [docs/research/](docs/research/) 三篇；候选登记 [docs/design.md](docs/design.md) §5 |
| 文档体系整合（五文档结构：索引 / 任务书 / 设计 / 参照 / 规范） | 2026-08-14 | 本文 + [docs/design.md](docs/design.md) + [docs/references.md](docs/references.md) + [docs/task-guide.md](docs/task-guide.md) + [docs/v1-migration-reference.md](docs/v1-migration-reference.md) |

### 3.2 待执行（阶段之外）

| 任务 | 说明 | 任务书 / 依据 | 状态 |
| --- | --- | --- | --- |
| 多账户功能族并入 plan | 把已确认的 F1–F5 与 G6 增量写入 S2/S5/S6/S8/S11/S12 计划文档，并按「少测试」约定核减非关键测试项 | [docs/research/multi-account-quota-plan-merge.md](docs/research/multi-account-quota-plan-merge.md) §4（前置条件已满足，可随时开启） | ⚪ |

### 3.3 候选（未排期）

候选功能池见 [docs/design.md](docs/design.md) §5（已确认扩展功能族 G1–G7）与 §6（候选功能对照，28 项 P1–P3）。候选纳入排期时：在本节 §3.2 登记任务并把内容并入对应 `plan/S*.md`，按 §6 状态回写约定执行。

---

## 4. 未决事项

| 事项 | 说明 | 需要拍板的时点 |
| --- | --- | --- |
| License | crates.io 发布硬前置 | S12 前 |
| crates.io 占名 | 是否早期以 0.0.1 空包占位 | 不阻塞开发 |
| 冻结候审资产砍留 | quota 远端 / browser-computer / tool_search（清单见 [docs/v1-migration-reference.md](docs/v1-migration-reference.md) §4.4） | S11 前 |
| V1 目录处置 | 归档分支 / tag | S12 |
| GPUI Desktop（apps/desktop） | 消费 `pawork-client`，建议 S12 后独立启动 | 不阻塞 |
| OpenCode Go 仅走 `/messages` 的模型 | 是否在 S2 anthropic 适配器中一并覆盖 | S2 计划内决定 |
| 审批等待前未落盘 `ToolApprovalRequested` | engine 在 `host.decide` 返回后才成对发出 Requested/Responded；`kill -9` 时 tool_call 停在 `collecting_arguments`，resume 走重新询问（S3 冒烟已确认不重复执行）。若要让 seal-as-denied 覆盖生产杀进程窗口，需在等待前先 persist Requested | 不阻塞 S4；S9 headless 远程审批前建议收口 |
| `plan/archive/` M0–M8 正文缺失 | README 与各 `plan/S*.md` 仍链向包级细则，磁盘上仅有 `archive/README.md`；执行时改引迁移词典 + V1 源码 | 不阻塞；补档或改链可随时做 |
| 对外账户池网关模式（F6-B） | 近期不内建（F6-A 已确认）；以 `pawork-channels` 扩展 feature 长期评估，见 [docs/design.md](docs/design.md) §5 | S12 后按需 |

---

## 5. 增量式特有风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 最小实现偏离 V1 语义，后期迁移对不上 | [docs/design.md](docs/design.md) §3.2 契约表：激活即采用 V1 完整形状；engine 等「增量长出」的实现以 V1 测试为准绳逐步替换/并入 |
| 「先简后改」侵蚀冻结契约 | golden 先于消费实现迁移；`--json` 等未定型输出显式标注 unstable |
| 真实 API 波动导致验收不稳定 | 冒烟（人工）与自动化（Mock/golden）分离；真实 API 测试 env 门控、不进默认测试路径 |
| env 注入 key 的过渡机制被长期留存 | S6 退出标准包含「Keychain 为主、env 降级为 fallback 且行为有回归测试」 |
| 早期包数量多、单包极薄带来的维护噪音 | 薄包只含终局布局中必然存在的包；不为增量新造任何临时包 |
| 双线漂移（V1 继续演进） | V1 冻结为只收安全修复（沿用旧计划约定）；新功能一律在 V2 做 |

---

## 6. 状态回写约定

- **阶段任务**：阶段收尾时更新 §2 总览表状态列 + 对应 `plan/S*.md` 冒烟清单与退出标准打勾；experimental / 延期项在 §4 登记激活条件。开发期不做逐任务文档同步。
- **阶段外任务**：开启 / 完成时更新 §3.2 状态列；完成后移入 §3.1 并登记产出链接。
- **候选转正**：候选功能纳入排期时按 §3.3 流程登记。
- 完整收尾清单（测试、冒烟、评估记录、报告格式）见 [docs/task-guide.md](docs/task-guide.md) §8。
