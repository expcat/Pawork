# 任务实现规范（V2 开发期公共约定）

> 本文档是所有 V2 任务（阶段任务、阶段外任务、并行子任务）**开启 / 进行 / 收尾**的公共规范。任务启动时引用本文即可，无需在提示词里重复展开这些约定——这就是「公共提示词」本体。
>
> 事实源关系：任务索引 [../ROADMAP.md](../ROADMAP.md) · 逐阶段任务书 [../plan/](../plan/) · 设计与冻结契约 [design.md](design.md) · 参照项目手册 [references.md](references.md) · 迁移词典 [v1-migration-reference.md](v1-migration-reference.md) · 工作约定基线 [../AGENTS.md](../AGENTS.md)（本文 §6 的 V2 开发期放宽项优先生效）。

---

## 1. 最小启动提示词

启动一个阶段任务，提示词只需三行：

```text
按 docs/task-guide.md 执行 plan/S<N>-*.md 的〈波次/任务名，如「波 B：pawork-net」〉。
范围：〈可选——写入集或包边界的额外限定；不写则以任务书该波次的写入集为准〉
凭证：〈PAWORK_API_KEY_* 已设 / 本任务无需真实 key〉
```

约定：**任务书（plan/S\*.md）+ 本规范 = 任务完整上下文**。任务书负责「做什么、验收什么」，本规范负责「怎么开始、过程守什么、怎么收尾」；提示词只补范围、凭证与临时约束。

变体：

- **阶段外任务**：`按 docs/task-guide.md 执行〈任务书路径 + 章节〉`（例：`docs/research/multi-account-quota-plan-merge.md §4`）。
- **子代理派发**：主代理把上面模板 + 该波次的写入集边界发给子代理（见 §7）。
- **候选功能转正**：先按 [../ROADMAP.md](../ROADMAP.md) §3.3 登记，再以阶段外任务启动。

---

## 2. 任务开启前（必做核对）

1. **读任务书**：对应 `plan/S<N>.md` 全文；确认本任务所属波次、前置波次是否完成。
2. **核对状态与依赖**：[../ROADMAP.md](../ROADMAP.md) §2——依赖阶段须为 🟢；本阶段状态与实际一致。
3. **契约核对**：[design.md](design.md) §3.2 冻结契约表中列出本任务涉及的契约；确认 golden 测试先于消费实现迁移（golden 先行）。
4. **V1 资产定位**：[v1-migration-reference.md](v1-migration-reference.md) §4.1 映射总表（唯一迁移词典）+ [../plan/archive/](../plan/archive/README.md) 中实际存在的对应包级细则。M0–M8 正文当前未落仓，遇到缺失引用须报告并回退到 §4.1，不得臆造。迁移方式一律「复制 + 合并 + 改名 + 测试随迁」，V1 目录只读、git 历史不跟随。
5. **查参照资料**：[design.md](design.md) §4 本阶段的功能 ↔ 参照项目映射；需要机制细节时进 [references.md](references.md) 与 [research/](research/)。
6. **凭证检查**（需真实 API 的任务）：确认所需 key 已在环境变量或 Pawork auth 文件（§5）；**缺失或失效即终止任务并向用户索取，不静默跳过、不换用其他凭证、不降级为 mock 继续**（fail-closed，[research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) §1.1）。

---

## 3. 任务进行中（红线与纪律）

### 3.1 架构红线（违反须先升级为 ADR 讨论或向用户确认）

- 纯 Rust；CLI 与 Core 同进程同二进制，`pawork` 是唯一正式宿主；不引入 Node/Bun/V8/嵌入式 JS Runtime；GUI 独立进程经 GUI Connection Protocol 连接 CLI。
- canonical 纯净：`pawork-domain` / `pawork-api` 不依赖 GUI framework、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider（依赖树可断言）。
- Agent Engine 不按 Provider 名称走特例分支；能力差异一律经 registry/capability 数据表达。
- Secret（明文 Token）不落库、不入日志、不进事件 payload、不写入任何可能提交到仓库的文件；Debug/Display 输出脱敏（`[REDACTED]` 语义自 S0 生效）。
- 所有 Agent 事件可持久化、可重放；磁盘/线上格式是冻结契约（[design.md](design.md) §3.2），只动代码组织、不动格式。
- 禁止 crate 间循环依赖；文件工具输入一律 `workspace_id + relative_path`，拒绝绝对路径与越 root 的 `..`。

### 3.2 迁移与合入纪律

- **激活即 V1 完整形状**：契约类型整包/整组迁移、零裁剪，宁可字段暂时闲置，不做「先简后改」。
- **无消费者不合入**：合入的包/能力同批接到 `pawork` 装配链有真实调用点；接不上的以 `experimental` feature 门控 + [../ROADMAP.md](../ROADMAP.md) §4 登记激活条件，严禁静默库存。
- **冻结候审资产不迁**（quota 远端适配器 / browser-computer-runtime / tool_search，[v1-migration-reference.md](v1-migration-reference.md) §4.4）。
- **计划内替换不是返工**：任务书标注的替换点（如 S3 用 policy 替换 S2 的临时路径校验）保持外部签名不变。
- 已知死代码不原样搬运：V1 评审标记的 deferred-consumer API 迁移时逐项决定「接线或删除」。

### 3.3 测试纪律（开发期）

- **少测试、无门禁**：只做能证明本任务核心行为的关键定向测试；不跑 Workspace Full Gate、不做 L0–L3 分级、不做 clippy/fmt 门禁（全部集中在 S12；[v1-migration-reference.md](v1-migration-reference.md) §6、[research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) §1.2）。
- **三类关键测试例外——必须随资产迁移同步落地**：安全红线定向回归；持久化与重放契约 golden；协议与解析 golden/种子（清单见 [v1-migration-reference.md](v1-migration-reference.md) §6.1）。
- S2 起 engine/工具循环逻辑回归全部走 MockProvider；真实 API 只承担冒烟与 env 门控 `--ignored` 测试（§5），不承载逻辑回归。
- 验证命令：`cargo check -p <crate>` / `cargo test -p <crate>`（多包重复 `-p`，不因包多改用 `--workspace`）。

### 3.4 平台与输出纪律

- 开发期在当前平台（Windows）实测；Unix 平台代码随迁移进入、交叉 `cargo check` 可选，三平台实跑在 S12。
- stdout 协议纪律（S1 起）：`--json` 模式 stdout 只承载 JSONL，文本与日志走 stderr。

---

## 4. 任务收尾（结束前必做）

1. **定向自动化测试全绿**：任务书「定向自动化测试」节列出的命令逐条执行。
2. **冒烟清单执行**：任务书「真实测试与评估」节逐项跑（真实 key，按 §5 通道）；**模型评估记录留档**（勾选项旁注记或写入任务报告——这是 S12《真实通道模型评估报告》的原始素材）。
3. **任务书回写**：冒烟清单与退出标准打勾；核对「为后续阶段预留 / 明确不做」未被越界实现。
4. **ROADMAP 回写**：按 [../ROADMAP.md](../ROADMAP.md) §6 状态回写约定（阶段收尾更新 §2 状态列；experimental/延期项登记 §4；阶段外任务更新 §3）。
5. **任务报告**（简式，开发期不使用 L0–L3 分级模板）：
   - 写入集：实际触碰的包/文件；
   - 验证：实际运行的命令与结果（含冒烟结论、评估记录要点）；
   - 登记项：experimental / 延期 / 新发现的未决事项；
   - 明确说明未运行全量门禁属正常状态（S12 统一收口）。

---

## 5. 测试通道与凭证

S0–S5 的前期功能验证使用用户提供的两把 key；端点只来自配置。S6 起首发渠道 adapter 可以提供经过核对的默认 endpoint，但 `base_url` 始终可覆盖，不能把 endpoint、模型名或认证方式写进 Agent Engine 分支。

| 通道 | 协议 | Base URL | 说明 |
| --- | --- | --- | --- |
| GLM Coding Plan（中国区开发测试） | OpenAI Chat Completions | `https://open.bigmodel.cn/api/coding/paas/v4` | Coding Plan 专属端点（**不是**标准计费的 `/api/paas/v4`）；继续由配置显式指定，不是 S6 的 Z.AI 国际站默认值 |
| GLM Coding Plan | Anthropic Messages | `https://open.bigmodel.cn/api/anthropic` | S2 起作为第二协议通道，验证 provider 契约不过拟合 OpenAI 形状 |
| OpenCode Go | OpenAI Chat Completions | `https://opencode.ai/zen/go/v1` | Bearer 认证；模型目录经 `GET /models` 可查（`deepseek-v4-pro`、`kimi-k2.x`、`glm-5.x` 等）；少数模型仅走 Anthropic `/messages` |

> 若实际提供的 key 属于自建代理（如 opencodex 网关），仅 `base_url` 不同，接入方式完全一致。

S6 首发产品范围冻结如下；这里的“已预设”不等于“已完成登录或真实冒烟”，阶段状态以 [S6 任务书](../plan/S6-providers-auth.md) 为准。

| 通道 | 凭证 | 默认协议 / endpoint | 说明 |
| --- | --- | --- | --- |
| ChatGPT | OAuth bearer | Responses；`https://chatgpt.com/backend-api/codex` | 当前实现快照，可覆盖；需 account id；不是公开稳定的第三方 API 合约 |
| xAI Grok | OAuth bearer | 按模型选 Responses/Chat；`https://api.x.ai/v1` | 本期不接受 xAI API key；登录/刷新由 auth 层负责 |
| Z.AI GLM Coding Plan | API key | Chat；`https://api.z.ai/api/coding/paas/v4` | 国际站 Coding Plan 专属端点；`provider_id` 沿用 `glm-coding` |
| OpenCode Go | API key | Chat；`https://opencode.ai/zen/go/v1` | 混合协议模型必须在 registry 显式声明 transport |
| Qwen Token Plan | API key | Chat；`https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | Token Plan 专属 endpoint，不与按量计费通道混用 |
| DeepSeek | API key | Chat；`https://api.deepseek.com` | OpenAI-compatible |

除上表六条外的 Provider/认证方式均延期到后续需求。S0 generic OpenAI-compatible 与 S2 Anthropic 基线仍为既有能力，不因此删除或算作首发新增。

**默认测试模型（低消耗约定，2026-08-17 起）**：常规冒烟、定向回归与模型评估默认只用 [../ROADMAP.md](../ROADMAP.md) §1.1 低消耗矩阵的四个组合（`deepseek`/`deepseek-v4-flash`、`glm-coding`/`glm-4.7`、`opencode-go`/`deepseek-v4-flash`、`xai`/`grok-4.3`）；ChatGPT、Qwen Token Plan 两通道与各通道高阶模型仅用于任务书要求的一次性接通验证，或用户明确指定的高级功能专项评估。政策事实源为 ROADMAP §1.1，规则全文不在此重复。

**Key 管理约定（安全红线自 S0 生效）**：

- S0–S5 的 API key 只经环境变量注入：`PAWORK_API_KEY_<PROVIDER_ID 大写、`-`→`_`>`（如 `PAWORK_API_KEY_GLM_CODING`）。S6 起以 `$PAWORK_HOME/auth.json` / `~/.pawork/auth.json`（JSON v1、0600、临时文件 + rename 原子写、损坏 fail-closed）为正式存储，环境变量降级为 headless/CI fallback。
- ChatGPT/xAI adapter 只消费 auth 层解析后的 `OAuthBearer`；OAuth client secret、access token、refresh token 不得写入 adapter 默认值、配置、数据库、事件流或日志。
- key 不写入配置文件、不落数据库、不进日志与事件流（V1 `ResolvedCredential` 的 Debug 脱敏语义自 S0 采用）。
- 执行期凭证由用户在任务开始时临场提供（env 或 `pawork auth` 写入仓库外的 Pawork auth 文件）；不写入任何可能提交到远程仓库的文件；缺失即终止（fail-closed）——完整约定见 [research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) §1.1。
- **V2 开发期本地冒烟**：两通道 key 放在 `.env`（已列入 `.gitignore`，禁止提交）。冒烟进程用 `set -a && source .env && set +a` 注入环境变量；产品路径仍只读 env，不把 `.env` 当配置层。S6 起 `~/.pawork/auth.json` 已为正式存储且四通道凭证已入库，冒烟默认直接走 auth 文件；`.env` 仅作 S0–S5 遗留 fallback。
- 配置样例随 S0 产出：`fixtures/config/config.example.toml`（含上表三个 provider 条目）。

**真实测试的两种形态**（每个阶段计划文档都含这两节）：

1. **冒烟清单**：手工命令序列（人执行、人评估），验证用户可见行为，兼做两个模型的能力评估记录。
2. **env 门控自动化**：`#[ignore]` 标注的真实 API 测试，读取 `PAWORK_SMOKE_BASE_URL` / `PAWORK_SMOKE_API_KEY` / `PAWORK_SMOKE_MODEL` / `PAWORK_SMOKE_PROTOCOL`，本地按需 `cargo test -p <pkg> -- --ignored`；不进默认测试路径，避免 CI 依赖外部服务。

---

## 6. 测试与验证策略（与根 AGENTS.md 的关系）

- **每阶段双重验收**：冒烟清单（真实 key，人评估）+ 定向自动化（`cargo test -p <pkg>`，多包用多个 `-p`）。阶段计划文档中逐条列出。
- **三类关键自动化测试**随契约/资产迁移同步落地（不推迟）：安全红线定向回归；持久化与重放契约 golden；协议与解析 golden/种子。清单见 [v1-migration-reference.md](v1-migration-reference.md) §6.1。
- **MockProvider 兜底**：S2 起所有 engine/工具循环逻辑用 MockProvider 做确定性测试；真实 API 测试只验证「接得通、流解析正确、模型行为可用」，不承载逻辑回归。
- **开发期不做**（沿用 [v1-migration-reference.md](v1-migration-reference.md) §6.2）：Workspace Full Gate、L0–L3 分级、clippy/fmt 门禁、schema drift CI、三平台矩阵、覆盖率。全部集中到 S12。
- **平台策略**：开发期在当前平台（Windows）实测；Unix 平台代码随迁移进入但只做交叉 `cargo check`（可选），三平台实跑在 S12。
- **与根 [../AGENTS.md](../AGENTS.md) 的关系**：根文件（2026-08-17 重建的 V2 版）§5 已明确开发期验证以本节为准（S12 前不做 L0–L3 分级门禁与 Workspace Full Gate，S12 Release Hardening 时恢复全量门禁）；其余章节（架构红线、命名、提交与分支、安全与权限、子代理使用）全量适用。

---

## 7. 并行执行与子代理派发

- **阶段内并行**：写入集以包/目录为边界互不重叠，可并行派发子代理（每包一路）；各阶段任务书给出分波建议。
- **跨阶段并行**：满足 [../ROADMAP.md](../ROADMAP.md) §2 依赖关系的前提下，S5/S6 可并行，S8（git）可与它们并行；S7 GUI 设计波不依赖 S6，实现波建议 S1–S5 已绿。主干阶段（S0–S4）建议串行。
- **契约文件单一 owner**：`foundation/domain`、`foundation/api` 的改动不并行派发，由单一任务串行处理，避免 serde 形状漂移。
- **任务类型路由**：V1 资产迁移类任务（「复制 + 合并 + 改名 + 测试随迁」）边界清晰，适合并行子代理；新装配/接线类任务（touch `host/app`、`apps/pawork`）串行执行；涉及真实 key 的冒烟建议主代理执行。
- **子代理提示词**：使用 §1 模板 + 明确的波次任务名 + 写入集边界（允许触碰的目录/包清单）+「完成后报告写入集与验证结果」；子代理同样受本文全部纪律约束。

---

## 8. 状态回写与任务报告

- **阶段任务收尾**：更新 [../ROADMAP.md](../ROADMAP.md) §2 总览表状态列 + 对应 `plan/S*.md` 冒烟清单与退出标准打勾 + 如有 experimental/延期项在 ROADMAP §4 登记。开发期不做逐任务文档同步（[v1-migration-reference.md](v1-migration-reference.md) §2.4）。
- **阶段外任务**：开启/完成时更新 ROADMAP §3.2 状态；完成后移入 §3.1 并登记产出链接。
- **文档一致性**：若任务改动了冻结契约、包布局或候选功能状态，同批更新 [design.md](design.md) 对应章节；新增参照项目调研放 [research/](research/) 并在 [references.md](references.md) 登记。
- **任务报告**按 §4 第 5 条的简式模板；评估记录（模型行为、协议对比、闭环成功率、缓存命中率等）必须留档，S12 汇总为《真实通道模型评估报告》。
