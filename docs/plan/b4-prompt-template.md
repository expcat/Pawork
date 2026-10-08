# B4 · Workspace / App Prompt-template 预留裁决

> 状态：✅ 完成。完成日期 / 更新：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决未落地资源类型与 Prompt-template 的跨包保留范围。

## 裁决与事实依据

基线已核实为 `main` / `1e72b7bd8a06c8f482835690b35683f3877e4295`（B3 本地提交），实施前工作区干净。先完整读取 AGENTS.md、本文和两篇包级 Spec，再经 CodeGraph 定位，补读未返回的目标源码；按需核对 [候选池](../spec/backlog.md) A1 / BK-EXT-01 与 [架构 §3](../architecture.md#3-冻结契约与追加不重写)。

| 登记项 | 裁决 | 依据 |
| --- | --- | --- |
| `ResourceSelection.prompt_template` | 删除 | App 只选择 profile / active skills，加载器不发现或选择模板；serde 请求仅在宿主内构造，不是配置、磁盘或 wire 输入。 |
| `ResourceSelection.prompt_arguments` | 删除 | 无参数读取或渲染路径；随内部选择类型导出不构成已激活模板能力。 |
| `ResourceLimits.max_template_file_refs` | 删除 | 除声明 / 默认值外无读取；`ResourceLimits` 无 serde，不进入配置。 |
| `ResourceLimits.max_rendered_prompt_bytes` | 删除 | 没有模板展开消费者；App 现行 `@file` 使用自身文件 / 图片上限，不消费此预算。 |
| `ResourceKind::PromptTemplate` | 删除 | 无加载、诊断构造或消费；A1 仍是未排期候选。 |
| `ResourceKind::LanguageServer` | 删除 | resources 无 LSP 加载路径；BK-EXT-01 未激活。 |
| `ResourceKind::UserHook` | 删除 | resources 无 hook 加载路径；compatibility 导入的 `ImportCategory::UserHook` / `CompatPayload::UserHook` 是另一组现行类型，继续保留。 |
| `ResourceInstructionKind::PromptTemplate` | 删除 | 仅枚举声明、priority 与 App 穷举映射，无指令生产者；compatibility 导出也不生成此 kind。 |

保留实际加载的 Instructions / AgentsFile / Skill / AgentProfile 四类资源，以及 profile、全局 / workspace 指令、root / path AGENTS、active skill、session / run 八类注入指令。其 kind 字符串、相对排序与显式 priority（2 / 4 / 5 / 6 / 7 / 8 / 13 / 14）不变。保留 `ResourceSelection` 的 active / disabled skills、profile、session / run instructions，以及实际消费的 `ResourceLimits.max_file_bytes`（1 MiB）/ `max_resources_per_kind`（1024）。

### 序列化、磁盘与外部承诺

- `ResourceRequest` / `ResourceSelection` 虽 derive serde，但真实构造只在 resources 与 App 内部；`PaworkConfig`、SQLite、canonical 事件、CLI / protocol 与检入的三类 schema 均不包含这些类型。删除两个字段收窄内部 Rust / serde 形状，不修改配置的未知键保留行为。
- `ResourceKind` 只用于资源诊断；诊断序列化用在资源回归，不构成生产磁盘 / wire 出口。App 把指令映射为 engine 的 `InjectedLayer` 文本；持久 `resources.injected` Diagnostic 的 kind 是普通字符串，既有事件与重放不按此枚举解码。
- `ResourceInstructionKind` 确实嵌在 `CompatPayload::Instructions` 并写入 `compat-import.json`。已核对 [扫描生产者](../../crates/workspace/src/import/parse.rs)：仅产出 `user_global_instructions` / `workspace_instructions`；[App 应用入口](../../crates/app/src/services/import.rs) 从扫描结果应用，未从导出文件恢复资源请求。保留现行计划所有已产出标签与 `manifest_version=1`，无需磁盘迁移；现有导出回归增加扫描 → 写盘 → 反序列化相等断言。
- Cargo workspace 明确 `publish=false`；README、稳定契约目录、架构 §3.2 与生成 schema 没有承诺这些候选字段 / 变体。候选登记没有激活消费面，依据架构 §3.3 收窄内部 API，不为未激活 serde 标签增加兼容副本。A1 / BK-EXT-01 继续留在候选池，未激活模板、Hooks、LSP 或插件框架。

## 实际写入集

共 9 个文件：

- [resources/request.rs](../../crates/workspace/src/resources/request.rs)：删两个模板选择字段、两个未消费限制与不再需要的 `BTreeMap` import。
- [resources/source.rs](../../crates/workspace/src/resources/source.rs)：删三个未加载变体及 `as_str` 臂。
- [resources/loader.rs](../../crates/workspace/src/resources/loader.rs)：删模板指令变体及 priority 臂。
- [App extensions.rs](../../crates/app/src/extensions.rs)：删模板映射死臂。
- [workspace smoke.rs](../../crates/workspace/tests/smoke.rs)：既有导出用例补 3 行磁盘计划往返断言。此紧相关回归是实际 serde 磁盘消费面的验证，不修改 import 实现，不新增测试用例或依赖。
- [workspace Spec](../spec/crates/workspace.md)、[App Spec](../spec/crates/app.md)、本文、[ROADMAP](../ROADMAP.md)：同步公开面、现行 / 候选边界与真实状态。App Spec 仅追加本次注入说明，保留 B3 额度 / 租约事实校正。

## 完成条件

- 删除或保留的公开契约、配置与序列化影响有明确裁决。
- 现有资源发现、指令加载和预算边界的相关回归通过。
- 指挥安排独立只读 review；若有问题，修复与复审通过后再关闭任务。

## 实现与验证记录

2026-10-08：实现与确定性检查完成，92 条定向回归通过；随后独立只读 review PASS，满足本任务完成条件。

日志目录：`/tmp/pawork-b4-20261008-12enburp`。每次 Cargo 前确认没有其它 Cargo 进程，串行执行；保持 `.cargo/config.toml` 的 kache wrapper，无 clean、真实 Provider 或全 workspace 门禁。

| 命令 | feature / 目标 | exit | 通过 / 忽略 / filtered |
| --- | --- | --- | --- |
| `cargo test -p pawork-workspace --offline --lib resources::` | 默认 feature；resources 单元回归 | 0 | 60 / 0 / 78 |
| `cargo test -p pawork-workspace --offline --test smoke` | 默认 feature；compatibility 集成回归 | 0 | 15 / 0 / 0 |
| `cargo test -p pawork-app --offline --lib services::extension::tests` | 默认 feature；注入与附件回归 | 0 | 15 / 0 / 284 |
| `cargo test -p pawork-app --offline --lib gui_host::handlers::recording::tests` | 默认 feature；技能录制 / 加载与拒绝 | 0 | 2 / 0 / 297 |

App 没有额外开启 `ui-fixture` / `live-smoke`；其 manifest 固定开启 providers 的 anthropic / chatgpt-oauth / xai-oauth / glm-coding / opencode-go / qwen-token-plan / deepseek / kimi-platform / kimi-code、storage 的 compaction / checkpoint / protected，以及 dev transport 的 local / memory。每条命令的完整参数、feature、耗时、exit 与测试计数均写在同名 `.json`；输出保存在 `.log`。

Validated: 上表 4 条命令（合计 92 passed，0 failed / ignored）、`git diff --check`、5 个 Rust 文件的 `rustfmt --check --edition 2021`、改动文档本地链接检查、源码 / schema 预留引用归零检查、现行枚举 / priority / App 映射与 HEAD 比较、9 文件写入集检查。

Targeted regressions: 资源发现与确定性排序、AGENTS / skills / profiles / session / run 指令加载、单文件字节上限 / UTF-8 / 路径越界 / Secret 隔离、现行 compatibility 计划往返与禁用 hook / Secret 语义、App 目标 workspace 信任过滤、独立 `@file` 文本 / 图片上限与安全边界、录制技能安全保存与再加载。

Full workspace gate: NOT RUN（当前未设置全量门禁）。

## 独立审查与完成记录

2026-10-08：实现对话 `01a118e1-95ea-7481-8f30-b17f18db5f90` 与独立审查对话 `01a118f0-b606-7623-bea5-3e3b8a0ecc83` 均正常完成，明确使用 `gpt-6.1-sol` / thinking `max`。审查结论 **PASS**，无本改动引入的可执行问题；审查者只读源码、生产消费历史、当前 diff 与上述日志，没有修改文件、编译或运行测试。

审查确认扫描生产者自激活以来只产出两类现行指令标签、App 应用扫描计划、事件持久 kind 不按收窄后的枚举解码；现行加载、排序、有效预算、信任过滤与 compatibility hook 禁用记录保持。审查时当前 diff 与 `final.diff` 的 SHA-256 一致，四份原始测试日志均为 exit 0，92 passed、0 failed / ignored；App Spec 的 B3 内容未被覆盖。

指挥同步本文与 ROADMAP 的完成状态，并仅按上述 9 文件写入集本地提交，不推送或发布。本任务无未闭合验收项；模板 / Hooks / LSP 仍是未激活候选，产品激活不属于本次裁决。
