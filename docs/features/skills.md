# Skills、Prompts 与 Instructions

## 职责

加载并按确定优先级组合 Skills、Prompt 模板与项目指令（`AGENTS.md` 等），为上下文构建提供可诊断的来源。

Phase 8 已落地独立 `resource-loader` crate。工作区侧入口只接受 `workspace_id + root_index + relative_path`，拒绝绝对路径与 `..`；宿主解析的用户全局资源目录不暴露给模型。单个文件缺失、超限、非 UTF-8、格式错误或冲突会变成 `ResourceIssue`，不会中断其他资源加载或导致 Core 崩溃。

## 目录约定

```text
<global-resource-dir>/
├── instructions.md
├── skills/<id>/{manifest.toml,SKILL.md,...}
├── prompts/<id>.md
└── profiles/<name>.toml

<workspace-root>/
├── AGENTS.md                         # 可在目标路径的每一级目录出现
└── .pawork/
    ├── instructions.md
    ├── skills/<id>/{manifest.toml,SKILL.md,...}
    ├── prompts/<id>.md
    └── profiles/<name>.toml
```

工作区同 ID 资源覆盖用户全局资源；多 Root 和同层重复使用稳定 `source_key` 决策并生成诊断，不依赖文件系统扫描顺序。

## Skills 格式

```text
skill/
├── SKILL.md
├── manifest.toml
├── prompts/
├── scripts/
└── assets/
```

Manifest：

```toml
id = "rust-review"
version = "1.0.0"
description = "Review Rust code"
permissions = ["filesystem.read", "process.cargo"]
```

支持全局/工作区 Skills、显式激活与禁用、参数、资源文件、脚本入口、权限声明、semver 版本与依赖、双向冲突检测和热重载。脚本与资产在本阶段只作为声明加载，Resource Loader 不执行脚本；路径必须是包内相对路径且不得包含 `..`。激活会递归拉取满足版本约束的依赖，disabled 优先；缺失、版本不符或冲突的技能会被确定性拒绝并保留诊断。

## Prompt Templates

Prompt 是带 TOML `+++` frontmatter 的 Markdown：

```markdown
+++
id = "review"
files = ["src/lib.rs"]

[parameters.target]
required = true
default = "workspace"

[defaults]
model = "example-model"
thinking = "high"
tools = ["read_file"]
budget = 8000
+++
Review {{target}} using {{file:src/lib.rs}}.
```

支持 `{{name}}` 参数、默认值、必填校验、工作区相对文件引用，以及默认 model/thinking/tools/budget。每个 `{{file:path}}` 必须先在 frontmatter `files` 中显式声明；引用次数、单文件大小与最终渲染总字节数均受 `ResourceLimits` 约束，重复引用使用单次读取缓存。文件引用经过 canonical 边界检查，不能通过绝对路径、`..` 或 symlink 离开当前 Workspace Root。工作区模板按 ID 覆盖全局模板。

## Instructions

支持全局 Instructions、工作区 Instructions、根与当前文件路径层级 `AGENTS.md`、激活 Skills、Prompt Template、Agent Profile v1、Session 与单次 Run Instructions。`AGENTS.md` 始终按根到当前路径排序，离当前文件最近的指令最后进入该层。

Agent Profile v1 格式：

```toml
name = "reviewer"
instructions = "Review correctness and cite evidence."
default_provider = "openai"
default_model = "example-model"
```

v1 只包含可命名配置、instructions 与 provider/model 默认值；tools/denied、skills、MCP、permissions、hooks、memory、max-turns、background、isolation 等完整维度属于 [P17-5 Agent Profile v2](../../plan/P17-5-agent-profile-v2.md)。运行期指令按 profile < session < run 排序，单次 Run 最后生效。

## 确定性与 Context Engine 接线

配置继续复用 `config-service::ConfigTier` 的六级顺序：

```text
Builtin < Global < Profile < Workspace < Session < Run
```

`resource-loader` 输出不依赖 `context-engine` 的中性 `ResourceInstruction` DTO；`context-engine` 单向依赖并映射为已有 14 类 `ContextSource`，随后按 `(ContextSource priority, ConfigTier priority, source_key)` 稳定排序。相同输入与资源树产生相同上下文。这里交付的是 Phase 8 的 Resource/Context 生产契约；`pawork` Host 的 Run 编排仍按 Phase 13 接入 `app-service/core-runtime`，当前 app-service 骨架不伪造实际 Run。

## Resource Diagnostics

每个候选资源记录 kind、resource ID、ConfigTier、稳定 source key、workspace-relative origin 与 Loaded/Active/Overridden/Disabled/Rejected 状态。`ResourceDiagnosticView` 可输出稳定文本或 JSON，用来解释「为什么生效」。诊断是 allowlist：不包含 instruction、prompt、Skill/脚本正文或宿主绝对路径，问题消息进入视图前复用 `diagnostics::Redactor` 脱敏。

## Hot Reload

`ResourceLoader::watch` 使用 `notify-debouncer-full` 递归监听全局资源目录与 Workspace Roots；全局资源目录尚不存在时监听最近的既存父目录，以捕获后续创建。注册 watcher 后立即补做一次 reload，覆盖初次加载到注册完成之间的窗口；所有 reload 通过独立互斥量串行化，但资源构建期间不持有快照状态锁。成功后以 `Arc` 原子替换并递增 generation；致命重建失败保留最后一次成功快照并记录脱敏错误。`ResourceWatcher` drop 即停止监听，也支持 `reload_now` 手动刷新。

## 已解析但暂无消费者（deferred-consumer）

以下字段在 Phase 8 已完成解析与校验，但截至当前没有任何运行期消费者；它们是为 Phase 13 Host-Run 接线预留的前向兼容契约（见 [p8-review §3.2](../../review/p8-review.md#32-skillstemplates-字段解析后从不读取)），不应被误读为「已生效」：

- Skills `manifest.parameters` / `scripts` / `assets` / `permissions`：只作为声明加载，Resource Loader 不执行脚本、不应用权限。
- Templates `PromptDefaults.{model,thinking,tools,budget}` 与 `RenderedPrompt.included_files`：渲染结果携带但无人读取。
- AgentProfile `default_provider` / `default_model`：仅 resource-loader 自持副本；config-service 另有独立生效副本。
- ResourceBundle 结构化字段（agents/skills/templates/profiles/resolved_instructions）：仅 crate 内部测试消费，context-engine 只读取 `.instructions`。
- `ResourceDiagnosticView`：诊断视图基础设施，按 [p8-review §4.3](../../review/p8-review.md#43-热重载--诊断视图实现质量高但属过早基础设施p2) 延迟至 P13 接线。
- ResourceHotReload / `watch`（P8-8）：实现并测试完毕，等待 P13 接线（同见 §4.3）。

这些字段被完整解析、校验并携带，但尚无运行期消费者；P13 接线前请勿将其视为已生效配置。

## 验收标准

- [x] 能显示所有有效指令来源
- [x] 相同配置始终产生确定性上下文
- [x] Resource 加载错误不导致 Core 崩溃
- [x] 文件变更后去抖重载并原子替换快照

## 相关文档

- [context](context.md) · [workspace-index](workspace-index.md)
- [ROADMAP Phase 8](../../ROADMAP.md)
