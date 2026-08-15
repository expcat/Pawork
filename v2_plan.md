# Pawork V2 任务开启编排

> 本文是 V2 开发的**指定开启文件**。每次开新对话，提示词指向本文即可；主代理按本文编排**一个波次**（研究 → 设计 → 实现 → 收尾）。
>
> 本文只负责「选哪一波、怎么搜、怎么设计、怎么派子代理」。过程纪律（架构红线、迁移、测试、凭证、收尾清单）以 [Pawork_v2/docs/task-guide.md](Pawork_v2/docs/task-guide.md) 为准，不在此重复展开。

---

## 1. 文档地图

| 文档 | 读它做什么 |
| --- | --- |
| 本文 `v2_plan.md` | 开启编排、当前指针、统一提示词、子代理模型约定 |
| [Pawork_v2/ROADMAP.md](Pawork_v2/ROADMAP.md) | 阶段总索引、依赖、状态、阶段外任务 |
| [Pawork_v2/docs/design.md](Pawork_v2/docs/design.md) | 包激活映射、冻结契约、本阶段功能与参照项目映射 |
| [Pawork_v2/docs/task-guide.md](Pawork_v2/docs/task-guide.md) | 开启核对、红线、测试通道、并行纪律、收尾与报告 |
| [Pawork_v2/plan/](Pawork_v2/plan/) | 本阶段任务书：目标、包与 V1 资产、冒烟、退出标准、**并行拆分（波次）** |
| [Pawork_v2/docs/references.md](Pawork_v2/docs/references.md) | 参照项目手册（公开文档入口） |
| [Pawork_v2/docs/v1-migration-reference.md](Pawork_v2/docs/v1-migration-reference.md) | V1→V2 唯一迁移词典 |
| [Pawork_v2/plan/archive/](Pawork_v2/plan/archive/README.md) | 归档的包级迁移细则 |
| [AGENTS.md](AGENTS.md) | 仓库级红线；V2 开发期验证放宽见 task-guide §6 |

工作区根仍是本仓库；V2 代码落在独立 workspace `Pawork_v2/`。V1 目录只读，不在 V1 上继续加功能。

---

## 2. 开启提示词（用户侧）

**子代理模型必填。** 未写模型时，主代理只完成「读指针 / 提议下一波」，然后提问并停止，不启动研究或实现。

```text
按 v2_plan.md 开始。
子代理模型：〈必填：当前宿主可接受的模型标识，见 §7〉
范围覆盖：〈可选。例：S0 波 B。不写则按 §4 自动选下一波〉
凭证：〈可选。PAWORK_API_KEY_* 已设 / 本波无需真实 key；本地冒烟默认 source Pawork_v2/.env〉
临时约束：〈可选。例：跳过真实冒烟、只设计不实现——默认不要用〉
```

同一条消息里的「范围覆盖」优先于自动选择。不要在提示词里粘贴 task-guide 全文。

---

## 3. 当前指针（每波收尾由主代理更新）

| 字段 | 值 |
| --- | --- |
| 当前阶段 | S6（[plan/S6-providers-auth.md](Pawork_v2/plan/S6-providers-auth.md)） |
| 阶段状态 | 🔵 进行中 |
| 已完成波次 | S0 波 A–D（含 2026-08-14 两通道真实冒烟）；S1 波 A–C（含 2026-08-14 两通道真实冒烟：`sessions` / `--resume` / `run --json` / `kill -9` 恢复）；S2 波 A–C；S2 波 D（2026-08-14：cli `⚙` 工具行 + `host/app` 装配 `run_session`/scheduler/四只读工具；三通道冒烟 GLM OpenAI / GLM Anthropic / OpenCode Go）；S3 波 A（2026-08-15：`pawork-policy` 整包 + `pawork-tools` 写三件 + scheduler `check_gate`）；S3 波 B（2026-08-15：engine `ApprovalGate` + app 写三件/审批宿主/resume 封口 + cli `--approval-mode`/`y`/`a`/`n`/`--json` fail-closed）；S3 波 C（2026-08-15：两通道真实冒烟 + 提示注入评估；S3 收口）；S4 波 A（2026-08-15：`pawork-exec` process+sandbox；`run_command` + policy 认 `argv`；fail-closed 守 ADR-031 可观测回退）；S4 波 B（2026-08-15：engine `CancelHandle` + 工具中取消不发 `ToolExecutionCompleted`；cli 命令流式渲染/stderr 着色/Ctrl-C 走 `cancel(User)`；app 注册第八件 `run_command`）；S4 波 C（2026-08-15：两通道「读-改-跑」闭环 + Ctrl-C `RunCancelled` + `git push --force` Dangerous 拒绝；S4 收口）；S5 波 A（2026-08-15：`pawork-provider-core` usage/registry/pricing/negotiate/reasoning；`pawork-session` compaction feature + `TokenEstimator` 注入）；S5 波 B（2026-08-15：engine `context` 四模块迁入并接 `run_session`（ContextPrepared 估算/软限压缩/硬限截断）；app registry 装配 + session usage/cost + 手动压缩；cli `/compact` + 每轮用量行 + `models` 目录（window/定价）；修 trigger 消息 id 同毫秒撞主键与压缩折叠水位误删保留尾部）；S5 波 C（2026-08-15：两通道真实冒烟——长对话软限压缩/`--resume`/`/compact`/token 对账 1:1/`pawork models` + 评估记录；修复 `last_run_usage` 冻结最早轮；S5 收口） |
| S6 波次进度 | 波 A（2026-08-15）：六条首发通道 adapter、共享 Responses、credential fail-closed、wiremock 契约完成；未做真实凭证冒烟。波 B（2026-08-15）：`pawork-auth` 整包迁移 + Keychain→env→无凭证解析链（41 测试）；`pawork-diagnostics` 全局脱敏 layer + `RedactingFmtLayer`（metrics/bundle 门控 `experimental`）；workspace manifest 修复 glob 命中占位目录的加载错误 |
| S6 波次进度 | 波 A（2026-08-15）：六条首发通道 adapter、共享 Responses、credential fail-closed、wiremock 契约完成；未做真实凭证冒烟。波 B（2026-08-15）：`pawork-auth` 整包迁移 + Keychain→env→无凭证解析链（41 测试）；`pawork-diagnostics` 全局脱敏 layer + `RedactingFmtLayer`（metrics/bundle 门控 `experimental`）；workspace manifest 修复 glob 命中占位目录的加载错误。波 C（2026-08-15）：六通道正式装配 + `pawork models` 聚合 + `pawork auth` 四子命令 + REPL `/model` `/provider` 切换事件 + 宿主全局脱敏挂载；glm-coding/opencode-go 完成 set-key→清 env→Keychain 流式工具任务，GLM 双协议通道切换实测，trace 日志 0 泄漏；修复 keyring 平台后端缺失（原默认 mock 存储）；ChatGPT/xAI OAuth 与 Qwen/DeepSeek 凭证按 fail-closed 登记 |
| **下一波次** | **S6 收口**（真实 OAuth 冒烟与剩余 API-key 通道：需用户提供 ChatGPT 浏览器登录、xAI `[oauth.xai]` 端点或 Qwen/DeepSeek key；另需在 Keychain Access 清理冒烟遗留的 `pawork.glm-coding` / `pawork.opencode-go` 条目） |
| 阻塞 | 无。真实凭证冒烟留到波 C；本地 API-key 冒烟凭证仍从 gitignored 的 `Pawork_v2/.env` 注入 |

自动选择以本表为准，再用 ROADMAP / 任务书 / 工作区实态交叉校验（§4）。三者冲突时：**工作区实态 > 本表 > ROADMAP 状态列**；更新本表使三者一致后再开工。

---

## 4. 选任务规则

一次开启只做 **一个波次**（任务书「并行拆分建议」里的 A/B/C/…）。做完即收尾，不自动跨入下一波。

1. 读 ROADMAP §2。依赖阶段必须为 🟢；若当前阶段为 ⚠️，停止并报告阻塞。
2. 取第一个非 🟢 的主干阶段（S0→S12）。阶段外任务（ROADMAP §3.2）仅当用户在「范围覆盖」里点名时才做。
3. 读该阶段任务书的「并行拆分建议」，结合 §3 指针与工作区（`Pawork_v2/**/Cargo.toml`、成员 crate 是否已激活、任务书勾选）选出**最早未落地的波次**。
4. 用户覆盖（「做 S2 波 B」「先做阶段外：多账户并入 plan」）立即生效。
5. 在聊天里用三行声明后立刻进入 §5（不必等确认）：
   - 本次：`S<N> 波 <X>` + 任务书中该波一句话；
   - 子代理模型：用户指定值；
   - 写入集：该波允许触碰的包/目录。

主干 S0–S4 按阶段串行。跨阶段并行（S5/S6/S7 等）只在 ROADMAP §2 依赖已满足、且用户明确要求时才开第二条线。

---

## 5. 主代理执行流程

未指定子代理模型 → **停在 §4 第 5 步之前**，向用户要 §2 模板中的那一行。

### 5.1 开启核对（主代理亲自读，不派发）

按 [task-guide.md](Pawork_v2/docs/task-guide.md) §2：任务书全文、ROADMAP 依赖、[design.md](Pawork_v2/docs/design.md) §3.2 本波相关冻结契约、§4 本阶段功能表。本波若需要真实 API，缺 key 则 fail-closed（task-guide §5），不改用 mock 顶替冒烟。

### 5.2 并行研究（只读，2–3 路同时派发）

在写设计、改代码之前，用 **§8.1 同一骨架** 并行派出研究子代理（全部使用 §7 指定的模型）。默认三路：

| 路 | 搜什么 | 目的 |
| --- | --- | --- |
| R1 V1 资产 | 本仓库对应 crate / 测试 / `docs/features/` | 定位要迁的代码、golden、已知死代码与 deferred API |
| R2 参照项目 | design.md §4 本阶段已映射项 + references.md 链接；只深挖本波功能，不扫整仓 | 行为对标与取舍（红线排除项只记「不采纳」） |
| R3 迁移词典 | v1-migration-reference.md 映射总表 + plan/archive 中本波相关包级细则 | 合并来源、行数、关键动作、冻结候审清单 |

约束：

- 只读。禁止改文件、禁止开始实现。
- 不克隆参照项目到本仓库；公开文档用已有 research / 链接抓取。
- 回传要带路径 + 行号（或稳定符号名），不要空泛摘要。
- 本波任务书已写明「直接迁移 / 新写」的，R1+R3 仍要跑（核对事实源）；R2 若 §4 本阶段没有对应行可省略。

### 5.3 本波实现设计（主代理写，不改冻结契约）

研究齐了之后，主代理在**本会话**写出「本波实现设计」（结构化消息，默认不新建 markdown）。子代理不得自行改设计。需要 ADR 或与任务书/冻结契约冲突时，**先问用户再实现**。

设计至少包含：

1. **目标 / 非目标**：对应该波与任务书「为后续阶段预留 / 明确不做」。
2. **V1 事实源**：复制 / 合并 / 改名 / 测试随迁的路径；明确不搬的死代码。
3. **参照取舍**：采纳的行为 vs 红线排除（无 TUI、无 JS runtime 等）。
4. **包与模块**：激活还是增强；关键类型 / trait / 文件；依赖方向。
5. **冻结契约**：本波涉及的表项；字段宁可闲置，禁止「先简后改」。
6. **写入集**：允许触碰的目录/包；契约文件（`foundation/domain`、`foundation/api`）单一 owner。
7. **验证**：本波 `cargo test -p …` / `cargo check -p …`；是否需要真实 key。
8. **派发图**：本波若标「并行 ×N」，列出每路写入集；若标「串行 / 单一 owner」，主代理自做或只派 **一个** 实现子代理。

设计默认留在会话里。仅当发现任务书或 design.md 的缺口/错误时，由主代理改现有文档，不另开设计文件。

### 5.4 按波次实现

- **研究已并行结束**，再进入实现；不要边搜边写。
- 实现并行度 **严格按该波标注**：并行波一次派齐互不重叠的实现子代理；串行波不拆。
- 契约文件不并行。装配收口（`host/app`、`apps/pawork`）与真实 key 冒烟由主代理做。
- 每个实现子代理使用 **§8.1 同一骨架**（角色=实现）+ 主代理设计中属于它的切片 + 写入集边界。
- 子代理写完后主代理做：冲突检查、定向测试、必要接线。子代理之间禁止改同一文件。

### 5.5 本波收尾（主代理）

1. 跑本波写入集对应的 `cargo check/test -p <crate>`（多个 `-p`，不用 `--workspace`）。开发期无 clippy/fmt/Full Gate（task-guide §6）。
2. 更新本文 §3 指针：已完成波次、下一波次；本阶段仍有剩余波次则 ROADMAP 标 🔵。
3. 最后一波才跑任务书冒烟清单、勾退出标准、ROADMAP 标 🟢（需真实 key 则 fail-closed）。
4. 简式报告（task-guide §4 第 5 条）：写入集、验证、登记项、说明未跑全量门禁为正常。
5. 不提交、不推送，除非用户当场要求。

---

## 6. 并行与子代理纪律

- 文档、指针、设计、ROADMAP/任务书勾选：**主代理写**。
- 研究可并行；实现按波次并行。两阶段不要叠成「有的包还在搜、有的包已经在写」。
- 写入集以包/目录为界，互不重叠。
- 一次开启只派 **本波** 的实现子代理，不预派下一波。
- 并发保持小：研究最多 3 路；实现按任务书该波的并行度（通常 1–3）。不要为加速拆碎契约波。
- 子代理同样受 task-guide 全文约束；提示词里写明「禁止越写入集、禁止改冻结契约形状、禁止 git commit」。

---

## 7. 子代理模型

开启提示词里的「子代理模型」作用于**所有** `Task` 子代理（研究 + 实现）。主代理用当前对话模型，不要擅自换成别的。

本文**不映射具体模型**：不维护型号清单，也不做「用户写法 → `Task` 参数」的翻译表。用户写的模型标识由主代理**原样**落入 `Task`（落在哪个参数、取什么值以当前宿主为准），不猜测、不替换、不查表。

写法：

- 直接写当前宿主 `Task` 可接受的模型标识（如宿主文档/可选列表中的 model slug、subagent 类型等），一行一个值；研究与实现共用同一值。
- 想与主代理同模型：写 `inherit`（或宿主的等价写法）。

规则：

- 用户写的模型标识当前宿主无法识别 → 提问，不猜测、不替换。
- 禁止对研究用快模型、对实现用另一个模型，除非用户在「临时约束」里写明。
- 用户指定的模型不在当前宿主能力范围内时：告诉用户不可用项与可用项，等回复。
- 具体型号与 `Task` 参数映射随宿主（Cursor / opencodex 路由等）变化，以宿主当前可选值为准；本文件不维护、不钉死。

---

## 8. 统一提示词

所有子代理用同一骨架，只替换「角色 / 范围 / 产出 / 禁止」四段。主代理把用户指定的模型按 §7 传入 `Task`，**不要把模型名写进 prompt 指望子代理自己切换**。

### 8.1 骨架

```text
你是 Pawork V2 的〈角色：研究 | 实现〉子代理。只做本提示词里的范围。

规范（纪律全文，必须遵守）：
- Pawork_v2/docs/task-guide.md
- 仓库根 AGENTS.md（V2 开发期验证放宽以 task-guide §6 为准）

任务：
- 阶段任务书：Pawork_v2/plan/S<N>-*.md
- 波次：〈波 X：一句话〉
- 设计切片：〈实现角色必填——粘贴主代理本波设计中属于本路的部分；研究角色写「无，先于设计」〉

范围：
- 〈研究：只读路径/关键词；实现：允许写入的包/目录清单〉

产出（完成后一次性报告）：
- 〈见下方角色特化〉

禁止：
- 超出范围的文件改动或无关重构
- 改变冻结契约的 serde/磁盘/线上形状（字段可闲置，不可删减「图省事」）
- git commit / push / 改 git config
- 把 Secret 写入仓库或日志
- 运行 cargo --workspace / clippy 门禁 / cargo clean
- 研究角色：任何写入；实现角色：开始前改设计、碰契约包（除非写入集明确包含）
```

### 8.2 角色特化 — 研究 R1（V1 资产）

在骨架「产出」处填：

```text
- 本波相关 V1 crate 路径、关键类型/函数（文件+行号或符号名）
- 必须随迁的测试 / golden / 种子
- 评审已标的死代码、deferred-consumer API（建议接线或删除）
- 不要搬的内容（冻结候审、明显死代码）
- 与任务书「V1 来源与方式」不一致之处（事实源优先）
```

范围示例：`crates/agent-domain`、`crates/provider-api`、对应 `docs/features/`、crate 内 `tests/`。

### 8.3 角色特化 — 研究 R2（参照项目）

```text
- 仅针对 design.md §4 本阶段、且属于本波的功能行
- 每个功能：参照项目中的对应行为、文档 URL、与 Pawork 红线冲突的部分
- 建议采纳的行为要点（短）；明确不采纳的（TUI / JS 插件 / 身份伪装等）
- 不要写实现代码
```

### 8.4 角色特化 — 研究 R3（迁移词典）

```text
- v1-migration-reference.md 映射表中本波各包的来源、行数、关键动作
- plan/archive 里可直接引用的包级细则段落链接
- 本波激活 vs 后续增强的边界
- 冻结候审资产是否被任务书误列入
```

### 8.5 角色特化 — 实现

```text
- 按设计切片在写入集内完成迁移或新写
- 关键测试随迁并在本包 `cargo test -p <crate>`（或 check）通过
- 报告：实际写入文件、验证命令与结果、未做项、发现的计划偏差
- 接不上装配链的能力不要静默合入（experimental + 登记，见 task-guide）
```

主代理派发实现时，把 §5.3 设计中该路的第 4–7 条原样贴进「设计切片」，避免子代理重新设计。

---

## 9. 与 task-guide 的分工

| | `v2_plan.md`（本文） | `task-guide.md` |
| --- | --- | --- |
| 何时读 | 每次开聊最先读 | 核对、进行中、收尾时遵守 |
| 选哪一波 | §3–§4 | 不负责 |
| 研究 → 设计 → 派发 | §5–§8 | §7 只给并行原则 |
| 红线 / 迁移 / 测试 / key / 报告格式 | 引用 | 事实源 |
| 最小启动提示词 | §2（含必填模型） | §1 仍可用于「已选波次、不走编排」的窄任务 |

窄任务（例如「只修 pawork-net 一条 golden」）可以继续用 task-guide §1，不必走本文三段编排。**阶段波次开发默认走本文。**

---

## 10. 主代理自检清单（派发前）

- [ ] 子代理模型已由用户指定，且按 §7 能落到 `Task` 参数
- [ ] 本次恰好一个波次，写入集已写清
- [ ] 依赖阶段为 🟢；本波契约已对照 design.md §3.2
- [ ] 研究三路（或可省略的 R2）已回传后再写设计
- [ ] 设计未改冻结契约形状；冲突已升级而不是自行拍板
- [ ] 实现并行度与任务书该波一致；契约/装配未被拆并行
- [ ] 收尾会更新本文 §3，且不会顺手开下一波
