# Pawork V2 总结(S0–S13,2026-08-14 ~ 2026-08-18)

> 本文是 V2 开发周期的**归档总结**。V2 的任务编排文档(`v2_plan.md`)、V2 版 `ROADMAP.md` 与阶段任务书(`plan/S0–S13`)已随 V3 规划删除,需要保留的事实与结论压缩于本文;后续开发以仓库根 [v3_plan.md](../v3_plan.md) 与 [ROADMAP.md](../ROADMAP.md) 为准。
>
> 仍然有效的常设文档:[design.md](design.md)(设计与冻结契约)· [gui-design.md](gui-design.md)(Desktop GUI 设计)· [references.md](references.md)(参照项目手册)· [task-guide.md](task-guide.md)(任务实现规范)· [v1-migration-reference.md](v1-migration-reference.md)(V1 迁移词典,冻结)· [reviews/s12/](reviews/s12/)(S12 审查九报告 + 五裁定)· [adr/](adr/)(本仓库现有 ADR-037;ADR-001~036 随 V1 归档)。V1 资产归档于仓库外 [../../Pawork_v1/](../../Pawork_v1/)。

---

## 1. V2 方法论与总体结论

- **背景**:V1(约 23.6 万行)呈「组件齐全、主干未通电」病灶(详见 [v1-migration-reference.md](v1-migration-reference.md) §1)。V2 改为增量式重建:S0 起 `pawork` 二进制始终可编译、可运行、可被真实使用,每阶段以「新增用户可见能力」定义。
- **三道保险**:终局包布局先行、冻结契约先行(golden 先于消费实现)、迁移词典 +「无消费者不合入」。V1 资产按「复制 + 合并 + 改名」按需搬运。
- **结果**:S0–S13 全部收口(S6 余一项 OAuth 临期 refresh 人工验收挂账);38 crate ≈ 19 万行;CLI 完整闭环(对话/工具/审批/沙箱/用量/多通道)+ GPUI Desktop v3 三栏工作台 + 服务化(多客户端/SDK/ACP/PTY)+ 工作流与控制面。
- **验证方式**:真实冒烟(低消耗模型矩阵,见 §3)+ 定向自动化(契约 golden、安全红线回归、解析器种子);开发期无 clippy/fmt/Workspace Full Gate;未发布。
- **旧计划教训**:最初「按域整体迁移」计划(M0–M8)第 5 个里程碑才出现可运行物,重演 V1 病灶,被增量式取代;M0–M8 正文从未落仓([../plan/archive/README.md](../plan/archive/README.md) 仅存索引),迁移细则唯一事实源是 [v1-migration-reference.md](v1-migration-reference.md) §4.1。

## 2. 阶段交付总览

| 阶段 | 主题 | 关键交付(全部已真实冒烟验收) |
| --- | --- | --- |
| S0 | 最小可对话 CLI | `pawork chat` 流式多轮、Ctrl-C 取消、`pawork models`、TOML 配置 + env key;401/429/超时可读呈现 |
| S1 | 会话持久化 | SQLite 落盘、`sessions list/show`、`--resume` 续聊、`--json` 事件流;envelope golden 与 append-only 契约生效;`kill -9` 后可恢复 |
| S2 | Agent Loop 与只读工具 | read/list/search/find 四工具自主循环;OpenAI/Anthropic 双协议 tool-calling;MockProvider 测试基座 |
| S3 | 写入工具与审批 | write/edit/apply_patch + `--approval-mode` 终端审批;路径越界/symlink 拒绝;deny 后会话可续 |
| S4 | 命令执行与沙箱 | run_command(进程树清理 + Seatbelt/Landlock 沙箱 + 输出截断);「读-改-跑」编码闭环;fail-closed(ADR-031 可观测回退) |
| S5 | 上下文预算与用量 | 软限压缩/硬限截断、token 与费用统计、`/compact`、`models` 目录(window/定价);token 计量与厂商侧对账 1:1 |
| S6 | 首发 Provider 与认证 | 六通道适配(DeepSeek/GLM/OpenCode Go/ChatGPT/xAI/Qwen)、`models` 聚合、auth 文件后端为主 + env 降级 fallback、ChatGPT/xAI OAuth(singleflight + 跨进程 write/refresh 锁)、全局脱敏 layer,trace 0 泄漏 |
| S7 | 最小 Agent GUI | v3 三栏工作台(TaskRail 双分组/定向新建、流式对话、内嵌审批、取消、ContextMeter、RunStatusBar);`gui serve`(UDS 单实例);GPUI 锁定 `=0.2.2`(ADR-035);关窗不杀 Run;跨通道切换 |
| S8 | Git、Diff 与 Checkpoint | 会话 diff、编辑前快照、`pawork rollback`、审批 hunk 预览;blob store(`PWB1` + protected AEAD,ADR-032);git 注入防护(`--force`/`-o` 拒绝) |
| S9 | MCP、资源与兼容导入 | rmcp `=2.2.0` 锁入内部 codec、MCP 工具与内置共用 ToolRegistry;AGENTS.md/Skills 注入;`@file`;config 六层 + Profile;五来源只读导入(Claude/Codex 等)+ `sessions import/export`;workspace file-index |
| S10 | 服务化与客户端 | protocol 收口(typegen 检入 `schemas/`)、EventHub(ring/replay/Lagged)、多客户端 gui-server、`pawork-sdk`、ACP(Zed 1.15 实测)、PTY、`service install`(dry-run 默认)、headless `--json-stdio`、`sessions fork`、protocol-probe 9 场景 |
| S11 | 工作流、多 Agent 与控制面 | Plan 整版审批 gate、`pawork tasks/usage/agents demo`;control-plane(UsageLedger/audit JSONL)、quota(LocalLedger)、provider-control(lease/binding/pool)、orchestration(Supervisor spawn/cancel-tree/budget-gate) |
| S12 | 全项目 Code Review | 只读审查 CR-01~CR-09,60 finding(全 Confirmed:H15/M27/L18)→ 57 项任务;报告与裁定见 [reviews/s12/](reviews/s12/);不改代码 |
| S13 | S12 finding 整改 | 57 项三波收口(波 A 安全 F01–F14 → 波 B Bug F15–F40 → 波 C 文档);契约变更 ADR-037;安全红线回归全绿 |

## 3. 真实测试模型矩阵(V2 约定,V3 沿用输入)

| 通道(provider_id) | 默认测试模型 | 凭证形态 |
| --- | --- | --- |
| DeepSeek(`deepseek`) | `deepseek-v4-flash` | API key |
| GLM Coding Plan(`glm-coding`) | `glm-4.7` | API key |
| OpenCode Go(`opencode-go`) | `deepseek-v4-flash` | API key |
| xAI Grok 订阅(`xai`) | `grok-4.3` | OAuth bearer |

规则:常规冒烟/回归只用矩阵内组合;高级模型(`deepseek-v4-pro`、`glm-5.x`、`grok-4.6`、ChatGPT/Codex 系列等)仅限一次性接通验证或用户明确指定的专项评估。凭证在 `~/.pawork/auth.json`(env 降级 fallback);缺失即 fail-closed,不静默 mock。Secret 红线:key/token 不入日志、事件、配置样例与可提交文件。

## 4. 冻结契约与 ADR(仍然生效,V3 不得破坏)

| 契约 | 形状与位置 |
| --- | --- |
| 事件信封 | envelope v1,append-only;sessions schema v10(S13-F09 增 `ancestor_lineage`);信封字节 golden 在 `pawork-domain`(R1 起,`crates/domain/tests/events_golden.rs`),DDL/迁移锚在 `pawork-storage::session` |
| 会话存储 | SQLite DDL(sessions/events/usage);import/export v3 格式;fork 分支(`fork_from_event`,active branch 写入) |
| blob 格式 | `PWB1` + protected AEAD(ADR-032);artifact/protected/checkpoint 三区 |
| GUI 协议 | 帧格式 ADR-036;`SUPPORTED_API_VERSIONS` 1.0/1.1/1.2(S13-F09 `RunStart.provider` 升 1.2);typegen 检入 [../schemas/](../schemas/)(core-api/gui-protocol/headless-json) |
| headless JSON | `HeadlessResponse`(`type=event|response`);`run`/`chat --prompt --json` 已对齐 |
| config | 六层合并矩阵 + Profile 文件切换;凭证解析链 auth 文件→env→无(fail-closed) |
| 控制面 | usage `dedup_key`;audit JSONL(`fixtures/audit/event-v1.jsonl`) |
| 协议兼容表 | `PROTOCOL_CRATE_COMPATIBILITY`;协议帧 golden(9+17 条) |

ADR 索引(ADR-001~036 归档于 `../../Pawork_v1/docs/adr/`,原则继续有效;本仓库编号续接):ADR-031 沙箱不可用时可观测回退(S4/S13-F19 沿用)· ADR-032 blob 格式(S8 采用)· ADR-035 gpui 版本锁定 `=0.2.2`(S7 采用)· ADR-036 GUI 协议版本协商(S7/S10 沿用)· [ADR-037](adr/ADR-037-s13-wave-b-contracts.md) S13 波 B 五项契约(trait 归 domain / 维持 ADR-031 / `ToolResultContent.artifacts` / `Revised` title+steps / `ResultArchived.task_id`)。

## 5. S13 关键拍板(安全语义,V3 重构须保持)

- **F01**:读写工具均拒 `.git`(无审计开关)。
- **F02**:macOS Seatbelt 写+网模式诚实标签 `HardWritesAndNetwork`;`default_secret_paths` 扩充。
- **F05**:MCP 凭证走 SecretRef(仅 `pawork.mcp.*` 命名空间)+ 独立 `mcp-auth.json`;stdio 子进程 `env_clear` 且拒绝透传 `PAWORK_API_KEY_*`。
- **F06/F07**:workspace 级配置剥离 `proxy_url`/非回环 `base_url`;HTTP 错误只留 `HTTP {status}`;`redirect(Policy::none())`。
- **F08**:workspace 级配置剥离 MCP `trusted`/`auto_start`。
- **F11/F32**:EventHub Lagged → `ReplayUnavailable`;客户端收齐附带 Snapshot。
- **F33**:未映射 headless 命令 fail-closed。
- 路径检查统一 `policy::path` 内核(读路径 symlink 同内核);生产 `gui serve` 强制 token(UDS 0600);Timeline 锚点用 `event_id`/`sequence`。

## 6. 遗留债务(V2 收口时未完成,已转入 V3 规划输入)

### 6.1 待执行任务(原 K-01~K-10)

| 编号 | 内容 |
| --- | --- |
| K-01 | 仓库根迁移后 `foundation/config` 路径闭环核对 |
| K-02 | `ToolApprovalRequested` 等待前持久化(崩溃后 seal/resume/不重复执行语义) |
| K-03 | S7 Desktop 人工验收:中文 IME、多行粘贴、1440×1024 对照定稿图、键盘走查(F14/F34/F35/F36/F53–F56 证据) |
| K-04 | S8 Desktop Changes 面(Inspector Files/Summary + ActivityPopover 摘要;并入 `HunkStageService` 消费,S12-F57) |
| K-05 | S9 本机会话格式导入(`~/.claude/projects/**/*.jsonl` 与 Codex rollout,待脱敏样本) |
| K-06 | S9 Desktop `@`/Resources 面 |
| K-07 | `host/app/src/rate_limit.rs` 有实现无生产调用:接入或删除 |
| K-08 | `ArtifactStreaming` 能力宣告与实际 unsupported 不一致:接线或停止宣告 |
| K-09 | macOS sandbox `network_allow_hosts` 全拒未实现:egress broker 或收窄配置 |
| K-10 | Anthropic Messages 能力收口(prompt cache/thinking/hosted tools 等 adapter TODO 逐项定夺) |

另:S6 ChatGPT/xAI OAuth **自然临期真实 refresh** 人工验收挂账(唯一非 🟢 主干项);F03 Windows Service 本机无法验收(降级登记);F10 两 GUI 冒烟未复跑;「多账户功能族并入 plan」(F1–F5+G6,决策 D1–D8 已确认)未执行。

### 6.2 休眠能力与激活条件(已迁入但无生产消费者)

| 能力 | 状态 | 激活条件 |
| --- | --- | --- |
| `pawork-diagnostics` metrics/bundle | `experimental` feature 门控 | 出现真实诊断导出/指标消费方 |
| control-plane OTel audit exporter | 类型已迁,无 collector | 真实审计导出消费者 |
| provider-control account/routing/health/factory | feature `account-control-v1`(默认开);demo 只走 lease 路径 | 真实多账户 factory 装配 |
| `pawork-workflow` `process-exec` | feature 默认关 | 后台任务需要真实进程 |
| `pawork-workflow` goal/automation/monitor 域 | 状态机+测试已迁,无宿主消费面(S12-F40) | 对应产品面立项 |
| `pawork-memory` | Mock 召回,无真实 EmbeddingProvider | 真实 embedder + 宿主 `memory_available` |
| `pawork-review` Forge | Generic 占位,无 GitHub/GitLab 实现 | 会话内评审接线 |
| `pawork-orchestration` teams / 真实双子 run_session | demo 级 | teams 面或真实并行子 Agent |
| tool result 分级裁剪 | `ToolResultContent.artifacts` 已扩,engine 未接线(S12-F49) | 超大 tool output 需分级 |
| S10 本机 GPUI 多窗口 | 单 `open_window`(S12-F37) | 产品定义每窗策略 |
| 对外账户池网关(F6-B) | 不内建(F6-A 已确认) | `pawork-channels` 扩展 feature 长期评估 |

### 6.3 未决决策

- License 与 crates.io 占名(任何发布前硬前置)。
- 冻结候审资产砍留:quota 远端六厂商 WebScrape/refresh、browser-computer、tool_search(清单见 [v1-migration-reference.md](v1-migration-reference.md) §4.4)。
- 扩展生态整族(WASM 插件/市场/用户 Hooks/LSP):移出排期待决策;预留已保留(`PluginId`、`ToolCapability::ExternalPlugin`、`pawork-api` `plugin` feature 等);实现资产见 [../plan/archive/S10-extensions-deferred.md](../plan/archive/S10-extensions-deferred.md)。
- 全量门禁、三平台验证与发布:须用户明确决定后另立任务(历史线索:[v1-migration-reference.md](v1-migration-reference.md) §6.3)。

## 7. 历史里程碑(压缩)

| 日期 | 事件 |
| --- | --- |
| 2026-08-14 | V1 全量 Review 定稿(原 ROADMAP_V2.md → v1-migration-reference.md);按域计划 M0–M8 登记后即被增量式 S0–S12 取代;多账户调研 D1–D8 确认;五文档体系成型 |
| 2026-08-14~15 | S0–S5 收口(对话→持久化→工具→审批→沙箱→上下文) |
| 2026-08-15~16 | S6 六通道与认证(OAuth 收口本地实现) |
| 2026-08-16~17 | GUI v3 视觉基准定稿([../design/README.md](../design/README.md));S7 四波收口 |
| 2026-08-17 | V1 归档至 `../Pawork_v1` + V2 摊平为仓库根(1280 D/267 R);AGENTS/README 重建,84 断链修复;S8/S9/S10 收口(Zed ACP 实测);S11 波 A–C |
| 2026-08-18 | S11 收口;S12 九包审查(60 finding);S13 三波整改收口(57 项);参照项目补 Codex Router |

> 各阶段逐波实现记录(写入集、测试数、冒烟口令等)原载 `v2_plan.md` §3 指针表,历史价值有限未随迁;如需考古以 git 历史中的 `v2_plan.md` 与 `plan/S*.md` 为准。
