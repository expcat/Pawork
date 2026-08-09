# Phase 8 Review：Skills、Prompts 与 Instructions（resource-loader）

> 审查范围：`crates/resource-loader`（P8-1～P8-8）与 `crates/context-engine` 的 Resource 接线。
> 方法：Commander 统筹 + 4 个 `deepseek_explorer` 并行调查（核心结构 / skills·templates / watch·io·diagnostics·profiles·agents / 集成与消费者），结论由 Commander 复核合并。
> 性质：**只 Review，不改实现。**

---

## 0. 一句话结论

Phase 8 的 crate 内部实现是**干净的、确定性的、有测试的**，但它在当前仓库里**没有任何端到端消费者**——`resource-loader` 只被 `context-engine` 依赖，而 `context-engine` 的 Resource 入口又没有任何 Host/Run 代码调用。所有 8 个子任务标记的 🟢 准确描述的是"crate 内部完成"，**不是"已接入真实运行"**。这与 plan 文档显式声明的边界（Host Run 编排属 P13）一致，因此不算未完成；但它意味着 P8 的复杂度尚未被任何真实路径验证，而其中相当一部分复杂度（semver 固定点解析、双向冲突、诊断视图、热重载）在当前 declare-only 加载路径下**不可观测**。

核心建议方向：**减少**——删死抽象、合并重复校验、把"为未来保留"的字段推迟到真正需要它们的 Phase，而不是在本阶段提前固化。

---

## 1. 设计符合度

| 子任务 | plan 目标 | 实现位置 | 符合度 | 备注 |
|---|---|---|---|---|
| P8-1 Resource Loader | 加载错误不崩溃 | `loader.rs` 全流水线 + `ResourceIssue` 隔离 | ✅ 符合 | 单文件缺失/超限/非 UTF-8/格式错均被隔离，无崩溃路径 |
| P8-2 AGENTS.md 层级 | 根+路径层级确定性聚合 | `agents.rs:113-117`（`(depth, relative_path)` 排序） | ✅ 符合 | 写序无关，反向输入测试覆盖 |
| P8-3 Skills | manifest/激活/冲突/热重载 | `skills.rs` 全量 | ⚠️ 符合但过度 | 冲突/依赖解析规模超出 declare-only 需求（见 §3） |
| P8-4 Prompt Templates | 参数/默认配置/覆盖 | `templates.rs` | ⚠️ 符合但有死 API | `PromptTemplate::render` 零调用；`PromptDefaults` 解析后从不应用 |
| P8-5 Profiles v1 | 单次运行 instructions | `profiles.rs` | ✅ 符合 | v1 边界诚实（`deny_unknown_fields`），但 `default_provider/model` 全仓库无消费者 |
| P8-6 配置优先级 | 确定性合并 | 复用 `ConfigTier` + context-engine 映射 | ✅ 符合 | 真实复用 `config-service`，无重复实现；但有双优先级表漂移风险（见 §4） |
| P8-7 Resource Diagnostics | 显示生效来源 | `diagnostics.rs` | ⚠️ 产出无消费者 | 诊断被真实产出，但 `diagnostic_view()` / `ResourceDiagnosticView` 全仓库零调用 |
| P8-8 Hot Reload | 变更去抖重载 | `watch.rs` | ⚠️ 实现无消费者 | 生产级实现（drop 停监、初始窗口补 reload、锁外重建），但 `ResourceLoader::watch` 仅本 crate 测试调用 |

**判定**：plan 目标在 crate 内全部达成，🟢 标记与实现一致。但 P8-3/4/7/8 四项存在"实现完整但当前无任何消费者能观测其行为"的情况——这不违反 plan 边界，却在"是否过度"维度上值得记录。

---

## 2. 最重要发现：零端到端消费者

这是本次 Review 的**第一结论**，由 4 个独立调查路径一致确认：

- `resource-loader` 在整个 workspace 中**只被 `context-engine` 依赖**（`context-engine/Cargo.toml:12`）。`app-service`、`cli-host`、`agent-engine` 的 `Cargo.toml` 均不依赖它。
- `context-engine` 暴露的 Resource 入口 `ContextBuilder::resource_bundle` / `resource_instructions`（`builder.rs:78-91`）**没有任何 crate 调用**——甚至不在测试里。
- `ResourceLoader::watch`（P8-8 热重载入口）**仅被 `resource-loader` 自身的测试调用**（`loader.rs:691, 741`）。
- `diagnostic_view()`（P8-7 诊断视图入口）**全仓库零调用**。

也就是说：从 `agent-domain` → `provider-runtime` → `agent-engine` → `app-service` → `cli-host` 的真实运行链里，目前**没有任何一环读取用户配置的 Skills/AGENTS.md/Profile/Prompt**。这并非缺陷——plan 明确把 Host Run 编排归到 P13，docs/features/skills.md 也写了"最终 Host Run 装配属于 Phase 13"。

**含义**：

1. P8 的全部行为正确性目前**只能由单元测试背书**，无法由集成路径背书。
2. 在这个阶段引入的高复杂度机制（见 §3）承担的是"为 P13/P17 预留语义"的角色，而非"被当前路径验证过"。
3. Review 无法评估"设计与实际是否一致"中**实际**那一半——因为目前没有实际。建议在 P13 接线时优先验证本 Review 标记的各处。

---

## 3. 冗余与过度设计（按可削减量排序）

### 3.1 Skills 依赖/冲突引擎（削减量最大）

`skills.rs` 非测试机器约 238 行用于 semver 依赖解析与冲突检测，其中约 113 行在当前路径下**语义不可观测**：

- `resolve_active`（`skills.rs:624-671`，48 行）实现"最大可共存激活子集"的**不动点收敛**：每轮剔除被冲突的技能再重算。但 P8 中 Skills 只声明不执行，没有任何下游会区分"最大子集"与"单次 BFS 的结果"——两者产出的 `Rejected` 状态与错误码完全相同。
- `bfs_supported`（`skills.rs:572-622`，51 行）携带 `allowed` 重收敛参数，仅为支撑上面的不动点。
- `collect_dep_issues`（`skills.rs:673-737`，65 行）**重新计算一遍 BFS 里已经做过的 semver 匹配**，只为生成诊断条目。

这部分共约 **113 行 + 配套测试 ~278 行**，可被"单次 BFS + 复用其匹配结果生成诊断"替代，语义不变。`detect_conflicts`（36 行，O(n²) 成对检测）是合理的，保留。

### 3.2 Skills/Templates 字段"解析后从不读取"

以下字段被完整解析、校验、携带，但**全仓库无任何代码读取**（连 `loader.rs` 都不读）：

- Skills：`manifest.parameters`（`skills.rs:405-412`）、`scripts`（414-452）、`assets`（454-461）、`permissions`（74）。
- Templates：`PromptDefaults.model/thinking/tools/budget`（`templates.rs:30-36`）、`RenderedPrompt.included_files`（51-54）。

这些是"为 P13 运行时预留"的字段。在尚无消费者时提前解析并校验，承担的是"前向兼容契约"的角色——可接受，但应明确标记为 deferred-consumer，避免被误读为"已生效"。

### 3.3 死 API 与死抽象

| 项 | 位置 | 状态 |
|---|---|---|
| `LoadResources` trait | `loader.rs:84` | 单实现、零 trait-object/generic 使用，纯死抽象 |
| `ResourceBundle::diagnostic_view` | `loader.rs:78-81` | 零调用 |
| `ResourceLoader::options` | `loader.rs:102-104` | 零调用 |
| `PromptTemplate::render` | `templates.rs:80-96` | 零调用（连测试都不调） |
| `AgentsDocument::new` / `into_documents` / `iter` / `is_empty` | `agents.rs:36,67,72,75` | 零调用 |
| `ResourceOrigin::Builtin` | `source.rs:31-32` | 从不在生产代码构造，仅 redactor match |
| `ResourceLimits.max_include_depth` | `request.rs:121-124` | 文档自述"为后续递归 include 预留"，从不读取 |

### 3.4 `ResourceBundle` 双重状态

`ResourceBundle` 同时持有结构化字段（`agents`/`skills`/`templates`/`profiles`/`resolved_instructions`）**和** `instructions`——后者是前者的扁平化（`loader.rs:203-224` 的 `append_*` 把结构化结果重新发为 `ResourceInstruction`）。而 context-engine **只消费 `instructions`**（`builder.rs:89-91`），其余五个字段**仅被 crate 内测试读取**（`loader.rs:454-466`）。

两份等价状态并存是真实的维护负担：改一边漏改另一边会产生静默不一致。应二选一——要么 bundle 只留 `instructions + diagnostics`，要么去掉 `instructions` 让消费方扁平化。

### 3.5 重复的解析/校验逻辑

四份调查独立指认的重复：

- **ASCII 标识符校验 4 份**：`skills.rs:557`、`templates.rs:532`、`templates.rs:545`、`profiles.rs:344`。
- **安全相对路径校验 2 份**：`skills.rs:534`（`validate_declared_path`）与 `templates.rs:514`（`validate_relative_reference`）几乎相同，本应归入已托管路径安全的 `io.rs`。
- **TOML 解析错误样板 3 份**：`skills.rs:355`、`templates.rs:293`、`profiles.rs:301`，一个 `parse_toml<T>` 辅助即可统一。

合计可削减重复约 60-80 行，并把"路径安全"这一安全关键逻辑收口到 `io.rs` 单点。

---

## 4. 架构问题

### 4.1 双优先级表漂移风险（P1，设计层）

同一份"指令优先级"语义在两处各写一遍，且**没有任何测试保证两者数值一致**：

- `resource-loader`：`ResourceInstructionKind::priority()`（`loader.rs:37-49`），在 bundle 内排序用。
- `context-engine`：`ContextSource::priority`（`source.rs:15-44`），在最终上下文排序用。

`context-engine/src/resources.rs:67-101` 的映射测试**只断言变体映射正确**，不断言两表数值一致。一旦有人改了其中一处，另一处不会报错，确定性会静默退化。建议：要么让 context-engine 直接复用 resource-loader 的优先级，要么加一条"两表数值一致"的 cross-check 测试。

### 4.2 Session/Run 合并进 `AdHocInstructions`（P2）

`ResourceInstructionKind::SessionInstructions` 与 `RunInstructions` 在 `resources.rs:16-17` **合并为同一个 `ContextSource::AdHocInstructions`**。session 先于 run 的顺序**无法用 `ContextSource` 优先级表达**，只能靠把 `tier.priority()` 焊进 `source_key` 字符串前缀（`resources.rs:21-24`）来恢复。

这是"一个 ContextSource 变体承载两个语义层级"的泄漏——排序键里嵌进了 config-tier 的数值编码。功能正确，但脆弱。可考虑给 `ContextSource` 增加 session/run 区分，或文档化这条隐性依赖。

### 4.3 热重载 / 诊断视图：实现质量高但属"过早基础设施"（P2）

`watch.rs` 的实现细节（drop 停监、注册后立即补 reload 关闭初始窗口、锁外重建避免持锁、失败保留旧快照）都是正确的生产级实践。但**目前没有任何外部消费者**，它处于"已建好但没人接电源"状态。

这不是建议删除——P13 接线时确实需要它。但它的存在让 P8-8 的 🟢 略显乐观：更准确的描述是"实现并测试完毕，等待 P13 接线"。诊断视图同理。

---

## 5. 合并 / 拆分 / 删除建议

按优先级与风险给出（**本 Review 不执行任何修改**）：

### 建议删除（零风险，纯减负）

- 删 `LoadResources` trait（`loader.rs:84`）——单实现无分发，保留具体 `ResourceLoader`。
- 删死方法 `ResourceBundle::diagnostic_view`、`ResourceLoader::options`、`PromptTemplate::render`、`AgentsDocument::{new, into_documents, iter, is_empty}`。
- 删 `ResourceOrigin::Builtin`（从不在生产构造）。
- 删 `ResourceLimits.max_include_depth`（自述"预留"，待 include 语义落地再加）。

### 建议简化（低风险）

- Skills 依赖解析：把 `resolve_active` 不动点 + `bfs_supported` 重收敛 + `collect_dep_issues` 重匹配，替换为单次 BFS + 复用匹配结果生成诊断（约 -113 行 + 对应测试）。
- `ResourceBundle` 二选一：去掉 `instructions` 让消费方扁平化，或去掉结构化五字段只留 `instructions + diagnostics`。

### 建议合并（低风险）

- 标识符校验、安全相对路径校验、TOML 解析错误——收口到 `io.rs` / 单个 helper，去掉 4+2+3 份重复。

### 建议补强（防御性，仅测试/文档）

- 加"双优先级表数值一致"cross-check 测试（§4.1）。
- 文档化 session/run → AdHocInstructions 的 tier-prefix 隐性依赖（§4.2）。
- 在 docs/features/skills.md 给"解析后暂无消费者"的字段（§3.2）加 deferred-consumer 标记，避免误读为已生效。

### 不建议改动

- `ResourceIssue` / `ResourceLimits` / `ResourceRequest` / `ResourceInstruction` 的 request-vs-source 拆分是真实职责边界，非冗余。
- `ResourceLoadError`（致命）vs `ResourceFileError`（可隔离）的拆分合理。
- `io.rs` 的安全边界 helpers 是有目的的薄封装，不是 std 重复。
- agents.rs 的发现/排序逻辑正确且最小，仅删死访问器即可。
- ConfigTier 六级真实复用，**无重复实现**——这是本 Phase 做得对的地方。

---

## 6. 改进优先级矩阵

| 优先级 | 项 | 收益 | 风险 | 时机 |
|---|---|---|---|---|
| P0 | P13 接线时端到端验证 resource-loader（§2） | 让所有"已实现无消费者"的能力首次被真实路径验证 | 无 | P13 |
| P1 | 双优先级表加 cross-check 测试（§4.1） | 防止确定性静默退化 | 零 | 现在可做 |
| P1 | 删 `LoadResources` trait + 死方法/死字段（§3.3） | 减少公共面 ~40 项概念 | 零 | 现在可做 |
| P2 | Skills 依赖解析简化（§3.1） | -113 行机器 + 对应测试 | 低（语义不变，需保测试通过） | 现在可做 |
| P2 | ResourceBundle 二选一（§3.4） | 消除双状态维护负担 | 低 | 与 P13 接线一起做更稳 |
| P2 | 合并重复校验（§3.5） | -60~80 行，安全逻辑收口 | 低 | 现在可做 |
| P3 | deferred-consumer 字段文档标记（§3.2） | 防止误读 | 零 | 现在可做 |

---

## 7. 整体评价

Phase 8 的**架构选择是正确的**：单一 `resource-loader` crate + 中性 `ResourceInstruction` DTO + 单向被 `context-engine` 消费，依赖方向干净，ConfigTier 真实复用而非重复实现，确定性有反向输入测试守护。这套结构符合 Pawork "agent-domain 不依赖具体实现、确定性上下文"的红线。

真正的问题是**时序与复杂度的错配**：在尚无任何运行消费者的阶段，Skills 子系统已经长出了 declare-only 路径用不到的 semver 不动点解析，Templates 已经带上了从不适用的默认配置，热重载与诊断视图已经达到生产级——这些都不会错，但它们当前的全部产出（除了被扁平化的 instruction 文本）**只有诊断条目**。

按本次 Review 的导向（"优先寻找可以减少代码、模块、接口和概念数量的方案"），最值得做的是 §5 的"删除/简化/合并"三组——它们共同能在不损失任何当前可观测语义的前提下，显著收窄这个 crate 的概念面，让 P13 接线时面对的是一个更小、更诚实的加载器。

---

## 附：调查覆盖与证据

本次 Review 由 4 个 `deepseek_explorer` 并行调查以下不重叠切片，证据均为 `file:line`：

- 核心结构：`lib.rs` / `loader.rs` / `request.rs` / `source.rs` / `error.rs` + 公共面盘点。
- Skills + Templates：`skills.rs`（~44KB）/ `templates.rs`（~25KB）全读 + 依赖/冲突机器逐函数行数核算。
- Watch + IO + Diagnostics + Profiles + Agents：`watch.rs` / `io.rs` / `diagnostics.rs` / `profiles.rs` / `agents.rs` 全读 + 跨仓库 `rg` 消费者核查。
- 集成与消费者：`context-engine` 的 `resources.rs` / `source.rs` / `builder.rs` + 全仓库依赖与调用核查（确认 app-service/cli-host/agent-engine 零依赖）。
