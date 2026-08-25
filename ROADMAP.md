# Pawork 路线图

> 本文档是 Pawork 的**任务事实源**：当前指针、剩余任务、阶段外任务、未决登记、候选池，以及任务开启/进行/收尾的工作约定。**已完成的工作不留在本文**——V2（S0–S13）与 V3（R0–R9 各已收口波次）的交付史、决策记录与整改细节见 [docs/history.md](docs/history.md)。
>
> 文档导航见 [README.md](README.md)；架构红线与冻结契约见 [docs/architecture.md](docs/architecture.md)；工程约定基线见 [AGENTS.md](AGENTS.md)。

---

## 1. 当前指针

| 字段 | 值 |
| --- | --- |
| 活动线 | V3 重构（R0–R9）收尾期：R0–R7 🟢 已收口（史见 [docs/history.md](docs/history.md)） |
| R8（GUI 组件化） | 🔵 波 A–D ✅、波 E 自动化部分 ✅（提交 528ab3d）+ 整阶段审计修复 ✅（2026-08-25）；**仅剩 K-03 人工走查签字**（[docs/gui-design.md](docs/gui-design.md) 附录 A.2 十一项），签字后 R8 🟢 |
| R9（一致性收口） | 🔵 波 A1 🟢（2026-08-25：P/S/R 谱系确认 + [Spec 文档集](docs/spec/README.md)建立）；**下一波 A2**（其余文档/登记/红线断言一致性核对 + 发现项修复），其后波 B/C 见 [plan/R9-consistency-closeout.md](plan/R9-consistency-closeout.md) |
| 阻塞 | 无 |

状态符号：⚪未开始 · 🔵进行中 · 🟢已完成 · ⚠️阻塞。自动选波以本表为准，与任务书、工作区实态交叉校验；冲突时**工作区实态 > 本表 > 任务书**，更新一致后再开工。

---

## 2. 剩余任务

### 2.1 V3 收尾（进行中）

| 任务 | 内容 | 事实源 |
| --- | --- | --- |
| R8 K-03 人工签字 | Desktop 人工走查（IME / 1440×1024 / 键盘 / 菜单 / 滚动 / DiffView 横滚等十一项，自动化取证截图已备） | [docs/gui-design.md](docs/gui-design.md) 附录 A；[plan/R8-gui-components.md](plan/R8-gui-components.md) |
| R9 波 A2 | 其余文档/登记/红线断言一致性核对 + 发现项修复 | [plan/R9-consistency-closeout.md](plan/R9-consistency-closeout.md) |
| R9 波 B | K-01 config 路径闭环核对；S6 OAuth 自然临期 refresh 人工验收（V2 唯一未收口项）；两 GUI 冒烟复跑（F10） | 同上 |
| R9 波 C | 安全红线/golden/协议定向回归全量复跑；§4 挂账复查（probe flake、usage 哨兵口径、Seatbelt 探针补强、shell wrapper 收紧评估、`canonical_within` 残余等）；遗留与候选登记收口 | 同上 |

### 2.2 V3 之后

V3 是重构线，不新增用户可见功能。R9 收口后的下一条产品线**须先立产品目标**（候选来源：§5 候选池 + [docs/design.md](docs/design.md) §3 已确认 G1–G6 / §4 候选 28 项），由用户拍板后另立任务书；不追溯扩张已收口阶段。发布 / 全量门禁 / 三平台矩阵须用户明确授权后另立任务（License 为硬前置）。

---

## 3. 阶段外任务

### 3.1 进行中

当前无进行中的阶段外任务。最近完成：文档体系重构（2026-08-25，包级 Spec + 存档体系，新旧对照见 [docs/history.md](docs/history.md) 附节）。

### 3.2 待办窄任务（不阻塞主线）

| 任务 | 内容 | 来源 |
| --- | --- | --- |
| usage 幂等键冲突修复 | `crates/app/src/control.rs` 以 `rec-{run_id}` 为 record_id，多轮迭代同 run 产生多条 usage 记录判 Conflict 反复重放 warn；修法：record_id 加迭代序号或聚合为每 run 一条 | R0 冒烟发现 |
| pawork-policy regex 死依赖清理 | R7 波 B tokenizer 化后 shell.rs 零 regex 使用，Cargo.toml 声明未随删 | R7 波 B |
| Spec 撰写发现的代码侧小漂移 | ① workflow/orchestration Cargo.toml description 仍含 R0 已归档域（「五合一 reducer」「Agent Teams」）；② workflow 声明未使用的 regex/tracing 依赖；③ orchestration `budget.rs` 硬超限文档注释写 `>` 而实现为 `>=`（`budget.rs:233,245`，二选一对齐）；④ `domain/src/degrade.rs:7` 注释「26 帧 golden」过期（实为 32）；⑤ `protocol/src/app/event.rs:186,282,342` 注释仍引用 R0 已归档的 `teams::` 类型（改为「原 teams crate（已归档）」措辞） | 包级 Spec 撰写（2026-08-25） |
| StoredCredential serde alias 移除 | R5 波 B keychain→secret 词汇迁移保留 `#[serde(alias)]` 读旧兼容一个版本期；期满移除 | R5 波 B |
| protocol 测试箱合并 | protocol 11 个 `[[test]]` crate 合并降编译成本；顺带评估拆 pawork-client 对 pawork-app 的 dev-dep | 波次门禁膨胀登记 |

### 3.3 V2 遗留未收口项

| 遗留项 | 内容 | 归属 |
| --- | --- | --- |
| K-01 | config 仓库根路径闭环核对 | R9 波 B |
| S6 挂账 | ChatGPT/xAI OAuth 自然临期真实 refresh 人工验收 | R9 波 B |
| K-03 | Desktop 人工验收 | R8 波 E（仅剩签字） |
| K-04 残余 | Changes 面只读已交付；git_stage/HunkStageService 接线需 wire 扩展 | 另立 ADR 时（§4） |
| F03 | Windows Service SCM 本机无法验收 | 候选（需 Windows 环境） |

其余 K 项（K-02/05/06/07/08/09/10）均已落地，原委见 [docs/history.md](docs/history.md)。

---

## 4. 未决事项（开放登记）

已关闭条目（ADR-038~041 确认、既有测试失败修复、rmcp/directories 升级决议、ModelList 不对称、client 错误帧路由、gui_host record flake 根因等）已移入 [docs/history.md](docs/history.md)，此处只留**开放项**：

| 事项 | 说明 | 复查时点 |
| --- | --- | --- |
| 上游传递多版本残留 | base64 0.22/0.23、syn 2.x/3.x、thiserror 1.x（portable-pty→filedescriptor 等）均为上游传递，直控面已单版本；随上游对齐自然消除 | R9 复跑 `cargo tree -d` |
| gpui 升级跟踪 | `=0.2.2` 为当前锁定（ADR-035，V1 归档）；上游发新版后评估（影响 Desktop 组件 API） | 出现新版时 |
| License 与 crates.io 占名 | 发布硬前置；不阻塞当前线 | 发布任务前 |
| probe `snapshot-reconnect` 偶发超时 | 10s 事件等待偶发超时，多轮复跑全绿，判既有 flake | R9 复跑核对 |
| usage 哨兵口径差异 | host 侧硬填 `upstream_attempt: Some(1)` vs control-plane legacy `None`，均符合单机语义但口径不一（已 doc+pin 钉死） | R9 复查是否统一 |
| R4 人工验收项 | K-02 真实 kill -9 崩溃冒烟、GUI 审批恢复人工验收、ACP 双连接交错压测、Zed 真实冒烟 | 人工验收 |
| R5 真实 Anthropic 冒烟 | 本机无 anthropic 凭证，fail-closed 未发真实请求；GLM Anthropic 端点冒烟留人工验收 | 人工验收 |
| R6 真实 fork/compact 冒烟 | 真实 Provider 的 main/fork 双支续聊与 fork 后 compact/resume 未执行（消耗外部凭证） | 人工验收 |
| R6 波 C P3 登记 | ① Claude 多 text part 无分隔拼接；② 畸形缺 id 对回退失配→整文件 fail-closed；③ 嗅探首行整行读取有界接受；④ 部分损坏静默导入残缺且 CLI 未透出 unknown_fields；⑤ 扫描根自身为 symlink 报错 | R9 复查；真实影响再立窄任务 |
| R7 命令级交互审批（ADR 候选） | terminal_create 的 AskUser 一律 fail-closed 落 Deny（审批回路以 run 为键，命令级无承载）；AlwaysAsk/AskForWrites 档不能创建终端。如需命令级审批，须另立 ADR 做 wire 演进 + desktop 渲染面 | 另立 ADR 时 |
| R7 Desktop PTY 面板冒烟 | 桌面端真实冒烟未执行；desktop 暂无响应新字段（sandboxed/policy/note）渲染面 | 人工验收 |
| shell wrapper 升档变松（已接受） | tokenizer 化后 nohup/env/xargs 程序位 wrapper 内危险命令不再升档（旧为子串偶合）；灾难地板与沙箱 Enforce 不受影响 | R9 复查；需收紧立窄任务（有界 launcher 剥离） |
| Seatbelt 真机探针补强 | `/tmp`、`/dev` 写洞双形态、symlink 根、`(deny network*)` 缺真机种子（golden 已钉死形状） | R9 或另立窄任务 |
| Desktop 真窗口启动门禁缺口 | controller.connect 非 tokio 上下文路径无自动门禁（崩溃已修）；候选：connect 回归测试或 CI 窗口感知冒烟 | R9 |
| R8 HunkStageService 接线（ADR 候选） | Changes 面只读已交付；stage/unstage/hunk 双向 wire 与审批语义须随协议扩展一并设计 | 另立 ADR 时 |
| R8 「@」补全 query | composer 「@token」候选浮层未做，需 host file-index 模糊补全 query + desktop 浮层 | 出现需求时立窄任务 |
| R8 Resources「已加载规则」分区 | 无 host 出口不画；待 gui 可用 query 暴露已加载 AGENTS.md/技能清单时补渲染面 | host 出口落地时 |
| Desktop 键盘导航缺口 | 菜单 ↑/↓ 导航与 grouping/scope 触发器 tab stop 未实现（基准 §3.6 既有承诺）；K-03 走查确认缺口范围无新增 | 出现真实需求时立窄任务 |
| K-03 漂移：窄窗响应式（已拍板接受） | 1080–1279 宽度 TaskRail 收敛未实现（V2 起即固定 288px）；固定宽维持现状，转候选 | 出现真实需求时 |
| R7/R8 审计 P3 集 | Windows Job 单测缺口（Windows CI 引入时）；终端闸 AlwaysAsk 专项单测缺口；BackToBottom 滚轮死区；desktop 心跳泵无自动测试；泵错误路径 state.client 竞态（窗口极窄）；main.rs 窗口尺寸字面量；extension.rs mcp_list 死分支 | 触碰同文件时 |
| gpui 渲染面无自动门禁 | 菜单开合/跟随滚动/变高虚拟化/hover 等渲染行为无 gpui 测试设施可依，靠真窗口截图 + K-03 人工走查 | gpui 上游提供测试设施或另立 harness 任务时 |
| S12-CR09-05 残余 | `crates/workspace/src/resources/io.rs` `canonical_within` 自写 canonicalize+前缀比较（资源加载专用，语义同源）；语义矩阵见 [docs/architecture.md](docs/architecture.md) §3.3 | R9 或触碰同文件时改复用 policy 内核 |

---

## 5. 候选池（未排期）

纳入排期时：在 §3 登记任务并入对应 `plan/*.md` 或另立任务书，按 §7.5 回写约定执行。

- **多账户 factory 装配**（G1–G6/F1–F5 已确认，D1–D8 已拍板）：激活时按新装配面重写（归档代码经 git tag `v2-final` 可查，调研见 [docs/references.md](docs/references.md) 附录）。
- **远程 GUI（transport remote）**：R0 归档 TLS 实现；复活须按当时协议版本重评。
- **teams / goal / automation / monitor 复活**：domain 事件保留可重放；reducer 归档；对应产品面立项时另立任务。
- **GUI git 面板**（Branch/Stash/Conflict/History/Commit + StatusCache watcher）：R0 归档（tag `v2-final` 可找回）；产品定义后另立。
- **扩展生态整族（WASM 插件 / 市场 / Hooks / LSP）**：移出排期；预留保留（`PluginId`、`ToolCapability::ExternalPlugin`、GUI 未知 capability 隐藏）；资产清点见 [docs/history.md](docs/history.md)。
- **对外账户池网关（F6-B）**：维持不内建。
- **K-09 选项 (a) egress broker**：本地策略代理 + 沙箱内仅放行 loopback 代理端口 + 域名白名单（srt 两层模型 + codex-network-proxy 参照）；ADR-041 D3 已选 (b) 删字段，本项为激活时另立任务书的候选。
- **artifact 流式（GUI）**：R0 停止宣告后转候选；registry 就位后接线成本低。
- **候选功能池 28 项**（P1 5 / P2 17 / P3 6）：见 [docs/design.md](docs/design.md) §4；已确认扩展功能族 G1–G7 见同文 §3。
- **发布 / 全量门禁 / 三平台矩阵**：须用户明确授权后另立任务（License 为硬前置）。

---

## 6. 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 冻结契约被「顺手」破坏 | golden 先行（信封/DDL/PWB1/帧/headless）；serde 形状 diff 审查；ADR 之外不允许 schema 变更（契约清单见 [docs/architecture.md](docs/architecture.md) §3.2） |
| 重构期行为回退 | 每波收口复跑该域定向测试；R9 全量复跑安全红线/golden/协议三类；真实冒烟按 §7.4 矩阵 |
| feature 传染 | storage 的 session/blob/protected 分 feature；providers 六通道 feature 保留；触碰装配时核对 `cargo tree -p pawork` 闭包不膨胀 |
| 归档误删未来必需资产 | git tag `v2-final` 兜底；domain 事件类型一律保留（重放红线）；复活条件登记 §5 |
| GUI 视觉漂移 | hover/active 等有意改动先更新 [design/README.md](design/README.md) 视觉基准再实现；按 1440×1024 基准人工对照 |
| 沙箱平台行为差异 | 平台探测回归；fail-closed 语义不放宽（ADR-031 可观测回退保持） |

---

## 7. 任务约定（开启 / 进行 / 收尾）

### 7.1 任务开启

**波次任务**（`plan/*.md` 内的阶段波次）由主代理按以下编排执行；**窄任务**（单点修复、单文件文档、一条 golden）用三行最小提示词直接开工：

```text
按 ROADMAP.md §7 执行 plan/<任务书>.md 的〈波次/任务名〉。
范围：〈可选——写入集或包边界限定；不写则以任务书该波写入集为准〉
凭证：〈auth 文件已就绪 / 本任务无需真实 key〉
```

波次编排流程（主代理）：

1. **选波**：一次只做一个波次。取 §1 指针的下一波，与任务书、工作区实态交叉校验；硬前置须 🟢；用户覆盖立即生效。**ADR 闸门**：破坏式决议须用户确认 ADR Accepted 后才可开工，主代理不代替用户拍板。
2. **开启核对**（主代理亲自读）：任务书全文；本波涉及的冻结契约（[docs/architecture.md](docs/architecture.md) §3.2）与 golden 位置；写入集各包 Spec（[docs/spec/crates/](docs/spec/README.md)，只读写入集包，禁止全量通读）；跨包链路才读 [docs/spec/flows.md](docs/spec/flows.md) 对应一条。用户可见行为变更时核对 [docs/spec/](docs/spec/README.md) 相关篇目。需要真实 key 而凭证缺失即 fail-closed 终止并向用户索取，不静默跳过、不降级 mock。
3. **并行核查**（只读，≤3 路）：任务书证据（消费者/行数/调用点）按工作区实态重验；圈定契约面（改前必须先有 golden）与影响面（写入集外被牵动的 use/测试/断言）。实态与任务书冲突以实态为准并回写任务书。范围小的波次可减为主代理自查。
4. **本波设计**（主代理在会话内写）：目标/非目标、事实源路径、涉及契约、写入集、验证命令、派发图。需要 ADR 或与冻结契约冲突时先问用户。
5. **实现**：核查结束再实现。并行度按任务书标注；写入集互不重叠；契约文件与装配收口（`crates/app`、`apps/pawork`）单一 owner 串行。归档动作统一「移出 workspace + 删除源目录」。

子代理提示词统一骨架（核查 | 实现两种角色）：

```text
你是 Pawork 的〈核查 | 实现〉子代理。只做本提示词里的范围。
规范：仓库根 AGENTS.md + ROADMAP.md §7；写入集各包 docs/spec/crates/<pkg>.md 实现前必读（禁止读未列入写入集的包；Spec 冲突以源码为准）；跨包链路才读 docs/spec/flows.md 对应一条。
任务：〈任务书路径 + 波次一句话 + 设计切片（实现角色必填）〉
范围：〈核查：只读路径清单；实现：允许写入的包/目录清单〉
产出：〈核查：逐条证据核验（路径+行号）与差异；实现：实际写入文件、验证命令与结果、未做项、计划偏差〉
禁止：超范围改动；改冻结契约 serde/磁盘/线上形状；git commit/push/tag；Secret 入仓库或日志；cargo --workspace / clippy 门禁 / cargo clean；并行轨同时跑 cargo；重复编译已绿命令；默认跑 protocol golden/probe/spawn_e2e/desktop/cargo check -p pawork（除非本波实际改了对应文件）。
```

### 7.2 进行中纪律

架构红线全文见 [AGENTS.md](AGENTS.md) §2 与 [docs/architecture.md](docs/architecture.md) §1，违反须先升级 ADR 或问用户。重构纪律：

- **消费面先行**：保留在主 workspace 的模块必须有真实装配点；零消费者代码归档，不以 experimental feature 库存。
- **合并不裁剪契约**：契约类型整组平移、零裁剪，golden/测试随迁；宁可字段闲置，不做「先简后改」。
- **破坏式改动边界**：允许破坏内部代码组织与 API；不允许静默破坏磁盘/线上格式、CLI 用户可见行为与安全语义（fail-closed 只紧不松）。
- **平台与输出**：macOS 实测；Linux/Windows 保持编译、交叉 `cargo check` 可选。`--json` 模式 stdout 只承载 JSONL，文本与日志走 stderr。

### 7.3 测试纪律

- **少测试、无全量门禁**：只做能证明本任务核心行为的关键定向测试；不跑 Workspace Full Gate、不做 clippy/fmt 门禁。**三类关键测试不推迟**：安全红线定向回归；持久化与重放契约 golden；协议与解析 golden/种子。
- **默认门禁死表**（每波一条 Cargo 进程）：`cargo test -p <crate> --offline --lib --tests`；多包一次多个 `-p` 但仍是一个 Cargo 进程，不用 `--workspace`。`cargo check -p <crate>` 仅在该包无测试或只需类型检查时用。
- **默认不跑**（除非本波实际改了对应文件，且只由主代理收口跑一次）：protocol golden、client probe、spawn_e2e、desktop、`cargo check -p pawork`。合并/归档波才补 `cargo tree` 断言（无环、`-p pawork` 闭包不膨胀）。
- engine/工具循环逻辑回归全部走 MockProvider；真实 API 只承担冒烟与 env 门控 `--ignored` 测试。
- **Cargo 串行**：全会话同一时刻只允许一个 Cargo 进程；并行轨不得抢同一 `target/` 锁。禁止 `cargo clean`；stale incremental 用 `python3 scripts/clean-stale-incremental.py` 按前缀清理，禁止 `rm -rf target`。
- **审查者不编译**：reviewer 读 worker `/tmp` 日志与源码 diff；同一门禁命令 worker/reviewer/主代理不得各跑一遍。确定性检查先于模型审查；每个门禁只调用一个审查者。

### 7.4 测试通道与凭证

通道登记单点为 `crates/providers/src/channels/registry.rs`（`CHANNEL_REGISTRY`）；endpoint 只来自配置或经核对的 adapter 默认值，`base_url` 始终可覆盖，不得把 endpoint/模型名/认证方式写进 Engine 分支。

| 通道 | 凭证 | 默认协议 / endpoint | 说明 |
| --- | --- | --- | --- |
| ChatGPT | OAuth bearer | Responses；`https://chatgpt.com/backend-api/codex` | 需 account id；非公开稳定第三方合约 |
| xAI Grok | OAuth bearer | 按模型选 Responses/Chat；`https://api.x.ai/v1` | 不接受 xAI API key |
| Z.AI GLM Coding Plan | API key | Chat；`https://api.z.ai/api/coding/paas/v4` | `provider_id` 为 `glm-coding`；中国区端点由配置显式指定 |
| OpenCode Go | API key | Chat；`https://opencode.ai/zen/go/v1` | Bearer；混合协议模型须在 registry 显式声明 transport |
| Qwen Token Plan | API key | Chat；`https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | 专属 endpoint，不与按量计费混用 |
| DeepSeek | API key | Chat；`https://api.deepseek.com` | OpenAI-compatible |

**低消耗测试矩阵**（常规冒烟、定向回归与行为对比默认只用以下组合；高级模型仅限一次性接通验证或用户明确指定的专项评估；模型名以 `pawork models` 实际返回为准；凭证缺失即 fail-closed）：

| 通道（provider_id） | 默认测试模型 | 凭证形态 |
| --- | --- | --- |
| DeepSeek（`deepseek`） | `deepseek-v4-flash` | API key |
| GLM Coding Plan（`glm-coding`） | `glm-4.7` | API key |
| OpenCode Go（`opencode-go`） | `deepseek-v4-flash` | API key |
| xAI Grok 订阅（`xai`） | `grok-4.3` | OAuth bearer |

**Key 管理红线**：正式存储 `$PAWORK_HOME/auth.json` / `~/.pawork/auth.json`（JSON v1、0600、原子写、损坏 fail-closed）；env `PAWORK_API_KEY_<PROVIDER_ID>` 仅 headless/CI fallback。key/token 不入配置文件、数据库、日志、事件流与任何可提交文件；执行期凭证由用户临场提供，缺失即终止。本地冒烟默认走 auth 文件；`.env`（已 gitignore）仅遗留 fallback。真实 API 测试两种形态：手工冒烟清单（人执行、人评估、留评估记录）与 env 门控 `#[ignore]` 自动化（`PAWORK_SMOKE_*` 变量，不进默认测试路径）。

### 7.5 收尾与状态回写

1. 定向自动化测试全绿（只跑本波写入集命令；worker 已绿的不重复编译，核对 `/tmp` 日志）。任务书有冒烟清单时执行并留评估记录。
2. 任务书回写：波次状态与退出标准打勾；核对「非目标」未越界。
3. 本文回写：§1 指针更新；阶段外任务在 §3 登记/移动；延期与新发现挂账入 §4；候选入 §5。阶段全部收口时，把该阶段的完成细节压缩迁入 [docs/history.md](docs/history.md)，本文只留开放项。
4. 文档一致性：改了冻结契约/包布局 → 同批更新 [docs/architecture.md](docs/architecture.md)；改了功能/候选状态 → [docs/design.md](docs/design.md)；写入集改了模块树、对外入口、依赖边或红线行为 → 同批更新该包 [docs/spec/crates/](docs/spec/README.md) Spec；用户可见能力/契约/安全/Desktop/验证/运维边界变化 → 同批更新对应 [docs/spec/](docs/spec/README.md) 篇目（「已实现」「已验证」「已人工验收」「已发布」分开表述）。新 ADR 落 [docs/adr/](docs/adr/)（编号续接，下一个 ADR-042）。
5. 简式任务报告：写入集、验证命令与结果、登记项；未跑全量门禁属当前路线正常状态。报告至少含：

```text
Validated: <实际命令 / tests / checks，或 none 及理由>
Targeted regressions: <实际覆盖，或 none>
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

6. 不提交、不推送，除非用户当场要求。
