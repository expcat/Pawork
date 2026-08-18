# 任务实现规范（V3 重构期公共约定）

> 本文档是所有 V3 任务（阶段波次、阶段外任务、并行子任务）**开启 / 进行 / 收尾**的公共规范。任务启动时引用本文即可，无需在提示词里重复展开这些约定——这就是「公共提示词」本体。
>
> 事实源关系：任务索引 [../ROADMAP.md](../ROADMAP.md) · 开启编排 [../v3_plan.md](../v3_plan.md) · 逐阶段任务书 [../plan/](../plan/)（R0–R9）· 设计与冻结契约 [design.md](design.md) · V2 归档总结 [v2-summary.md](v2-summary.md) · 参照项目手册 [references.md](references.md) · 工作约定基线 [../AGENTS.md](../AGENTS.md)。V2 版本文的阶段性内容（S0–S13 专属约定）已随 V2 收官移除，历史见 [v2-summary.md](v2-summary.md)。

---

## 1. 最小启动提示词

**阶段波次开发默认走 [../v3_plan.md](../v3_plan.md) 的编排**（核查 → 设计 → 派发）。窄任务（单点修复、单文件文档、一条 golden）可直接用三行最小提示词：

```text
按 docs/task-guide.md 执行 plan/R<N>-*.md 的〈波次/任务名，如「R2 波 A：本地化 L1」〉。
范围：〈可选——写入集或包边界的额外限定；不写则以任务书该波次的写入集为准〉
凭证：〈auth 文件或 .env 已就绪 / 本任务无需真实 key〉
```

约定：**任务书（plan/R\*.md）+ 本规范 = 任务完整上下文**。任务书负责「做什么、验收什么」，本规范负责「怎么开始、过程守什么、怎么收尾」；提示词只补范围、凭证与临时约束。

变体：

- **阶段外任务**：`按 docs/task-guide.md 执行〈任务书路径 + 章节〉`（例：`docs/research/multi-account-quota-plan-merge.md §4`）。
- **子代理派发**：主代理按 [../v3_plan.md](../v3_plan.md) §8 统一骨架派发（含写入集边界）。
- **候选功能转正**：先按 [../ROADMAP.md](../ROADMAP.md) §3.3 登记，再以阶段外任务启动。

---

## 2. 任务开启前（必做核对）

1. **读任务书**：对应 `plan/R<N>-*.md` 全文；确认本任务所属波次、前置波次是否完成。
2. **核对状态与依赖**：[../ROADMAP.md](../ROADMAP.md) §2——硬前置阶段须为 🟢；[../v3_plan.md](../v3_plan.md) §3 指针与工作区实态一致。
3. **ADR 闸门**：R0 波 0、R1 波 A、R6 波 0、R7 波 0 产出的 ADR（038–041）须 Accepted（用户确认）后，同阶段后续波次才可开工；主代理不代替用户拍板破坏式决议。
4. **契约核对**：[design.md](design.md) §3.2 冻结契约表 + [v2-summary.md](v2-summary.md) §4/§5 中列出本任务涉及的契约与 S13 拍板；确认 golden 测试先于实现改动（golden 先行）。
5. **证据重验**：任务书内的行数/消费者/调用点证据基于 2026-08-18 分析快照，执行时按 [../v3_plan.md](../v3_plan.md) §5.2 重验；实态与任务书冲突以实态为准并回写任务书。
6. **查参照资料**：[design.md](design.md) §4 功能 ↔ 参照项目映射；需要机制细节时进 [references.md](references.md) 与 [research/](research/)；考古已归档代码用 git 历史与 tag `v2-final`（V1 资产另见 [v1-migration-reference.md](v1-migration-reference.md)）。
7. **凭证检查**（需真实 API 的任务）：确认所需凭证已在 Pawork auth 文件或环境变量（§5）；**缺失或失效即终止任务并向用户索取，不静默跳过、不换用其他凭证、不降级为 mock 继续**（fail-closed，[research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) §1.1）。

---

## 3. 任务进行中（红线与纪律）

### 3.1 架构红线（违反须先升级为 ADR 讨论或向用户确认）

- 纯 Rust；CLI 与 Core 同进程同二进制，`pawork` 是唯一正式宿主；不引入 Node/Bun/V8/嵌入式 JS Runtime；GUI 独立进程经 GUI Connection Protocol 连接 CLI。
- canonical 纯净：`pawork-domain`（含 R1 后并入的 provider/tool 契约面）不依赖 GUI framework、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider（依赖树可断言）。
- Agent Engine 不按 Provider 名称走特例分支；能力差异一律经 registry/capability 数据表达。
- Secret（明文 Token）不落库、不入日志、不进事件 payload、不写入任何可能提交到仓库的文件；Debug/Display 输出脱敏（`[REDACTED]` 语义）。
- 所有 Agent 事件可持久化、可重放；磁盘/线上格式是冻结契约（[design.md](design.md) §3.2），R6/R7 之外只动代码组织、不动格式；R6/R7 的格式演进须 ADR Accepted + 版本化迁移 + 升级 golden。
- 禁止 crate 间循环依赖；文件工具输入一律 `workspace_id + relative_path`，拒绝绝对路径与越 root 的 `..`。
- **不合并清单**（ADR-039 固化前先行生效）：`policy`、`exec`、`auth`、`git`、`engine`、`protocol`、`testkit` 保持独立包，不得「顺手」卷入合并。

### 3.2 重构与合入纪律

- **消费面先行**：任何保留在主 workspace 的模块必须有真实装配点（生产调用链或已排期激活条件登记）；零消费者代码按 R0 决议归档，不以 experimental feature 库存。
- **归档纪律**：归档 = 移出 workspace members + 删除源目录；tag `v2-final` 与 git 历史兜底；复活条件登记 [../ROADMAP.md](../ROADMAP.md) §3.3；不把归档代码复制到仓库其它位置。
- **合并不裁剪契约**：包合并时契约类型整组平移、零裁剪，golden/测试随迁；宁可字段暂时闲置，不做「先简后改」。
- **计划内替换不是返工**：任务书标注的替换点（如 R3 registry 替换手写表、R4 服务拆分替换单体）保持外部行为不变，有契约测试护航。
- **破坏式改动的边界**：允许破坏内部代码组织与 API；不允许静默破坏磁盘/线上格式、CLI 用户可见行为与安全语义（fail-closed 只紧不松）。

### 3.3 测试纪律

- **少测试、无全量门禁**：只做能证明本任务核心行为的关键定向测试；不跑 Workspace Full Gate、不做 L0–L3 分级、不做 clippy/fmt 门禁。
- **三类关键测试例外——必须同步落地、不推迟**：安全红线定向回归；持久化与重放契约 golden；协议与解析 golden/种子。
- engine/工具循环逻辑回归全部走 MockProvider；真实 API 只承担冒烟与 env 门控 `--ignored` 测试（§5），不承载逻辑回归。
- 验证命令：`cargo check -p <crate>` / `cargo test -p <crate>`（多包重复 `-p`，不因包多改用 `--workspace`）；合并/归档波补 `cargo tree` 断言（无环、`-p pawork` 闭包不膨胀）。
- 禁止 `cargo clean`；复用默认 `target/` 增量缓存。

### 3.4 平台与输出纪律

- 在当前可用平台（macOS）实测；Linux/Windows 平台代码保持编译、交叉 `cargo check` 可选；三平台实跑不在 R0–R9 排期（R7 沙箱回归按其任务书的平台策略执行）。
- stdout 协议纪律：`--json` 模式 stdout 只承载 JSONL，文本与日志走 stderr。

---

## 4. 任务收尾（结束前必做）

1. **定向自动化测试全绿**：任务书「验证」节列出的命令逐条执行。
2. **冒烟清单执行**（任务书有列时）：真实 key 按 §5 通道；**模型评估记录留档**。
3. **任务书回写**：波次完成状态与退出标准打勾；核对「非目标」未被越界实现。
4. **ROADMAP 与指针回写**：按 [../ROADMAP.md](../ROADMAP.md) §6 状态回写约定；同步更新 [../v3_plan.md](../v3_plan.md) §3 当前指针。
5. **任务报告**（简式）：
   - 写入集：实际触碰的包/文件；
   - 验证：实际运行的命令与结果（含冒烟结论、评估记录要点）；
   - 登记项：延期 / 新发现的未决事项 / 改判的任务书证据；
   - 明确说明未运行全量门禁属当前路线的正常状态。

---

## 5. 测试通道与凭证

首发六通道产品范围沿 V2 冻结（原委见 [v2-summary.md](v2-summary.md)）；endpoint 只来自配置或经核对的 adapter 默认值，`base_url` 始终可覆盖，不能把 endpoint、模型名或认证方式写进 Agent Engine 分支。

| 通道 | 凭证 | 默认协议 / endpoint | 说明 |
| --- | --- | --- | --- |
| ChatGPT | OAuth bearer | Responses；`https://chatgpt.com/backend-api/codex` | 当前实现快照，可覆盖；需 account id；不是公开稳定的第三方 API 合约 |
| xAI Grok | OAuth bearer | 按模型选 Responses/Chat；`https://api.x.ai/v1` | 不接受 xAI API key；登录/刷新由 auth 层负责 |
| Z.AI GLM Coding Plan | API key | Chat；`https://api.z.ai/api/coding/paas/v4` | 国际站 Coding Plan 专属端点；`provider_id` 沿用 `glm-coding`；中国区开发测试端点 `https://open.bigmodel.cn/api/coding/paas/v4`（Chat）与 `https://open.bigmodel.cn/api/anthropic`（Anthropic Messages，验证 provider 契约不过拟合 OpenAI 形状），由配置显式指定 |
| OpenCode Go | API key | Chat；`https://opencode.ai/zen/go/v1` | Bearer 认证；模型目录经 `GET /models` 可查；混合协议模型必须在 registry 显式声明 transport |
| Qwen Token Plan | API key | Chat；`https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | Token Plan 专属 endpoint，不与按量计费通道混用 |
| DeepSeek | API key | Chat；`https://api.deepseek.com` | OpenAI-compatible |

> 若实际提供的 key 属于自建代理（如 opencodex、Codex Router），仅 `base_url` 不同，接入方式完全一致。

**默认测试模型（低消耗约定）**：常规冒烟、定向回归与模型评估默认只用 [../ROADMAP.md](../ROADMAP.md) §1.1 低消耗矩阵的四个组合（`deepseek`/`deepseek-v4-flash`、`glm-coding`/`glm-4.7`、`opencode-go`/`deepseek-v4-flash`、`xai`/`grok-4.3`）；ChatGPT、Qwen Token Plan 两通道与各通道高阶模型仅用于一次性接通验证或用户明确指定的专项评估。政策事实源为 ROADMAP §1.1。

**Key 管理约定（安全红线）**：

- 正式存储为 `$PAWORK_HOME/auth.json` / `~/.pawork/auth.json`（JSON v1、0600、临时文件 + rename 原子写、损坏 fail-closed）；环境变量 `PAWORK_API_KEY_<PROVIDER_ID 大写、`-`→`_`>` 为 headless/CI fallback。
- ChatGPT/xAI adapter 只消费 auth 层解析后的 `OAuthBearer`；OAuth client secret、access token、refresh token 不得写入 adapter 默认值、配置、数据库、事件流或日志。
- key 不写入配置文件、不落数据库、不进日志与事件流（`ResolvedCredential` Debug 脱敏语义）。
- 执行期凭证由用户在任务开始时临场提供；不写入任何可能提交到远程仓库的文件；缺失即终止（fail-closed）——完整约定见 [research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) §1.1。
- **本地冒烟**：四通道凭证已入 auth 文件，冒烟默认直接走 auth 文件；`.env`（已列入 `.gitignore`，禁止提交）仅作遗留 fallback，注入方式 `set -a && source .env && set +a`；产品路径仍只读 env，不把 `.env` 当配置层。
- 配置样例：`fixtures/config/config.example.toml`。

**真实测试的两种形态**：

1. **冒烟清单**：手工命令序列（人执行、人评估），验证用户可见行为，兼做模型能力评估记录。
2. **env 门控自动化**：`#[ignore]` 标注的真实 API 测试，读取 `PAWORK_SMOKE_BASE_URL` / `PAWORK_SMOKE_API_KEY` / `PAWORK_SMOKE_MODEL` / `PAWORK_SMOKE_PROTOCOL`，本地按需 `cargo test -p <pkg> -- --ignored`；不进默认测试路径。

---

## 6. 测试与验证策略（与根 AGENTS.md 的关系）

- **每波双重验收**：定向自动化（`cargo test -p <pkg>`，多包用多个 `-p`）+ 任务书标注的真实冒烟（真实 key，人评估）。
- **三类关键自动化测试**随改动同步落地（不推迟）：安全红线定向回归；持久化与重放契约 golden；协议与解析 golden/种子。
- **MockProvider 兜底**：engine/工具循环逻辑用 MockProvider 做确定性测试；真实 API 测试只验证「接得通、流解析正确、模型行为可用」。
- **当前 R0–R9 不做**：Workspace Full Gate、L0–L3 分级、clippy/fmt 门禁、schema drift CI、三平台矩阵、覆盖率。未来若需要发布，另立任务重新定义。
- **与根 [../AGENTS.md](../AGENTS.md) 的关系**：根文件 §5 已明确 V3 定向验证与 ADR 闸门；其余章节（架构红线、命名、提交与分支、安全与权限、子代理使用）全量适用。

---

## 7. 并行执行与子代理派发

- **阶段内并行**：写入集以包/目录为边界互不重叠，按任务书该波标注的并行度派发；核查（只读）与实现两阶段不叠加（见 [../v3_plan.md](../v3_plan.md) §5–§6）。
- **跨阶段并行**：满足 [../ROADMAP.md](../ROADMAP.md) §2 依赖关系且写入集不相交时可开第二条线（如 R7 ∥ R3–R6、R2 ∥ R3）；R3→R4→R5→R6 都触 `host/`，默认串行。
- **契约文件单一 owner**：domain/protocol/storage 的契约面（事件信封、帧、DDL、golden）改动不并行派发，由单一任务串行处理，避免形状漂移。
- **任务类型路由**：机械迁移/替换类（token 替换、use 路径修复、golden 随迁）适合并行子代理；装配/接线类（touch `host/`、`apps/pawork`）串行执行；涉及真实 key 的冒烟由主代理执行。
- **子代理提示词**：统一用 [../v3_plan.md](../v3_plan.md) §8 骨架（含写入集边界与禁止清单）；子代理同样受本文全部纪律约束。

---

## 8. 状态回写与任务报告

- **波次收尾**：更新 [../v3_plan.md](../v3_plan.md) §3 指针；阶段收尾更新 [../ROADMAP.md](../ROADMAP.md) §2 状态列 + 对应 `plan/R*.md` 退出标准打勾；延期项在 ROADMAP §4 登记。
- **阶段外任务**：开启/完成时更新 ROADMAP §3.2 状态；完成后移入 §3.1 并登记产出链接。
- **文档一致性**：若任务改动了冻结契约、包布局或候选功能状态，同批更新 [design.md](design.md) 对应章节（R1 收口时重写 §2 布局）；新增调研放 [research/](research/) 并在 [references.md](references.md) 登记；ADR 落 [adr/](adr/)（编号续接，现有 ADR-037）。
- **任务报告**按 §4 第 5 条的简式模板；评估记录（模型行为、协议对比、闭环成功率等）必须留档。
