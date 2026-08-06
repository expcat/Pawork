# Context Engine 与 Compaction

## 职责

按确定优先级构建上下文，估算与分配 Token 预算，裁剪大型工具输出，并在超限前触发压缩。

## 上下文来源（按优先级组合）

1. Core System Prompt
2. Agent Profile
3. 安全和权限策略
4. 用户全局 Instructions
5. 工作区 Instructions
6. 根目录 `AGENTS.md`
7. 当前文件路径层级的 `AGENTS.md`
8. 激活的 Skills
9. 用户选择的 Prompt Template
10. Session Summary
11. 历史消息
12. 文件和图片附件
13. 工具定义
14. 临时运行指令

## Resource 优先级

```text
内置默认值
  < 用户全局配置
  < 用户 Profile
  < 工作区配置
  < 当前 Session 配置
  < 单次 Run 参数
```

不得依赖文件扫描顺序。

## Phase 1 配置基线

`config-service` 已落地内置默认值、用户全局、Profile、Workspace、Session 与单次 Run 六级来源。来源先按层级、再按稳定的 `source_key` 排序；对象递归合并，标量与数组由高优先级来源替换。配置文件定位、解析与 schema 错误均保留来源路径，跨平台系统目录与最近 Workspace 配置发现已有回归测试。

## Token 预算

Context Engine 负责：Provider Token 估算；System Prompt 占用；Tool Schema 占用；附件占用；历史消息占用；Output Reserve；Thinking Reserve；超限前 Compaction；不支持精确 tokenizer 时的安全估算。

## Tool Result 裁剪

```text
小结果：完整加入
中等结果：头部 + 尾部 + 截断说明
大型结果：摘要 + Artifact 引用
超大结果：只保留元数据和查询接口
```

## Compaction Engine

支持：自动压缩；手动压缩；分支摘要；工具结果清理；历史消息摘要；保留最近 N 轮；保留未解决任务；保留用户约束；保留修改文件列表；保留失败和待处理工具调用；摘要版本化；压缩前后 Token 统计。

```rust
pub struct CompactionSnapshot {
    pub summary: String,
    pub retained_event_ids: Vec<EventId>,
    pub replaced_range: EventRange,
    pub token_usage_before: u64,
    pub token_usage_after: u64,
}
```

## 验收标准

- 相同配置始终产生确定性上下文
- 压缩后保留任务、用户约束与修改文件列表
- 压缩前后 Token 统计可查
- 可恢复压缩前 Branch

## Phase 5 上下文与压缩基线

`context-engine` 已实现 Tool Result 分级裁剪（`trim_tool_result`）：按字节体量分为小（完整保留）/ 中（头部 + 尾部 + 截断说明）/ 大（摘要 + ArtifactReference 占位）/ 超大（仅元数据 + ArtifactReference），大/超大输出原文经 `retained_full` 暂存以便回溯（ADR-018）。`compaction-engine`（新 crate）已落地版本化 `CompactionSnapshot`（`SnapshotVersion` + `validate()`）、自动/手动压缩统一入口（压缩前用 `create_branch` Fork recovery branch）、保留策略 `RetentionPolicy`（最近 N 轮、未解决任务、用户约束、修改文件、待处理/失败 tool call）与 Golden Session 回归；引擎只产快照与决策，向事件流追加 `CompactionStarted/Completed` 与上下文重建由调用方完成。

## 相关文档

- [agent-engine](agent-engine.md) · [skills](skills.md) · [sessions](sessions.md)
- [ROADMAP P5-5/P5-6/P5-7、Phase 8](../../ROADMAP.md)
