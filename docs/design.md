# Pawork 设计文档

> 本文档是**功能设计事实源**：目标与原则、各功能域设计及其到参照项目的映射、已确认扩展功能族（G1–G7）与候选功能池、发布策略。包布局、依赖方向、冻结契约与架构红线见 [architecture.md](architecture.md)；包内实现细节见 [包级 Spec](spec/README.md)；Desktop GUI 设计见 [gui-design.md](gui-design.md)；参照项目手册与调研附录见 [references.md](references.md)；V1/V2/V3 历史沿革见 [history.md](history.md)。
>
> **状态（2026-08-25）**：既有功能与结构阶段已归档；当前主线为新 R1–R8 Desktop UI 99% 还原与全功能模拟操作验收，剩余非 UI 工作顺延 R9–R10，R11 为 UI 终局比对与优化文档（见 [../ROADMAP.md](../ROADMAP.md)）。§2 的功能映射按 V2 阶段（S0–S13）组织，是「功能 ↔ 参照项目」的持续事实源，不代表现行包布局（现行布局见 [architecture.md](architecture.md) §2）。

---

## 1. 目标与原则

1. Pawork 是纯 Rust 的 CLI Coding Agent + 独立 GPUI Desktop：`pawork` 二进制内置 Core（引擎、工具、Provider、存储、策略），Desktop 经 GUI Connection Protocol 连接 CLI。V2 已把 V1 的 88 crate / 约 23.6 万行重组交付为可用产品；V3 R1 后布局定稿 21 成员。
2. **纵向优先**：先交付内置工具真实接线、能在真实仓库完成编码任务的 CLI Coding Agent，再长出最小 Agent GUI，其后按同一窗口增量加面；WASM 插件等扩展生态不在当前排期（候选见 §4–§5 与 [../ROADMAP.md](../ROADMAP.md) 候选池）。
3. **架构红线不变**：纯 Rust、CLI 与 Core 同进程同二进制、GUI 独立进程走协议、canonical domain 纯净、事件可持久化可重放、Secret 不落库不入日志、Engine 无 Provider 名称特例分支、禁止循环依赖（全文见 [architecture.md](architecture.md) §1）。
4. **无消费者不合入**：任何模块必须有真实装配点；零消费者代码归档（git tag `v2-final` 兜底），复活条件登记 [../ROADMAP.md](../ROADMAP.md) 候选池。
5. **少测试、无全量门禁**：验证纪律见 [../ROADMAP.md](../ROADMAP.md) 任务约定章节；三类关键测试（安全红线、持久化/重放 golden、协议 golden）不推迟。

---

## 2. 功能设计与参照项目映射

> 本节按 V2 阶段（S0–S13，均已交付）记录**用户可见功能**与参照映射，是「功能 ↔ 参照项目」的持续事实源；交付细节与阶段史见 [history.md](history.md)。「参照」列给出该功能在参照项目中的对应实现与资料入口——项目背景见 [references.md](references.md)，**参照项目 → 功能规划**的反向分类见同文 §6，机制细节见其附录 A（记作 research §N）。

### S0 最小可对话 CLI

| 功能 | 参照 |
| --- | --- |
| `pawork chat` 流式多轮 REPL、Ctrl-C 取消当轮 | [Codex CLI](https://github.com/openai/codex)（[产品文档](https://developers.openai.com/codex)）；OpenCode/Pi 的终端交互语义（仅对标行为——Pawork 无 TUI，见 §4.1 红线排除） |
| `pawork models` 模型目录 | OpenCode 外置 [models.dev](https://models.dev) 注册表 vs Pi 自维护内置目录（research §2.2 对比表）——Pawork 走 registry + config 覆盖 |
| TOML 配置 + env key（配置**无 api_key 字段**） | OpenCode `opencode.json` 与 `auth.json` 分离（research §2.1）；Pi `auth.json`（0600）与 `!command`/`$ENV` 插值（research §2.2） |
| openai-compatible 适配器（可配 `base_url`） | GLM Coding Plan / OpenCode Go / 自建网关（opencodex、[Codex Router](https://github.com/duolahypercho/codex-router) 等）均为此形态 |
| 可读错误呈现（401/429/超时/断网） | OpenCode ≤5 次重试、遵循 Retry-After（research §2.1）；Pi agent 层退避（research §2.2） |

### S1 会话持久化与恢复

| 功能 | 参照 |
| --- | --- |
| 事件流落盘（`AgentEventEnvelope` + append-only 存储） | 冻结契约（[architecture.md](architecture.md) §3.2）；最接近的外部同形：DeepSeek Harness 仅追加 `SessionEvent` 日志（模型可见输入必须可从日志重建，fork/resume/Trajectory 同源，[references.md](references.md) §2.4）；相邻实现：Pi JSONL 树形 session（research §2.2）、OpenCode 消息级 SQLite 落库（research §2.1） |
| `pawork sessions list/show`、`--resume` 续聊 | [Codex](https://github.com/openai/codex) sessions/resume；OpenCode/Pi 会话恢复；DeepSeek Harness 从同一事件流 resume |
| `pawork run`（非交互单次）+ `--json` JSONL 事件流 | Codex exec / headless 输出形态；DeepSeek Harness `dsh-headless` + JSONL session；`--json` → 正式 headless 映射见 [spec/contracts.md](spec/contracts.md) |

### S2 Agent Loop 与只读工具

| 功能 | 参照 |
| --- | --- |
| 只读四工具 read_file/list_directory/search_text/find_files | [OpenCode](https://opencode.ai/docs/) 内置工具族；Codex 工具面 |
| 引擎多轮工具循环（每 run 轮数上限防失控） | OpenCode agent `steps` 上限（research §2.1）；协议中立红线：工具映射在 adapter 侧完成，engine 零厂商分支 |
| OpenAI tools / Anthropic tool_use 双协议 | OpenAI 与 Anthropic 官方 API（缓存与协议文档入口见 [references.md](references.md) §4）；Pi `anthropic-messages` 实现（research §2.2） |
| workspace roots + `workspace_id + relative_path` 输入红线 | tool-api 类型化路径红线；OpenCode permission 边界（[agents 文档](https://opencode.ai/docs/agents/)） |
| MockProvider 确定性测试 | 工程实践（testkit） |
| F5 canonical 缓存注解占位（§3 G5） | — |

### S3 写入工具与审批

| 功能 | 参照 |
| --- | --- |
| write_file/edit_file/apply_patch 写三件 | OpenCode edit/write/patch 工具；Codex apply_patch |
| 终端审批（一次/本运行/拒绝）+ `--approval-mode` 五档（默认 ReadOnly；旧 `on-failure` 仅兼容读入并映射 NeverAsk） | [Codex approval modes](https://github.com/openai/codex)；OpenCode `permission`（research §2.1）；DeepSeek Harness 把 `sandbox/mode` 与 `approval/policy` 做成独立 knob，再经 permission preset 捆绑（[权限预设](https://deepseek-harness.github.io/deepseek-harness/en/reference/subsystems/permission-presets)）；policy 契约见 [architecture.md](architecture.md) §3.2 |
| 未信任 workspace 强制询问 | Pi Project Trust（[earendil-works/pi](https://github.com/earendil-works/pi)） |
| 路径越界/symlink/TOCTOU 红线 + 提示注入回归 | V1 安全红线资产（policy 整包随迁） |

### S4 命令执行与沙箱

| 功能 | 参照 |
| --- | --- |
| run_command + 沙箱（AppContainer/Landlock/Seatbelt）+ fail-closed（ADR-031：**可观测回退**，不是拒跑；CLI/GUI 必须展示 fallback） | [Codex](https://github.com/openai/codex) sandbox（Landlock/Seatbelt 路线）；DeepSeek Harness `ctx.sandbox` 与审批分轨；V1 exec 链（Windows Job Object + AppContainer）；[ADR-031](../../Pawork_v1/docs/adr/ADR-031-sandbox-backend-architecture.md)（归档）· [ADR-037](adr/ADR-037-s13-wave-b-contracts.md)；R7 演进见 [adr/ADR-041](adr/ADR-041-sandbox-trust-model.md) |
| shell 风险分类 → 审批（Dangerous 必询） | V1 policy `shell` 分类；OpenCode `permission.bash` 语义 |
| 取消 = 清理整棵进程树 | V1 `cancel.rs` + 进程树管理（Job Object/进程组） |
| 输出截断 + 完整输出落工件 | 上下文预算纪律；对照 research §5.3 前缀稳定技巧 |

### S5 上下文预算与用量

| 功能 | 参照 |
| --- | --- |
| 上下文预算（软限压缩 / 硬限截断）+ `/compact` 手动触发 | OpenCode context overflow 自动 compaction（research §2.1）；**compaction=重写前缀=缓存全失效**的折中纪律（research §5.3）；Codex Router 可选旧工具结果老化与外部模型 continuation 摘要 |
| token 与费用统计（micros 定价、无定价不编造） | OpenCode 消息级 cost/tokens 落库（research §2.1）；Pi footer 实时命中率与成本（research §2.2）；LiteLLM 缓存差价计费（research §4.2） |
| 模型 registry（context window / 定价 / 别名） | [models.dev](https://models.dev)（OpenCode 路线）；Pi `models-store.json` + `models.json` 扩展（research §2.2） |
| F5 前缀稳定性分段产出、缓存用量并入统计（§3 G5） | — |

### S6 多 Provider 与认证

| 功能 | 参照 |
| --- | --- |
| 六条首发通道：ChatGPT OAuth、xAI Grok OAuth、Z.AI GLM Coding Plan、OpenCode Go、Qwen Token Plan、DeepSeek | 各厂商官方 API；端点/凭证形态对照：[Codex Router](https://github.com/duolahypercho/codex-router) 注册表；通道端点与凭证矩阵见 [../ROADMAP.md](../ROADMAP.md) 任务约定章节 |
| ChatGPT/xAI 共用 Responses transport；xAI 与 API-key 混合通道按模型 capability 选 Chat/Responses | canonical 保持 provider-neutral，Engine 不按厂商名分支 |
| `auth.json` 文件凭证 + `pawork auth` 子命令 | 形态对齐 Codex CLI；Pawork 额外锁定 0600、跨进程 write/refresh 锁、原子写、损坏 fail-closed、掩码展示与全链日志脱敏。env 仅作 headless/CI fallback |
| ChatGPT/xAI OAuth（PKCE/Device/refresh/callback） | Codex Sign in with ChatGPT；OAuth client secret 不进入 adapter/仓库 |
| REPL `/model` `/provider` 切换（事件流记录变更） | OpenCode `/models` 切换 + transform 归一化历史（research §2.1）；Pi 跨厂商 handoff 一等能力（research §2.2） |
| Z.AI GLM Coding Plan 端点预设 | 国际站 Coding Plan 专属端点 `https://api.z.ai/api/coding/paas/v4` |
| plan 凭证 kind（D2）、adapter 缓存映射 + registry 能力表、F2-B 被动配额信号 per-adapter 登记（§3） | — |

### S7 最小 Agent GUI（[设计](gui-design.md)）

| 功能 | 参照 |
| --- | --- |
| 先锁定最小 Agent 信息架构，再实现本机单窗口 | [gui-design.md](gui-design.md)；Codex Desktop 主对话壳；OpenCode Desktop/Web 的流式+工具行；DeepSeek Harness Web UI 的 Trajectory（默认壳不吸收） |
| `pawork gui serve` + GPUI Desktop：会话 / Timeline / Composer / 取消 / 模型切换 / 时间线内审批 | 独立进程 + GUI Connection Protocol（ADR-022/023/035，V1 归档）；协议帧完整形状、GUI 只消费对话子集 |

### S8 Git、Diff 与 Checkpoint

| 功能 | 参照 |
| --- | --- |
| `pawork diff` 结构化 diff（分页、CRLF/中文文件名） | V1 diff-service（unified diff 状态机 parser）；IDE in-place review 为候选形态（§4.5 D1） |
| 写前 checkpoint + `pawork rollback` | OpenCode `/undo` `/redo`（turn 级、经 Git——与 Pawork Run/Tool 级快照的粒度对比见 §4.2 A3）；V1 checkpoint-service |
| git 状态感知（status/branch/worktree）+ 注入防护 | V1 git-service（`validate_position_arg` 等防御随迁） |
| 审批 UX 升级为 diff 预览 | S3 预留升级点的兑现 |

### S9 MCP、资源与兼容导入

| 功能 | 参照 |
| --- | --- |
| MCP client（rmcp 收口）+ 与内置工具共存注册 | [MCP 官方](https://modelcontextprotocol.io)；「Pawork 作为 MCP server」为候选反向形态（§4.3 B7） |
| AGENTS.md / Skills / profiles 加载注入 | [AGENTS.md 开放约定](https://agents.md)；OpenCode rules、Codex AGENTS.md；DeepSeek Harness `tool-skill` + agent preset；Skills 对标 Claude/Cursor 的 SKILL.md 机制 |
| `@file` 引用 + file-index 模糊补全 | 各家 `@` 语义；OpenCode References（工作区外引用）为候选扩展（§4.3 B4） |
| 一键导入本机 Claude/Codex/Grok/Cursor/Pi 配置（只读） | 各工具本机配置布局；账户/端点导入源（G6）：cc-switch SQLite SSOT（research §3.2）、CLIProxyAPI auth-dir（research §3.3）、opencodex config（research §3.1）、Codex Router 托管 config 块（[references.md](references.md) §3.2） |
| config 完整六层 + Profile | V1 config-service 层级合并引擎 |
| G6 账户/端点导入源、F4 Agent Profile 绑定字段随 profiles 契约定型（§3） | — |

### S10 服务化与客户端补齐

| 功能 | 参照 |
| --- | --- |
| `pawork headless --json-stdio` + SDK 编程驱动 | [Codex](https://github.com/openai/codex) TS/Python SDK 与 app-server；OpenCode SDK/serve（[opencode.ai/docs](https://opencode.ai/docs/)）；Pi SDK `createAgentSession()`（research §2.2）；DeepSeek Harness headless + [Python SDK](https://deepseek-harness.github.io/deepseek-harness/en/guide/python-sdk) |
| `gui serve` 多客户端 + 断线 Replay + 慢客户端隔离 | V1 gui-server 资产；Desktop 增量见 [gui-design.md](gui-design.md) §5 |
| `pawork acp serve` 接入 ACP 编辑器 | [Agent Client Protocol](https://github.com/zed-industries/agent-client-protocol)（Zed 生态） |
| 会话分支 / `pawork session fork`（仅闭合 turn 后稳定事件） | Pi session tree/clone（research §2.2）；OpenCode 子 session（research §2.1）；DeepSeek Harness `ctx.sessions.fork`；R6 原生化见 [adr/ADR-040](adr/ADR-040-session-branch-lineage.md) |
| `pawork service install/start/stop` + 运维子命令（status/watch/shutdown/doctor） | V1 六运行模式（外部无直接对标） |
| PTY 交互式命令 + GUI Terminal | V1 pty-service（PTY 重连语义）；DeepSeek Harness `tool-terminal` + 持久 bash |

### S11 工作流、多 Agent 与控制面

| 功能 | 参照 |
| --- | --- |
| Plan 审批 gate（未批准整版拦截 turn；host 在 `run_session` 前校验，无 plan 放行） | V1 plan-service；相邻机制：OpenCode question/todowrite、DeepSeek Harness planning / `tool-todo` / `ctx.goals` 为模型侧轻量形态（候选 §4.3 B2/B3/B9） |
| 多 Agent 编排（spawn/registry/cancel-tree/recovery/budget-gate） | OpenCode `task` 子代理 + 权限派生 + `subagent_depth`（research §2.1）；Pi「核心不内置子代理」哲学（research §2.2）；DeepSeek Harness `tool-subagent` + workflows；CCR in-band 标签为**明确不采纳**的反例（research §4.1） |
| 子 Agent 声明式 provider/model/账户绑定 + 预算分配（F4） | opencode `agent.model` 声明式绑定（research §2.1）；方案见 [references.md](references.md) 附录 B（F4-A+B） |
| 多账户池 / 租约 / 路由 / 会话-账户亲和（F1/F3） | opencodex 账户池 + 三窗口配额 + thread affinity（research §3.1）；CLIProxyAPI RR/加权/fill-first + 冷却 + session-affinity（research §3.3）；claude-relay-service 内容 hash sticky（research §4.4）；Codex Router 仅额度耗尽换**模型**（[references.md](references.md) §3.2） |
| 额度感知与预算 gate（F2）+ `pawork usage` | opencodex 主动配额窗口探测（research §3.1）；LiteLLM 层级预算（research §4.2）；V1 quota-service/usage-ledger |
| audit / tenant 控制面 | LiteLLM org/team/user/key 层级（research §4.2）；`dedup_key`/audit JSONL 冻结契约（[architecture.md](architecture.md) §3.2） |
| 评审（re-anchor/resolution）与记忆抽象 | V1 review-engine / memory-service |
| F1–F4 全部 + 命中测试补全场景（§3） | — |

### S12–S13 全项目 Code Review 与整改

工程审查，无外部功能对标；按 CR-01～CR-09 独立产出 finding，Confirmed 项经 S13 三波整改收口。审查结论、整改清单与 S13 拍板已归档：拍板要点见 [architecture.md](architecture.md) §4，过程史见 [history.md](history.md)。

---

## 3. 已确认扩展功能族：多账户额度、切换、子 Agent 路由与输入缓存（G1–G7）

> 2026-08-14 调研并经用户确认（决策原则：**减少实现复杂度、优先缓存命中**；决策记录 D1–D8 见 [references.md](references.md) 附录 C）。调研全文见同文附录 A；分功能方案（F1–F6，**已确认**）见附录 B。
>
> 对照来源（第二批调研）：opencodex、cc-switch、CLIProxyAPI、claude-code-router、LiteLLM、new-api、claude-relay-service 等，以及 OpenCode/Pi 在多账户与缓存维度的补充调研（项目手册见 [references.md](references.md) §3）。2026-08-18 补入 Codex Router：凭证隔离的多客户端本地路由器，不作账户池主参照。

| ID | 功能 | 来源参照 | 说明 | 优先级 | 落点 |
| --- | --- | --- | --- | --- | --- |
| G1 | 同 Provider 多账户池与订阅 plan 凭证 | opencodex 账户池、CLIProxyAPI auth-dir；OpenCode/Pi 多账户缺位（差异化机会） | 激活 V1 provider-control 账户层（ProviderAccount/CredentialLease）+ 新增订阅 plan OAuth 凭证 kind + 扩展 `auth.json` 多账户命名（0600、原子写、损坏 fail-closed）+ `pawork accounts` CLI | P1 | 方案 F1-B |
| G2 | 额度窗口跟踪与预算 gate 增强 | opencodex 5h/周/30d 窗口探测、CLIProxyAPI-Plus 阈值停用、litellm 层级预算 | LocalLedger 派生 + 响应头/错误体被动配额信号捕获归一为 QuotaSnapshot；远端适配器与 WebScrape 保持冻结候审 | P1 | 方案 F2-A+B |
| G3 | 缓存感知的会话-账户亲和路由 | claude-relay-service sticky session、CLIProxyAPI session-affinity、opencodex thread affinity | SessionBinding 亲和默认开 + 新会话再平衡 + 新增「配额余量优先」路由策略 + 分类错误 rebind；请求级轮换不作默认 | P1 | 方案 F3-B |
| G4 | 子 Agent 声明式 provider/model/账户绑定 | opencode agent.model + 权限派生；CCR 子代理标签（反例，不采纳）；opencodex 模型即子代理 | Agent Profile/spawn 参数声明绑定 → RouteContext → provider-control 选账户；默认继承父绑定、显式覆盖；预算经 budget-gate 分配 | P1 | 方案 F4-A+B |
| G5 | canonical 输入缓存策略控制 | Anthropic cache_control、OpenAI prompt_cache_key、pi/opencode/Claude Code 断点收敛实践 | cache 注解（canonical，无厂商字段）+ registry 缓存能力表 + adapter 断点/亲和键映射 + 缓存用量入账与命中率观测 + compaction 联动 | P1 | 方案 F5-B |
| G6 | 账户/端点配置导入 | cc-switch SQLite SSOT、CLIProxyAPI auth-dir、opencodex config、Codex Router 托管 config 块、Claude/Codex 官方布局 | `pawork-workspace::import` 增加账户与端点只读导入源，secret 直接转存 Pawork auth 文件，不落仓库或中间文件 | P2 | 方案 F1 附属 |
| G7 | 对外账户池网关模式 | opencodex / CLIProxyAPI / Codex Router 网关形态 | 近期不内建：以 openai-compatible 上游接外部网关 + 对内账户池；长期按需评估 channels 扩展 feature | P3 | 暂不排期（方案 F6，决策项；登记于 [../ROADMAP.md](../ROADMAP.md) 候选池） |

**状态**：G1–G6 已确认、待立项（登记于 [../ROADMAP.md](../ROADMAP.md)）；G7 维持不做。其中 G5 涉及冻结契约的附加式字段扩展（CanonicalModelRequest/ModelResponseSummary），须遵守 [architecture.md](architecture.md) §3.2 golden 先行原则。配套工作约定（执行期凭证 fail-closed / 少测试无门禁 / 缓存命中 95-97-99 目标）见 [references.md](references.md) 附录 C。

---

## 4. 候选功能对照（未排期；对照 OpenCode / Pi / Codex / DeepSeek Harness）

> 本节先于 2026-08-14 对照 OpenCode / Pi / Codex 的公开功能面，再于 2026-08-17 补入 DeepSeek Harness，与 Pawork 已交付范围对照后识别**尚未规划**的功能缺口。每项标注来源、是否违反架构红线、建议优先级（P0 最高）。已交付或冻结候审的不在此列。四家项目的背景与功能全貌见 [references.md](references.md) §2。候选纳入排期的流程见 [../ROADMAP.md](../ROADMAP.md) 候选池章节。

### 4.1 架构红线排除项（不实现）

以下功能因违反 Pawork 架构红线（[ADR-001](../../Pawork_v1/docs/adr/ADR-001-pure-rust-core.md) 纯 Rust、[ADR-019](../../Pawork_v1/docs/adr/ADR-019-no-tui.md) 无 TUI，均 V1 归档）**不纳入路线图**，仅记录排除理由以备回溯。

| 功能 | 来源 | 排除理由 |
| --- | --- | --- |
| 交互式全屏 TUI（themes/keybinds/sounds/Ctrl+G 编辑器） | OpenCode / Pi | ADR-019 明确不实现 TUI；Pawork 以 CLI 交互模式 + GPUI Desktop 为用户界面 |
| JS/TS 插件运行时（Bun/Node 扩展、hot-reload、JS hooks） | OpenCode / Pi / DeepSeek Harness（Cordis「一切皆插件」） | 纯 Rust 红线（ADR-001）；若未来做代码插件，只评估 WASM + in-process hooks |
| npm 生态传输（npm SDK/插件安装、Bun/Node runtime） | OpenCode / Pi / DeepSeek Harness | 同上；即使未来做插件也不走 npm |

### 4.2 CLI 交互与命令体验

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| A1 | 自定义 slash 命令 / Prompt Templates | OpenCode `/commands`、Pi Prompt Templates | 用户定义 Markdown prompt 片段 + `$ARGUMENTS`/`{{var}}` 变量替换，作为 `/name` 命令调用；可绑定 agent/model/subtask | P1 |
| A2 | `pawork init` AGENTS.md 生成器 | OpenCode `/init` | 扫描仓库结构 → 交互式问答 → 生成/更新 AGENTS.md。当前只 *加载* AGENTS.md，不生成 | P1 |
| A3 | Turn 级 undo/redo | OpenCode `/undo` `/redo` | 回退上一轮用户消息 *及其关联文件改动*（经 Git）。区别于 Run 级 checkpoint/rollback——粒度是「对话轮次」 | P2 |
| A4 | 写后自动格式化（Post-edit formatters） | OpenCode | write/edit/apply_patch 成功后可选自动跑 `cargo fmt` / `prettier` 等；可按语言/工具开关 | P2 |

### 4.3 内置工具与上下文扩展

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| B1 | webfetch + websearch 内置工具 | OpenCode、Codex `--search`、DeepSeek Harness `tool-web` | 网页抓取（给定 URL → markdown）与网络搜索（关键词 → 结果摘要）；现有工具集无网络工具 | P1 |
| B2 | question 工具（模型侧多选问答） | OpenCode、DeepSeek Harness `tool-ask-user` | 模型主动调用结构化多选问题向用户提问，阻塞等待回答；区别于 policy 审批——这是模型侧信息获取 | P2 |
| B3 | todowrite 工具（模型侧任务清单） | OpenCode `todowrite`、DeepSeek Harness `tool-todo` | 模型自管理的轻量 checklist；区别于 plan 域（审阅+审批工作流） | P2 |
| B4 | References（工作区外引用） | OpenCode | 将额外本地目录或克隆的 Git 仓库注册为 `@alias` 上下文源。当前 `@file` 只在工作区 roots 内 | P2 |
| B5 | 图片输入与多模态 | Codex `--image` / `-i` | CLI 接受图片文件路径或 stdin 粘贴，作为 image content part 发送给 Provider。V1 P6-6 已实现；V2 未显式列为用户可见能力 | P1 |
| B6 | 图片生成工具 | Codex `$imagegen` | 让模型在编码循环中生成图片（图标、mockup、diagram）。需接入 image generation Provider | P3 |
| B7 | `pawork mcp-server`（作为 MCP Server） | Codex `codex mcp-server` | Pawork 自身作为 MCP Server 暴露工具，让其他 MCP Client 驱动 Pawork 会话。当前只做 MCP Client | P2 |
| B8 | Code Mode / 单轮组合多步工具 | DeepSeek Harness PTC（Code Mode SDK） | 模型用一段程序在单轮内组合多步工具，减少往返。**不得**引入 JS/TS runtime；若落地需另选 Rust/WASM 或结构化 DSL；会改 loop 形态，落地前需单独设计 | P2 |
| B9 | 会话级 Goals（目标对象） | DeepSeek Harness `ctx.goals` | 同一会话内维护可续跑的目标对象。区别于 B3 checklist 与 plan gate | P2 |

### 4.4 扩展生态

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| C1 | 能力包打包与 git 分发 | Pi Packages | 将 skills + prompt templates + hooks + themes 打包为单一 manifest，支持 git 仓库安装/分享（不含 JS/TS 代码） | P2 |
| C2 | 用户级 memories（`/memories`） | Codex local memories | 跨会话的用户级记忆存储 + 管理命令；用户显式管理的轻量 preferences/facts，不需要 embedding Provider | P2 |
| C3 | 连接器目录（Connector directory） | Codex plugins | 预置 MCP connector 目录 + 一键安装 + OAuth 配置 UX | P2 |
| C4 | LSP 自动安装矩阵 + diagnostics 反馈 | OpenCode lsp | 内置常用语言服务器自动发现与安装，把 diagnostics 反馈给模型 | P3 |

### 4.5 集成与分发

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| D1 | 第一方 IDE 扩展 | OpenCode、Codex | VS Code / Cursor / JetBrains 扩展，经 ACP 或 GUI Connection Protocol 连接 `pawork`（不嵌入 Core）；支持 open-file/selection context、in-place review | P1 |
| D2 | GitHub / GitLab CI bot | OpenCode `/opencode`、Codex `@codex review` | 在 issue/PR 评论中触发 Pawork，自动 triage / implement / open PR | P2 |
| D3 | 会话公开分享 | OpenCode `/share`、Pi session 分享 | 生成可分享的只读会话链接或导出（HTML/JSON/gist） | P2 |
| D4 | Web UI 浏览器客户端 | OpenCode `opencode web`、DeepSeek Harness `dsh web` | 作为 GUI Connection Protocol 的 Web client（本地 web app、LAN bind、basic-auth），与 Desktop client 并列 | P2 |
| D5 | 自更新与多渠道安装器 | OpenCode `opencode upgrade`、Codex installers | `pawork upgrade` + 多渠道安装器（Homebrew / Scoop / Winget / curl / cargo install） | P2 |
| D6 | Cloud 执行环境 | Codex Cloud | 隔离的远程执行环境，支持并行任务、结果本地应用。需 remote transport + 隔离沙箱 + 任务编排 | P3 |
| D7 | Slack / Linear 等 chat 平台集成 | Codex `@Codex` in Slack/Linear | channels 扩展 feature，将 chat 平台消息映射到 Pawork 会话 | P3 |
| D8 | 订阅登录（plan credits 认证） | Pi `/login`、Codex SiwC | ChatGPT 与 xAI 的订阅 OAuth 已交付；Claude Pro/Max、GitHub Copilot 等其它 plan 登录仍为候选。§3 G1（F1-B）继续负责后续多账户/订阅凭证抽象 | P2 |

### 4.6 企业与安全

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| E1 | Enterprise SSO + 组织级集中配置 | OpenCode Enterprise | 企业 SSO + 组织级集中配置下发（model allowlist、workspace roles、internal gateway）。control-plane 是本地 tenant/usage/quota，不含 org SSO | P3 |
| E2 | Bedrock / Vertex 作为显式模型源 | Codex Bedrock、Pi providers、DeepSeek Harness LLM 适配 | AWS Bedrock / GCP Vertex AI 作为模型接入端点；不在六条首发通道内 | P2 |

### 4.7 运维与产品体验

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| F1 | 版本自检 + 遥测 + 离线模式 | Pi telemetry | 启动时检查最新版本（可选匿名遥测 ping，opt-out），`--offline` 禁用所有启动时网络请求 | P3 |

### 4.8 落地建议

上表共 **28 项**候选功能（排除 3 项架构红线排除项），按优先级分布：

- **P1（5 项）**：自定义命令、AGENTS.md 生成器、webfetch/websearch、图片输入、IDE 扩展；下一产品线立项时按一个真实产品目标择取。
- **P2（17 项）**：核心功能补全；按 resources/MCP、clients、workflow、Desktop 等真实消费面分别立项。
- **P3（6 项）**：图片生成、LSP 自动安装、Cloud、Slack/Linear、Enterprise、版本/遥测/离线等重型或长尾功能。

**注意事项**：

1. 工具类缺口（B1–B9）大多可以新增 `AgentTool` 实现的方式低风险追加，不触及契约或架构；B8 除外（改 loop 形态，需单独设计）。
2. CLI 体验类（A1–A4）写入集限定在 `crates/cli`。
3. D1（IDE 扩展）实现路径是独立的 IDE 扩展项目经 ACP 连接，不影响 Core 包。
4. 部分功能（D5 安装器、F1 遥测）是纯运维/产品层，无架构依赖，可随时插入。
5. 每项落地时须遵守 [architecture.md](architecture.md) §3.2 冻结契约先行原则——新工具的 `ToolDescriptor` 审批/只读语义在加入时就定义清楚。

---

## 5. 发布策略

W1–W4 波次与包清单为 V1 时期的历史候选策略（原文随 V1 迁移参考归档，见 [history.md](history.md)），不属于当前执行范围。各包保持发布卫生（元数据、无类型泄漏、`publish = false` 默认）。只有在用户明确决定发布后，才另立发布任务并重新核对波次、License、全量门禁与三平台证据。
