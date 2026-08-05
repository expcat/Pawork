# 可观测性

## 职责

提供结构化日志、指标与诊断包，且全程自动脱敏，保证 Secret 不泄漏。

## 日志字段

```text
timestamp / level / component / workspace_id / session_id / run_id
provider / model / tool_call_id / trace_id / duration / error_code
```

## 自动脱敏

API Key；Bearer Token；Cookie；OAuth Code；Authorization Header；用户配置的 Secret Pattern。

## Metrics

Core 初始化时间；数据库时间；Provider 首 Token；Provider 总时长；Tool 执行时长；Context Token；Compaction 次数；Session 打开时间；Diff 生成时间；文件索引时间；内存；活跃 Task；Channel backlog；Blob Store 大小。

## Diagnostics Bundle

用户可导出：核心版本；OS；Provider 状态；模型目录；数据库 schema；插件列表；MCP 状态；最近脱敏日志；性能指标；崩溃报告。

默认不包含：Secret；完整用户消息；文件内容；Tool Output（除非用户明确选择包含）。

## 验收标准

- 日志与诊断包默认不含明文 Secret
- 关键指标可采集
- 诊断包可离线导出

## 相关文档

- [auth（脱敏状态）](auth.md) · [CLI Host（doctor）](cli-host.md)
- [ROADMAP P1-9 / P1-10 / P1-11](../../ROADMAP.md)
