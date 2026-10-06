# B4 · Workspace / App Prompt-template 预留裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决未落地资源类型与 Prompt-template 的跨包保留范围。

## 当前线索与读取范围

登记项为 `ResourceSelection.prompt_template/prompt_arguments`、`ResourceLimits.max_template_file_refs/max_rendered_prompt_bytes`、`ResourceKind::{PromptTemplate,LanguageServer,UserHook}` 和 `ResourceInstructionKind::PromptTemplate` 的 app 穷举死臂。

[workspace Spec](../spec/crates/workspace.md)、[app Spec](../spec/crates/app.md)、[候选池](../spec/backlog.md)。

## 实施范围

- 核对以上字段、变体、配置和消费面；确认候选能力与现行资源功能的界线。
- 若决定删除，跨 workspace / app 同批处理必要引用和 Spec，不新增兼容副本。

## 完成条件

- 删除或保留的公开契约、配置与序列化影响有明确裁决。
- 现有资源发现、指令加载和预算边界的相关回归通过。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

