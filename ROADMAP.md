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
> | [docs/references.md](docs/references.md) | 参照项目手册 |
> | [docs/task-guide.md](docs/task-guide.md) | 任务实现规范(开启 / 进行 / 收尾公共约定) |
> | [docs/v2-summary.md](docs/v2-summary.md) | V2 归档总结(S0–S13 交付、冻结契约、遗留债务) |
> | [docs/v1-migration-reference.md](docs/v1-migration-reference.md) | V1 迁移词典(冻结参考) |
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
| [R0](plan/R0-inventory-decisions.md) | 决策收口与休眠库存裁决 | ADR-038(单机 vs 多租户、remote/teams/三域/account-control 去留);归档约 3.3–3.8 万行零消费者代码;K-07 删除、K-08 停止虚假宣告;死 feature/死声明清理 | 全仓休眠面(workflow/orchestration/control-plane/transport/diagnostics/net/session/engine/host) | 无 | ⚪ |
| [R1](plan/R1-package-consolidation.md) | 包合并 39→21 | ADR-039(目标布局 + 目录扁平化);api→domain、sqlite+session+blob→storage、net+core+adapters→providers、core+resources+config+compat→workspace、mcp→tools、quota+provider-control→control-plane、gui-server→app、channels→cli、sdk→client、diagnostics 解散、probe→client 测试;golden 随迁 | 全部 crate 的 Cargo.toml/目录/use 路径;design.md §2 重写 | R0 | ⚪ |
| [R2](plan/R2-dependency-governance.md) | 依赖治理 | rand/parking_lot/base64 本地化;notify 8、windows 0.61、portable-pty 0.9、ts-rs 12、reqwest 0.13、toml 1.1、rusqlite 0.40、sha2 0.11 升级;lock 多版本去重断言;rmcp 3.x 专项 | 各 crate Cargo.toml + 少量调用点 | R1 | ⚪ |
| [R3](plan/R3-protocol-unification.md) | 协议与投影同源化(T3+T5) | 单一 command/capability registry,GUI 帧/headless/ACP 三通道 mapping 同源派生(宣告=授权=实现);Timeline 投影 reducer 下沉 protocol 共享模块,host/desktop 同源 + 投影 golden;OnFailure 档位裁决 | protocol、app、cli(headless/acp)、client、desktop projection | R1(R2 可并行) | ⚪ |
| [R4](plan/R4-host-decomposition.md) | 宿主拆解与可靠性内核(T2+T8+T9) | app 单体按领域服务拆分(巨 match → registry 分发);幂等 CommandLedger 持久化 + K-02 审批等待前落盘;ACP host actor 化;降级事件化契约(消灭静默 `let _`/回退) | app、cli、storage(幂等表)、protocol(降级事件) | R3 | ⚪ |
| [R5](plan/R5-provider-neutrality.md) | Provider 中立化与凭证收口(T6+T11) | provider_hints 命名空间契约(删存储层 provider 键名清单);通道 preset 数据化(新增通道单点登记);credential locator 合一 + keychain 词汇迁移;K-10 Anthropic 能力收口 + CapabilityNegotiator 接线;ReasoningProtector 持久化(PWB1 首个生产消费者) | providers、auth、storage(event_store)、workspace(config)、engine 守护测试 | R1(建议 R4 后) | ⚪ |
| [R6](plan/R6-session-branching.md) | 会话分支模型原生化(T4,ADR-040) | 事件/投影原生 branch lineage(替换后补 `branch_id` 列 + 反查回填);schema v11 迁移 + 旧库升级 golden;压缩按分支水位;K-05 本机会话导入 | storage、engine(compact)、app(resume/fork)、desktop projection | R4(投影同源已就位) | ⚪ |
| [R7](plan/R7-sandbox-isolation.md) | 执行面真隔离(T7,ADR-041) | macOS Seatbelt 按需白名单 profile(替换整盘只读+deny);PTY 入 policy 闸;shell 风险分类结构化解析;K-09 egress 决策;三平台沙箱回归 | exec、policy、tools(run_command)、app(PTY 装配) | R1(可与 R3–R6 并行) | ⚪ |
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

### 3.2 V2 遗留债务 → V3 阶段映射

V2 收口时的 K-01~K-10 与其他挂账项(原委见 [docs/v2-summary.md](docs/v2-summary.md) §6)全部并入 V3 阶段,不再单列执行:

| 遗留项 | 内容 | V3 归属 |
| --- | --- | --- |
| K-01 | config 仓库根路径闭环核对 | R9 |
| K-02 | `ToolApprovalRequested` 等待前持久化 | R4 波 B |
| K-03 | Desktop 人工验收(IME/1440×1024/键盘走查) | R8 波 E |
| K-04 | Desktop Changes 面(+`HunkStageService` 消费,S12-F57) | R8 波 D |
| K-05 | 本机会话格式导入(Claude jsonl / Codex rollout) | R6 波 C |
| K-06 | Desktop `@`/Resources 面 | R8 波 D |
| K-07 | `rate_limit.rs` 无生产调用 | R0(裁决:删除,Hub 序列补洞随之简化) |
| K-08 | `ArtifactStreaming` 宣告与实现不一致 | R0(停止宣告)+ R3(宣告=实现同源根治) |
| K-09 | macOS `network_allow_hosts` 全拒未实现 | R7 波 C |
| K-10 | Anthropic Messages 能力收口 | R5 波 C |
| S6 挂账 | ChatGPT/xAI OAuth 自然临期真实 refresh 人工验收 | R9 |
| F03 | Windows Service SCM 本机无法验收 | 候选(§3.3,需 Windows 环境) |
| F10 | 两 GUI 冒烟复跑 | R9(随定向回归) |

### 3.3 候选(未排期)

纳入排期时:在 §3.2 登记任务并入对应 `plan/R*.md` 或另立任务书,按 §6 回写约定执行。

- **多账户 factory 装配**(G1–G7/F1–F5 已确认,D1–D8 已拍板):R0 归档 account-control-v1 后,激活时按新装配面重写(归档代码经 git tag `v2-final` 可查,[docs/research/](docs/research/) 调研仍有效)。
- **远程 GUI(transport remote)**:R0 归档 TLS 实现(3,721 行);复活须按当时协议版本重评。
- **teams / goal / automation / monitor 复活**:domain 事件保留可重放;reducer 归档;对应产品面立项时另立任务。
- **GUI git 面板**(Branch/Stash/Conflict/History 服务):R0 归档;产品定义后另立。
- **扩展生态整族(WASM 插件 / 市场 / Hooks / LSP)**:沿 V2 决议移出排期;预留保留(`PluginId`、`ToolCapability::ExternalPlugin`、GUI 未知 capability 隐藏);资产见 [plan/archive/S10-extensions-deferred.md](plan/archive/S10-extensions-deferred.md)。
- **对外账户池网关(F6-B)**:维持不内建。
- **发布 / 全量门禁 / 三平台矩阵**:须用户明确授权后另立任务(License 为硬前置)。
- **artifact 流式(GUI)**:R0 停止宣告后转候选;R3 registry 就位后接线成本低。
- **DeepSeek Harness 等候选功能池**:见 [docs/design.md](docs/design.md) §5/§6(30 项 P1–P3,继续有效)。

---

## 4. 未决事项

| 事项 | 说明 | 拍板时点 |
| --- | --- | --- |
| ADR-038 库存与产品形态 | 单机优先 vs 多租户、remote/teams/三域/account-control/lifecycle/identity_schema/OTel exporter 去留——任务书 [plan/R0](plan/R0-inventory-decisions.md) 已给推荐决议,须用户确认后执行 | R0 波 0 |
| ADR-039 目录布局 | 推荐扁平 `crates/` + `apps/`(19 库规模下功能域目录成为噪音);备选保留域目录 | R1 波 A |
| ADR-040 分支模型 | 推荐原生 lineage(Fork 是已交付能力,删除属产品倒退);备选冻结线性 + 删 Fork | R6 波 0 |
| ADR-041 沙箱信任模型 | macOS 白名单 profile 的兼容性代价(Darwin 25 实测)与 PTY 语义 | R7 波 0 |
| rmcp 3.x | wire 兼容性未评估;若破坏 MCP golden 则锁 2.2 并登记 | R2 波 C |
| directories 5→6 | 目录语义兼容(`dev.pawork.pawork` 布局)评估后升级或显式锁定 | R2 波 B |
| gpui 升级跟踪 | `=0.2.2` 为当前最新(ADR-035);上游发新版后评估(影响 R8 组件 API) | 出现新版时 |
| License 与 crates.io 占名 | 发布硬前置;不阻塞 R0–R9 | 发布任务前 |
| `session_bindings` 孤儿表 | R0 归档 binding 后该表无读写方;迁移 append-only,留表 + 注释登记「预留」,不回滚 DDL | R0 执行时登记 |
| PWB1 protected 消费者 | R5 将 ReasoningProtector 接到 ProtectedBlobStore(兑现 S6 注释承诺);若 R5 裁决删除则 PWB1 契约转冻结候审 | R5 波 C |

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
- **阶段外任务**:开启/完成时更新 §3.2/§3.3;完成后移入 §3.1 并登记产出链接。
- **ADR**:R0/R1/R6/R7 的 ADR 编号续接(ADR-038 起),落 [docs/adr/](docs/adr/);状态 Proposed → 用户确认 → Accepted 后方可执行对应破坏式改动。
- **候选转正**:按 §3.3 流程登记。
- **模型评估记录**:注明通道与模型;默认属 §1.1 矩阵,例外须写明理由。
- 完整收尾清单(测试、冒烟、报告格式)见 [docs/task-guide.md](docs/task-guide.md) §8。
