# Pawork V3 开发路线图(重构式 · R0–R9)

> 本文档是 Pawork V3 的**任务总索引**:登记全部任务(未开始 / 进行中 / 已完成)的状态与介绍,并链接到 [plan/](plan/) 内的详细任务书。V3 不是新功能扩张,而是在 V2 增量交付完成(S0–S13,总结见 [docs/v2-summary.md](docs/v2-summary.md))后的一次**全仓结构重构**:目标是把 V2「先通电、后收敛」策略遗留的结构债一次清偿。允许破坏式重设计,但磁盘/线上冻结契约的演进必须经 ADR 版本化,不允许静默破坏。
>
> **文档体系**(常设文档不变,V2 编排文档已由 V3 版取代):
>
> | 文档 | 职责 |
> | --- | --- |
> | 本文 `ROADMAP.md` | 任务总索引:阶段状态、阶段外任务、未决事项、风险 |
> | [v3_plan.md](v3_plan.md) | 任务开启编排:当前指针、选波规则、子代理派发 |
> | [plan/R0–R9](plan/) | 每阶段任务书:目标、证据、决策点、波次拆分、退出标准 |
> | [docs/design.md](docs/design.md) | 设计文档:包布局(R1 收口后重写 §2)、冻结契约、功能设计 |
> | [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计(附件 [design/README.md](design/README.md):GUI v3 视觉基准) |
> | [docs/references.md](docs/references.md) | 参照项目手册(§7 为 R0–R9 阶段参照指引) |
> | [docs/task-guide.md](docs/task-guide.md) | 任务实现规范(开启 / 进行 / 收尾公共约定) |
> | [docs/v2-summary.md](docs/v2-summary.md) | V2 归档总结(S0–S13 交付、冻结契约、遗留债务) |
> | [docs/v1-migration-reference.md](docs/v1-migration-reference.md) | V1 迁移词典(冻结参考) |
> | [docs/code-map/README.md](docs/code-map/README.md) | 按需导览，非布局/契约事实源（按写入集读各包 `MODULE.md`；冲突以源码为准） |
>
> 工作约定见仓库根 [AGENTS.md](AGENTS.md)。V2 版 `v2_plan.md`、V2 ROADMAP 与 `plan/S0–S13` 已删除,保留内容压缩于 [docs/v2-summary.md](docs/v2-summary.md);考古以 git 历史为准。

---

## 1. V3 目标与计划原则

V3 由四条目标定义(依据:2026-08-18 五路只读分析——两路包合并、依赖用面审计、GUI 组件分析、补丁式实现全仓扫描;证据落在各 plan/R\*.md):

1. **结构收敛**:workspace 39 成员 → **21 成员**(19 库 + 2 应用);休眠库存(约 3.3–3.8 万行零消费者代码,近 20% src)裁决归档;确立「消费面先行」硬门——每个模块要么在产品面有真实装配点,要么移出主干。
2. **依赖治理**:本地化 3 项(rand→getrandom、parking_lot→std::sync、base64→本地 base64url)、死声明清理 2 处、版本升级 8+ 项(消除 Cargo.lock 多版本共存)、rmcp 3.x 专项评估。
3. **补丁根因重构**:12 个聚类主题(T1–T12,对照表见 §2.1)——协议三通道同源化、宿主单体拆解、幂等持久化、降级可观测、Provider 中立层渗漏封堵、凭证词汇净化、会话分支原生化、沙箱真隔离。
4. **GUI 工程化**:`apps/desktop` 建立 `ui/theme.rs` tokens 与 `ui/components/` 组件库(97 处硬编码色 → ~20 token;15 处手写按钮、4 组复制菜单 → 11 个组件),菜单改 gpui `anchored()/deferred()`,并收口 V2 遗留的 Changes / `@` / Resources 面与人工验收。

计划原则:

1. **每阶段保持可用**:`pawork` 二进制在任意波次收口时可编译、可运行、既有冒烟行为不回退;重构不断主干。
2. **冻结契约不静默破坏**:事件信封、session DDL、blob `PWB1`、GUI 帧、headless JSON、config 层级、usage `dedup_key`、audit JSONL(清单见 [docs/v2-summary.md](docs/v2-summary.md) §4)只能经 ADR 版本化演进(R6 分支模型、R7 沙箱为已知的两处);golden 先于实现改动。
3. **删除优先于门控,门控优先于库存**:零消费者代码默认归档(git tag `v2-final` 兜底可找回),不再以「experimental feature + 登记」方式库存。
4. **决策先行**:R0 一次性拍板产品形态与库存去留(ADR-038),避免后续阶段反复翻案;R1 包布局(ADR-039)、R6 分支模型(ADR-040)、R7 沙箱(ADR-041)各配独立 ADR。

### 1.1 真实测试模型约定(低消耗默认,沿用 V2)

| 通道(provider_id) | 默认测试模型 | 凭证形态 |
| --- | --- | --- |
| DeepSeek(`deepseek`) | `deepseek-v4-flash` | API key |
| GLM Coding Plan(`glm-coding`) | `glm-4.7` | API key |
| OpenCode Go(`opencode-go`) | `deepseek-v4-flash` | API key |
| xAI Grok 订阅(`xai`) | `grok-4.3` | OAuth bearer |

规则同 V2:常规冒烟、定向回归与行为对比默认只用矩阵内组合;高级模型仅限一次性接通验证或用户明确指定的专项评估;模型名以 `pawork models` 实际返回为准;凭证缺失即 fail-closed。Secret 红线不变:key/token 不入日志、事件、配置样例与任何可提交文件。

---

## 2. 阶段总览(R0–R9)

状态符号:⚪未开始 · 🔵进行中 · 🟢已完成 · ⚠️阻塞。每阶段详细任务、证据、决策点与并行拆分见 `plan/R*.md`。

| 阶段 | 主题 | 关键动作 | 触及范围 | 硬前置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| [R0](plan/R0-inventory-decisions.md) | 决策收口与休眠库存裁决 | ADR-038(单机 vs 多租户、remote/teams/三域/account-control 去留);归档约 3.3–3.8 万行零消费者代码;K-07 删除、K-08 停止虚假宣告;死 feature/死声明清理 | 全仓休眠面(workflow/orchestration/control-plane/transport/diagnostics/net/session/engine/host) | 无 | 🟢 |
| [R1](plan/R1-package-consolidation.md) | 包合并 37→21 | ADR-039(目标布局 + 目录扁平化);api→domain、sqlite+session+blob→storage、net+core+adapters→providers、core+resources+config+compat→workspace、mcp→tools、quota+provider-control→control-plane、gui-server→app、channels→cli、sdk→client、diagnostics 解散、probe→client 测试;golden 随迁 | 全部 crate 的 Cargo.toml/目录/use 路径;design.md §2 重写 | R0 | 🟢(波 A–E ✅ 2026-08-19,members 21,扁平 `crates/` 迁移定稿) |
| [R2](plan/R2-dependency-governance.md) | 依赖治理 | rand/parking_lot/base64 本地化;notify 8、windows 0.61、portable-pty 0.9、ts-rs 12、reqwest 0.13、toml 1.1、rusqlite 0.40、sha2 0.11 升级;lock 多版本去重断言;rmcp 3.x 专项 | 各 crate Cargo.toml + 少量调用点 | R1 | 🟢(波 A–D ✅ 2026-08-19~20:三项本地化、九项升级、rmcp `=3.1.3`/MSRV 1.88、lock 836→826;整阶段复核已修 notify Rescan 漏扫、directories 路径测试环境短路、rmcp InputRequiredResult 回归缺口与 PTY/Windows 路径注释;当前 tools 130/130,默认目标 tree 归档可复现 notify/reqwest 单版本及 sha2/toml/thiserror 例外,windows 0.57 明确为可选 screen-capture 的 lock 残留;历史 xAI OAuth/MCP 冒烟与编译数字原始输出未归档,不作为仓内可复现门禁) |
| [R3](plan/R3-protocol-unification.md) | 协议与投影同源化(T3+T5) | 单一 command/capability registry,GUI 帧/headless/ACP 三通道 mapping 同源派生(宣告=授权=实现);Timeline 投影 reducer 下沉 protocol 共享模块,host/desktop 同源 + 投影 golden;OnFailure 档位裁决 | protocol、app、cli(headless/acp)、client、desktop projection | R1(R2 可并行) | 🟢(波 A–D ✅ 2026-08-20:registry 单源三通道派生 + 未登记 fail-closed;投影 reducer 下沉 protocol::projection + 三种子 golden 与两端对拍,CR08-08 根治;OnFailure 删除 + serde alias 只进不出 + compat 导入映射 NeverAsk 记 issue,S13-F16 三处注释清除;26 帧 golden/events_golden/schemas 零 diff;probe-smoke、headless --json-stdio、ACP 三通道真实冒烟通过;整阶段审计 2026-08-20~21 已修 registry/生产 host 可用面失真、GUI 帧能力泄漏、订阅拒绝污染后续收帧、TerminalSessions snapshot 泄漏、Timeline 空展示页提前完成、assistant committed 失序/跨轮/live-history 覆盖、并发工具输出串线与重复 live output,并补定向回归;headless/ACP 与 OnFailure 实现复核无缺陷) |
| [R4](plan/R4-host-decomposition.md) | 宿主拆解与可靠性内核(T2+T8+T9) | app 单体按领域服务拆分(巨 match → registry 分发);幂等 CommandLedger 持久化 + K-02 审批等待前落盘;ACP host actor 化;降级事件化契约(消灭静默 `let _`/回退) | app、cli、storage(幂等表)、protocol(降级事件) | R3 | 🟢(波 A/B/C/D 全部收口 2026-08-21:波 A 七服务拆分完成,gui_host 目录化 + registry 分发表(pin 测试锁双射),lib.rs 1413 / gui_host mod.rs 679 行数达标;波 B CommandLedger 入 SQLite v11(列式作用域 (tenant,client_scope,command_id),record 不吞错,重启可重放)+ K-02 等待前落盘(engine emit 时序前移),GUI resume 呈现待审批、决策落盘不重跑,CLI 维持 seal Denied;审查 P0/P1 修复后复核 pass;波 C ACP actor 化(Mutex/expect 清零,prompt 串行语义不变)+ DegradeEvent 契约(domain degrade.rs,serde 零 diff)接五接点,审查双轮 pass;波 D host 域 `let _` 58 处清零(非测试归零,分级 tracing 化)+ HOME 回退结构化告警(load_with 消费 DataDirOutcome)+ usage 哨兵 D1 doc/pin(值零变化)+ hub 简化 + acp map.rs 死码删除,app/cli/client 定向与 probe 冒烟全绿,golden 零 diff,审查 pass;整阶段审计 2026-08-22 修复 InFlight 同键不同 command_id 占位挂死与丢唤醒、record 失败 inflight 不释放、tasks_start_agent 吞错、lib.rs compact_session 残留内联、flush_outbox 缺 warn、wait_std 无界 recv、open_read_only 缺回归共 7 项,驳回虚构路径等误报,独立门禁全绿、冻结契约零触碰,详见任务书 §2.6) |
| [R5](plan/R5-provider-neutrality.md) | Provider 中立化与凭证收口(T6+T11) | provider_hints 命名空间契约(删存储层 provider 键名清单);通道 preset 数据化(新增通道单点登记);credential locator 合一 + keychain 词汇迁移;K-10 Anthropic 能力收口 + CapabilityNegotiator 接线;ReasoningProtector 持久化(PWB1 首个生产消费者) | providers、auth、storage(event_store)、workspace(config)、engine 守护测试 | R1(建议 R4 后) | 🟢(波 A/B/C 全部收口 2026-08-22:波 A provider_hints 命名空间契约 + 通道登记单点化;波 B credential locator 合一 + keychain 词汇迁移 + mcp-auth 域隔离收编;波 C K-10 Anthropic 能力收口(prompt cache/thinking/hosted tools/signature/server_tool/citations 写 wire 或 HTTP 前拒绝)+ CapabilityNegotiator 接线 + ReasoningProtector 持久化(PWB1 首个生产消费者,app protected feature 进闭包,instance-level BlobScope `instance-reasoning` 已接受偏差);整阶段审计修复 provider_hints 深层 Secret/旧键导出、损坏 auth fail-closed 与 MCP Secret 域、Anthropic cache/thinking/signature/redacted/cross-model continuation、negotiator 分区、master.key 并发创建/权限/链接等缺口;Grok 4.6 终审 pass,定向门禁与 `cargo check -p pawork` 全绿;真实 Anthropic 冒烟 fail-closed 入 §4;详见任务书 §6 与 v3_plan §3) |
| [R6](plan/R6-session-branching.md) | 会话分支模型原生化(T4,ADR-040) | 事件/投影原生 branch lineage(替换后补 `branch_id` 列 + 反查回填);schema v12 迁移(v11 已由 R4 波 B command_ledger 占用)+ 旧库升级 golden;压缩按分支水位;K-05 本机会话导入 | storage、engine(compact)、app(resume/fork)、desktop projection | R4(投影同源已就位) | 🟢(波 0 ✅ 2026-08-23:[ADR-040](docs/adr/ADR-040-session-branch-lineage.md) Accepted——原生化 + append-only 单表全局 sequence + lineage 单点收编 + v12 回填即校验 + 压缩按分支水位;波 A ✅ 2026-08-23:CURRENT_SCHEMA_VERSION 12 + messages 重建去 DEFAULT + 4 升级 golden + 删 `ancestor_lineage` 外挂;波 B ✅ 2026-08-23:lineage compact + event-ledger snapshot 水位、闭合 turn fork 白名单/幂等、Pi 单分支折叠、非 wire ForkBoundary + Desktop 同 session reset;波 C ✅ 2026-08-23:K-05 双形态 compat 解析(Claude Code 本地 JSONL + Codex rollout 信封,真实样本结构采样 + 合成脱敏 fixture)、workspace `session_scan` 只读发现(排除 Claude `agent-*.jsonl` sidecar)+ CLI `sessions import --from` 批量导入(app facade 接线)、fork 生产路径 export→import 往返回归;隔离数据目录真实样本导入 + export 还原 + `--from` 幂等通过。全量 home `--from claude` 须排除 subagent sidecar,否则同 sessionId 多文件会 CompatImportConflict;真实 Provider fork/compact 冒烟留 §4 人工验收;整阶段审计(2026-08-23,grok ×4 分域 + 终审)无 P0–P2,P3×2 已修(compat claude 本地噪声 skipped_* 计数补齐、protocol 锚点回退注释定性),门禁全绿) |
| [R7](plan/R7-sandbox-isolation.md) | 执行面真隔离(T7,ADR-041) | macOS Seatbelt 写白名单正式化(读整盘 allow+挖洞);PTY 入 policy 闸;shell 风险分类结构化解析;K-09 删除 network_allow_hosts 字段;三平台沙箱回归 | exec、policy、tools(run_command)、app(PTY 装配) | R1(可与 R3–R6 并行) | 🟢(波 0 ✅ ADR-041 Accepted;波 A ✅ 2026-08-23:macOS profile 写白名单正式化 + 读洞/标签如实化 + default_secret_paths 扩六项 + golden 先行六面落钉,Linux/Windows 复核零行为变更;波 B ✅ 2026-08-23:PTY 创建入 policy 闸(D2;NeverAsk/ReadOnly 直拒,AskUser fail-closed 落 Deny——用户拍板选项 A,命令级交互审批入 §4 候选)+ Terminal 四帧与响应六 golden 先行落钉 ∥ shell 手写 tokenizer(D4;引号/管道/变量绕过种子收紧,-lc 组合簇闭环,地板集合不变);policy 73 / protocol+app 306 / exec 64 / client probe 全绿,cargo check -p pawork 绿,冻结面零 diff;审查 pass;波 C ✅ 2026-08-23:K-09 按 D3 落地——SandboxPolicy.network_allow_hosts 字段与 macOS 死分支删除,Enforce 全拒不变、profile 输出零 diff,exec 64 绿含 Seatbelt 真机逃逸种子无 SKIPPED,tools+app 定向与 cargo check -p pawork、msvc 交叉 check 全绿,审查 pass;Desktop PTY 面板冒烟入 §4 人工验收) |
| [R8](plan/R8-gui-components.md) | GUI 组件化与 Desktop 收口(T12) | theme.rs tokens + ui/components/ 11 组件;菜单 anchored/deferred;hover/active 补齐(先更 GUI 基准);Timeline 虚拟化;K-04 Changes 面、K-06 `@`/Resources 面;K-03 人工验收 | apps/desktop、docs/gui-design.md、design/ | R3(投影 reducer);R2(gpui 树) | ⚪ |
| [R9](plan/R9-consistency-closeout.md) | 一致性收口 | K-01 config 路径核对;S6 OAuth 自然临期 refresh 人工验收(V2 唯一未收口项);安全红线/golden/协议定向回归全量复跑;文档三处一致;遗留与候选登记 | 全仓只读核对 + 文档 | R0–R8 | ⚪ |

**依赖关系**:R0→R1→R2 串行主干(裁决 → 合并 → 治理)。R3–R7 在 R1 后开启:推荐顺序 R3→R4→R5→R6(四者都触 host,串行避免写入集冲突;R2 与 R3 写入集不相交可并行);R7(exec/policy 域)可与 R3–R6 并行。R8 依赖 R3(共享投影 reducer)与 R2(gpui 传递树升级)。R9 收口全部阶段。跨阶段并行须满足写入集不相交,由 [v3_plan.md](v3_plan.md) §4 按波裁定。

### 2.1 补丁主题 → 阶段映射(T1–T12)

来自补丁式实现全仓扫描(2026-08-18,60 处原始补丁聚类;逐项证据在各任务书):

| 主题 | 内容 | 归属 |
| --- | --- | --- |
| T1 休眠库存大清仓 | 约 3.3–3.8 万行零消费者代码裁决 | R0 |
| T2 host/app 单体拆解 | `lib.rs` 4,057 行 + `gui_host.rs` 2,594 行巨 match → 领域服务 | R4 |
| T3 三通道协议面归一 | GUI/headless/ACP 三套 mapping/授权 → 单一 registry | R3 |
| T4 会话分支模型原生化 | 后补 `branch_id` 列 + 反查回填 → 原生 lineage | R6 |
| T5 Timeline 投影单一事实源 | host/desktop/client 三处手搓投影 → 共享 reducer | R3 |
| T6 Provider 扩展元数据契约化 | 存储层 provider 键名清单、通道三处硬编码 → 命名空间契约 + 注册表 | R5 |
| T7 沙箱与执行面真隔离 | 诚实标签 → 真隔离(profile 重设计、PTY 入闸) | R7 |
| T8 降级与吞错可观测契约 | 323 处 `let _`、HOME→temp 静默回退 → 降级事件化 | R4 |
| T9 幂等与占用原语统一 | 内存 CAS、9 张 Mutex map、序列补洞 → 持久化 ledger + actor | R4 |
| T10 控制面多租户对齐单机现实 | `local/default` 哨兵宇宙裁决 | R0(拍板)+ R1(收编) |
| T11 凭证/配置解析去重与词汇净化 | env 双实现、keychain 兼容名、mcp-auth 前缀白名单 → 单一 locator | R5 |
| T12 Desktop UI 工程化 | 单文件 UI、零组件、97 硬编码色 → theme + components | R8 |

---

## 3. 阶段外任务登记

### 3.1 已完成

| 任务 | 完成日期 | 产出 |
| --- | --- | --- |
| V3 立项分析:包合并 ×2、依赖用面审计、GUI 组件分析、补丁式实现全仓扫描(五路只读) | 2026-08-18 | 结论沉淀于 [plan/R0–R9](plan/) 各任务书与本文 §1/§2 |
| V2 文档归档:v2_plan/V2 ROADMAP/plan S0–S13 压缩为总结 | 2026-08-18 | [docs/v2-summary.md](docs/v2-summary.md);原文档删除,git 历史可溯 |
| 参照项目全面复核与 V3 参照指引:GitHub API 全量复核 + 功能重叠二次清理(移除 5 项) + 新增 ACP/gpui-component/Zed ui/srt 四项 + R0–R9 阶段参照调研(三路子代理) | 2026-08-18 | [docs/references.md](docs/references.md) §7(阶段参照指引);移除记录见 [docs/research/multi-account-quota-reference.md](docs/research/multi-account-quota-reference.md) §8 |
| 参照项目补官方仓 openai/codex:手册 §1 主链接从产品文档站改为 GitHub 仓;§2.3/§6.2 与 research §1/§8、design/gui-design 引用同步 | 2026-08-21 | [docs/references.md](docs/references.md) §1/§2.3; [docs/research/multi-account-quota-reference.md](docs/research/multi-account-quota-reference.md) §1/§8; [docs/design.md](docs/design.md) §4; [docs/gui-design.md](docs/gui-design.md) §2 |
| 三层代码地图 | 2026-08-22 | 任务书 [plan/out-of-band/code-map.md](plan/out-of-band/code-map.md)；总索引 [docs/code-map/README.md](docs/code-map/README.md)；21 份 crate/app `MODULE.md`；热点 [docs/code-map/hotspots/](docs/code-map/hotspots/) |

### 3.1b 进行中

当前无进行中的阶段外任务。

### 3.2 V2 遗留债务 → V3 阶段映射

V2 收口时的 K-01~K-10 与其他挂账项(原委见 [docs/v2-summary.md](docs/v2-summary.md) §6)全部并入 V3 阶段,不再单列执行:

| 遗留项 | 内容 | V3 归属 |
| --- | --- | --- |
| K-01 | config 仓库根路径闭环核对 | R9 |
| K-02 | `ToolApprovalRequested` 等待前持久化 | ✅ 已落地(R4 波 B,2026-08-21:等待前落盘 + GUI resume 呈现待审批 + 决策落盘不重跑;CLI resume 维持 seal Denied) |
| K-03 | Desktop 人工验收(IME/1440×1024/键盘走查) | R8 波 E |
| K-04 | Desktop Changes 面(+`HunkStageService` 消费,S12-F57) | R8 波 D |
| K-05 | 本机会话格式导入(Claude jsonl / Codex rollout) | ✅ 已落地(R6 波 C,2026-08-23:双形态 compat 解析 + session_scan 发现(排除 agent sidecar) + `sessions import --from` 批量;隔离目录真实样本冒烟通过) |
| K-06 | Desktop `@`/Resources 面 | R8 波 D |
| K-07 | `rate_limit.rs` 无生产调用 | R0(裁决:删除,Hub 序列补洞随之简化) |
| K-08 | `ArtifactStreaming` 宣告与实现不一致 | R0(停止宣告)+ R3(宣告=实现同源根治) |
| K-09 | macOS `network_allow_hosts` 全拒未实现 | ✅ 已落地(R7 波 C,2026-08-23:ADR-041 D3 删除字段与死分支,网络维持 Enforce 全拒/Off·Hint 放行两档事实,egress broker 留 §3.3 候选) |
| K-10 | Anthropic Messages 能力收口 | ✅ 已落地(R5 波 C,2026-08-22:写 wire 或 HTTP 前显式拒绝;TODO 清除;真实 Anthropic 冒烟 fail-closed 入 §4) |
| S6 挂账 | ChatGPT/xAI OAuth 自然临期真实 refresh 人工验收 | R9 |
| F03 | Windows Service SCM 本机无法验收 | 候选(§3.3,需 Windows 环境) |
| F10 | 两 GUI 冒烟复跑 | R9(随定向回归) |

### 3.3 候选(未排期)

纳入排期时:在 §3.2 登记任务并入对应 `plan/R*.md` 或另立任务书,按 §6 回写约定执行。

- **多账户 factory 装配**(G1–G7/F1–F5 已确认,D1–D8 已拍板):R0 归档 account-control-v1 后,激活时按新装配面重写(归档代码经 git tag `v2-final` 可查,[docs/research/](docs/research/) 调研仍有效)。
- **远程 GUI(transport remote)**:R0 归档 TLS 实现(3,721 行);复活须按当时协议版本重评。
- **teams / goal / automation / monitor 复活**:domain 事件保留可重放;reducer 归档;对应产品面立项时另立任务。
- **GUI git 面板**(Branch/Stash/Conflict/History/Commit 服务 + StatusCache/CachedStatusService watcher):R0 波 C 归档(vcs/git 六模块 2,262 行,tag `v2-final` 可找回);产品定义后另立。
- **扩展生态整族(WASM 插件 / 市场 / Hooks / LSP)**:沿 V2 决议移出排期;预留保留(`PluginId`、`ToolCapability::ExternalPlugin`、GUI 未知 capability 隐藏);资产见 [plan/archive/S10-extensions-deferred.md](plan/archive/S10-extensions-deferred.md)。
- **对外账户池网关(F6-B)**:维持不内建。
- **K-09 选项 (a) egress broker**:本地策略代理 + 沙箱内仅放行 loopback 代理端口 + 域名白名单(srt 两层模型 + codex-network-proxy 实现,参照 [docs/references.md](docs/references.md) §7 R7 行);ADR-041 D3 已选 (b) 删除 `network_allow_hosts` 字段,本项为激活时另立任务书的候选。
- **发布 / 全量门禁 / 三平台矩阵**:须用户明确授权后另立任务(License 为硬前置)。
- **artifact 流式(GUI)**:R0 停止宣告后转候选;R3 registry 就位后接线成本低。
- **DeepSeek Harness 等候选功能池**:见 [docs/design.md](docs/design.md) §5/§6(30 项 P1–P3,继续有效)。

---

## 4. 未决事项

| 事项 | 说明 | 拍板时点 |
| --- | --- | --- |
| ADR-038 库存与产品形态 | [ADR-038](docs/adr/ADR-038-inventory-and-product-shape.md) **Accepted**(用户 2026-08-18 确认,22 项按推荐决议执行);波 0 tag `v2-final` 已打,波 A(D2–D7)、波 B(D8–D15/D20–D22)、波 C(D16 git 服务裁剪)已全部落地;波 B/C 实态核查共改判 4 项(D12、D14、D15、D16 commit.rs 补判,见 ADR 落实改判记录),本行不再是闸门 | 已确认 / R0 波 0 |
| ADR-039 目录布局 | [ADR-039](docs/adr/ADR-039-package-layout-and-no-merge-list.md) **Accepted**(用户 2026-08-19 确认):扁平 `crates/<短名>` + `apps/<name>`,目录迁移集中波 E 一次完成;不合并清单(policy/exec/auth/git/engine/protocol/testkit/transport/orchestration/workflow)固化;波 A–E 全部落地(members 21,19 库已迁扁平 `crates/`,design.md §2 已重写),本行不再是闸门 | 已确认 / R1 波 A |
| ADR-040 分支模型 | [ADR-040](docs/adr/ADR-040-session-branch-lineage.md) **Accepted**(用户 2026-08-23 确认):原生化 + append-only 单表全局 sequence + lineage 单点收编 + schema v12 回填即校验 + 压缩按分支水位;波 0/A/B/C 全部落地,本行不再是闸门 | 已确认 / R6 波 0 |
| ADR-041 沙箱信任模型 | [ADR-041](docs/adr/ADR-041-sandbox-trust-model.md) **Accepted**(用户 2026-08-23 确认):D1 macOS 写白名单正式化+读整盘 allow 挖洞(读白名单经 Darwin 25.6 实测不可行);D2 PTY 创建入 policy 闸(豁免支留用户拍板);D3 删除 network_allow_hosts 字段(egress broker 转 §3.3 候选);D4 shell 手写 tokenizer;波 A 可开 | 已确认 / R7 波 0 |
| `pawork-sdk` `handshake_exposes_version_instance_and_capabilities` 既有失败 | R0 波 B 收口发现:`clients/sdk/tests/fixtures/hello_ack.json` 内嵌 api_version 1.1,断言对比 `API_VERSION` 常量(S13-F13 已升 1.2);夹具未随 S13 波 B 升级,波 B 写入集未触碰该测试与夹具(2026-08-18 裁决) | ✅ 已修复(R1 波 E 收口按 task-guide §1 窄任务修:夹具 negotiated 对齐 1.2,现 `crates/client/tests/fixtures/hello_ack.json`,2026-08-19) |
| `plan_service::review_flow_replays_identically` 既有失败 | R0 波 A 收口发现:`revise(v2, v1, "revised", Vec::new())` 后 `steps[0]` 越界;基线 v2-final 复现,与 R0 改动无关(2026-08-18 裁决) | ✅ 已修复(R1 波 E 收口按 task-guide §1 窄任务修:测试侧改为携现有步骤修订,现 `crates/workflow/tests/plan_service.rs`,2026-08-19) |
| rmcp 3.x | ✅ 已决议升级(R2 波 C 2026-08-20:锁 `=3.1.3`;整阶段复核后 65 条 MCP 契约测试 + 隔离断言全绿,InputRequiredResult 明确 fail-closed;历史 stdio 冒烟通过但 2.2.0 基线原始输出未归档;MSRV 1.85→1.88,lock 830→826;复评条件:rmcp 下个 major 或 wire 协议变化时重跑同套门) | R2 波 C |
| 上游传递多版本残留(base64/syn/thiserror) | R2 波 D 实测 CLI 闭包:base64 0.22.1(reqwest/hyper-util)/0.23.1(rmcp)、syn 2.x(tracing-attributes/thiserror 1/ICU derives)与 3.x(async-trait/clap/serde/tokio macros)、thiserror 1.0.69(portable-pty→filedescriptor;desktop 另有 async_zip/postage/tokio-socks);均为上游传递,pawork 直控面已单版本;随上游对齐自然消除 | R9 复跑 `cargo tree -d` 核对 |
| directories 5→6 | 目录语义兼容(`dev.pawork.pawork` 布局)评估后升级或显式锁定 | ✅ 已决议升级(R2 波 B:6.0.0;整阶段复核后 macOS 快照 golden×2 均走 BaseDirs,auth 确定性覆盖 override/fallback;Windows 路径注释已校正,2026-08-20) |
| gpui 升级跟踪 | `=0.2.2` 为当前最新(ADR-035);上游发新版后评估(影响 R8 组件 API) | 出现新版时 |
| License 与 crates.io 占名 | 发布硬前置;不阻塞 R0–R9 | 发布任务前 |
| `session_bindings` 孤儿表 | R0 归档 binding 后该表无读写方;迁移 append-only,留表 + 注释登记「预留」,不回滚 DDL | ✅ 已登记(现 `crates/storage/src/session/migration.rs` v9 注释,2026-08-18) |
| probe snapshot-reconnect 间歇超时 | R3 波 A 审查发现:`crates/client/tests/probe/scenarios.rs:139-145` 10s 事件等待偶发超时(审查者 4 跑 2 败;主代理与实现方多轮全绿);该场景帧型不过本波授权门,与 R3 波 A diff 无因果路径,裁决为既有 flake | R9 复跑核对;若波 B/C 触碰 probe 时复现率上升则提前立窄任务 |
| gui_host record 失败 tracing 断言偶发失败 | R5 波 A 门禁发现(轨 b worker 与主代理门禁各复现 1 次):`crates/app/src/gui_host/tests.rs` `command_record_failure_is_counted_not_swallowed` 用 thread-local `tracing::dispatcher::set_default` 捕获事件,多线程 runtime 并行全量下 future 跨线程迁移致捕获为空;单测/串行/复跑全绿,R5 波 A 两轨均未触碰该路径,裁决为 R4 波 B 引入测试机制的既有 flake | R9 复跑核对;若复现率上升立窄任务(事件捕获改全局 subscriber 或测试串行化) |
| usage 幂等键冲突(冒烟发现) | R0 波 C 冒烟实证:现 `crates/app/src/control.rs:140` 以 `rec-{run_id}` 为 record_id,含工具调用的多轮迭代在同一 run 下产生多条内容不同的 usage 记录,命中 ledger 幂等键 (tenant, account, record_id) 判 Conflict;失败记录入重试队列,后续运行反复重放同一 warn(如 `rec-run-1787064020223-1`)。既有缺陷,与 R0 改动无关;按 task-guide §1 窄任务修(record_id 加迭代序号或聚合为每 run 一条) | 阶段外窄任务,不阻塞 R0 |
| usage 哨兵口径差异 | R4 波 D 登记:host 侧 `control.rs` 硬填 `upstream_attempt: Some(1)`,而 control-plane legacy v1 JSON 默认 `upstream_attempt=None`;两者均符合 D1 单机语义但口径不一,波 D 已 doc+pin 钉死 host 侧(值零变化) | R9 复查是否统一口径 |
| R4 人工验收项 | 波 B/C/D 登记的自动化外事项:K-02 真实 kill -9 进程崩溃冒烟(app 层 drop+reopen 回归已过)、GUI 审批恢复人工验收、ACP 双连接传输层交错压测(种子为单 Host 双会话)、Zed 真实冒烟 | 人工验收,不阻塞 R5 开工 |
| PWB1 protected 消费者 | ✅ 已落地(R5 波 C,2026-08-22:宿主 `crates/app/src/protected.rs` 注入 ProtectedBlobStore,storage `protected` feature 进 pawork 闭包;`chacha20poly1305` 仅随 feature 进入;instance-level BlobScope `instance-reasoning` 已接受偏差) | 完成 |
| R5 真实 Anthropic 冒烟 | 波 C 自动化门禁绿,但本机无 `ANTHROPIC_API_KEY`/`GLM_API_KEY`,`~/.pawork/auth.json` 仅有 glm/chatgpt/xai 等条目、无 anthropic;按 fail-closed 未发真实请求。任务书矩阵内 GLM Anthropic 端点冒烟留人工验收 | 人工验收,不阻塞 R6 波 0 ADR 起草 |
| R6 真实 fork/compact 冒烟 | 波 B 的 storage/engine/app/protocol/Desktop 自动回归与 `cargo check -p pawork` 已绿；真实 Provider 的 main/fork 双支续聊与 fork 后 compact/resume 会消耗外部凭证，本波未执行、未读取凭证 | 人工验收，R6 已🟢收口；归档前完成或明确 fail-closed |
| R6 波 C P3 登记 | ① Claude 同消息多 text part 无分隔拼接(compat.rs flush_claude_text);② 畸形缺 id 的 tool_use/tool_result 对按行号回退 id 必失配 → 整文件 fail-closed(真实格式均带 id,仅畸形输入触发);③ CLI 嗅探读首行整行,单行超大致敏内存占用为有界接受(导入下一步本就整文件读入);④ 部分损坏(有合法记录+坏行)静默导入残缺内容且 CLI 未透出 unknown_fields;⑤ 扫描根自身为 symlink 时直接报错(dotfile 管理器场景) | R9 复查;出现真实影响再立窄任务 |
| R7 命令级交互审批(ADR 候选) | R7 波 B 落地 ADR-041 D2 时用户拍板:terminal_create 的 AskUser 一律 fail-closed 落 Deny——GUI 审批回路(ToolApprovalRequired/ToolApprove/GuiApprovalHost)以 run_id+tool_call_id 为键、按 session 广播,terminal_create 只有 workspace_id,且 ADR-041 声明不含 wire 变更,命令级审批无承载。后果:AlwaysAsk/AskForWrites 档不能创建终端,仅 AskForDangerous+安全 shell 放行。如需命令级交互审批,须另立 ADR 做 wire 演进(审批事件/命令泛化)+ desktop 渲染面 | 另立 ADR 时 |
| R7 Desktop PTY 面板冒烟 | 波 B 闸后桌面端真实冒烟未执行(默认 ReadOnly 应如实拒绝、--approval-mode ask-for-dangerous 可建会话、标注文案可见);desktop 只消费 terminal_session_id,响应新字段(sandboxed/policy/approval_mode/note)暂无渲染面,如实标注 UI 属 R8 面 | 人工验收,不阻塞波 C |
| shell wrapper 升档变松(R7 波 B P2-1，已接受) | tokenizer 化移除兜底正则后,nohup/env/xargs 等程序位 wrapper 内危险命令不再升档(旧为子串偶合命中;同批消除 echo 'rm -rf /' 字面量误报);灾难地板集合不变、沙箱 Enforce 不受影响;crates/policy/src/shell.rs 模块 doc 已如实登记 | R9 复查;需收紧时立窄任务做有界 launcher 剥离 |
| pawork-policy regex 死依赖 | 波 B 后 shell.rs 零 regex 使用,Cargo.toml 声明未随删(写入集纪律);tokenizer 无第三方依赖 | 后续窄任务清理 |
| StoredCredential serde alias 兼容期 | R5 波 B(2026-08-22)keychain 词汇迁移:字段改名 secret_service/secret_account,保留 #[serde(alias = "keychain_service"/"keychain_account")] 读旧名兼容一个版本期(读旧写新 + 迁移测试已落地,auth.json entries 落盘形状本就无该词汇、零变化) | 兼容期满(一个版本期)后随 R6 或 R9 移除 alias |
| 波次门禁膨胀(编译成本) | R3–R5 实测测试体秒级,慢在 rustc + worker/reviewer/主代理重复 Cargo。2026-08-22 已把默认门禁收成写入集 `cargo test -p --offline --lib --tests`、审查者不编译、邻包 golden/e2e/desktop/`cargo check -p pawork` 仅收口且有改动才跑;dev/test profile 开 incremental + unpacked split-debuginfo;R1 遗留 incremental 用 scripts/clean-stale-incremental.py 按前缀清理。未做:合并 protocol 11 个 [[test]] crate、拆 pawork-client 对 pawork-app 的 dev-dep。 | 后续波次遵守 task-guide §3.3;协议测试箱合并另立窄任务 |
| protocol-probe `snapshot-reconnect` 偶发超时 | R1 波 B 收口发现:批量 `cargo test` 下 `snapshot-reconnect` 场景一次「receive frame timed out after 10s」,单独连跑 3 次全绿;波 B 对该链路只有纯路径改名,判定为既有偶发;R9 全量复跑时验证 | R9 复跑核对 |
| ModelList 与 switch_provider 模型目录不对称(冒烟发现) | R1 波 E 冒烟实证:`models_overview`(`crates/app/src/lib.rs`)把运行期 /models 探测结果合并进 ModelList,而切换路径原先只查静态注册表。R1 整阶段审查新增按明确 `(provider, model)` 的动态探测/惰性合并解析,未知模型仍 fail-closed;`switch_provider_accepts_runtime_discovered_model` 回归通过,隔离 instance 的 desktop probe 实测 glm-4.7 可完成首轮并继续切换模型 | ✅ 已修复(R1 整阶段审查,2026-08-19) |
| client 事件泵抢占命令错误帧(冒烟发现) | R1 波 E 冒烟实证:`FrameWant::Event` 原先会匹配所有 `ServerFrame::Error`,从而掩盖命令真实错误。R1 整阶段审查改为 Response/Snapshot/Resume 只接同 request_id 的错误,Event 只接 `request_id=None` 的连接级错误;`frame_wants_route_errors_by_request_id` 回归通过,desktop probe 的请求/切换/取消/断线存活链路通过 | ✅ 已修复(R1 整阶段审查,2026-08-19) |

---

## 5. 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 归档误删未来必需资产 | R0 执行前打 git tag `v2-final`;归档决议逐项过 ADR-038;domain 事件类型一律保留(重放红线) |
| 大规模 git mv 丢失历史可读性 | R1 目录移动与内容改动分开提交;`git log --follow` 验证抽查;合并波内「先 mv 后改」两步走 |
| 合并引入循环依赖 | R1 每波收口跑 `cargo tree` 断言(已知唯一风险点:policy 不得卷入含 tools 的包);desktop deny-list 断言随迁 |
| 冻结契约被「顺手」破坏 | golden 先行(信封/DDL/PWB1/帧/headless);serde 形状 diff 审查;R6/R7 之外不允许 schema 变更 |
| feature 传染 | storage 的 session/blob/protected 分 feature;providers 六通道 feature 保留;合并波逐包核对 `cargo tree -p pawork` 闭包不膨胀 |
| 编译粒度变粗(providers/storage 单体) | 记入 ADR-039 决策代价;增量构建实测对比记录 |
| 重构期行为回退 | 每波收口复跑该域定向测试;R9 全量复跑安全红线/golden/协议三类;真实冒烟按 §1.1 矩阵 |
| host 拆解(R4)牵动所有通道 | R3 先建协议 golden + registry,R4 在契约测试护航下拆;波内单一 owner |
| 沙箱重设计三平台行为差异 | R7 独立 ADR + 平台探测回归;fail-closed 语义不放宽(ADR-031 可观测回退保持) |
| GUI 视觉漂移 | hover/active 等有意改动先更新 [design/README.md](design/README.md) 再实现;R8 波 E 按 1440×1024 基准人工对照 |

---

## 6. 状态回写约定

- **阶段任务**:阶段收尾更新 §2 状态列 + 对应 `plan/R*.md` 退出标准打勾;experimental/延期项在 §4 或 §3.3 登记激活条件。
- **阶段外任务**:开启时写入 §3.1b；完成后移入 §3.1 并登记产出链接。候选仍走 §3.3。
- **ADR**:R0/R1/R6/R7 的 ADR 编号续接(ADR-038 起),落 [docs/adr/](docs/adr/);状态 Proposed → 用户确认 → Accepted 后方可执行对应破坏式改动。
- **候选转正**:按 §3.3 流程登记。
- **模型评估记录**:注明通道与模型;默认属 §1.1 矩阵,例外须写明理由。
- 完整收尾清单(测试、冒烟、报告格式)见 [docs/task-guide.md](docs/task-guide.md) §8。
