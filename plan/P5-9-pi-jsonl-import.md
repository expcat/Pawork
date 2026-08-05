# P5-9：Pi JSONL Importer

> Phase 5 · Session、Branch 与 Compaction · 状态：🟡未开始 · 依赖：P5-8

**最终目的**：实现 Pi JSONL Session 导入器（扫描/解析 header/消息/tool call/模型切换/compaction/branch、保存未知字段、迁移报告），让用户可迁移历史会话且不修改原文件（ADR-005）。

**涉及范围**：`session-store`

## 细分步骤

1. **扫描与 header 解析** —— 目的：识别 Pi 格式。
2. **消息/tool call/模型切换/compaction/branch 解析** —— 目的：还原会话结构。
3. **保存未知字段** —— 目的：兼容未来格式。
4. **迁移报告 + 不修改原文件** —— 目的：安全可审计。

## 主要产出物

- Pi JSONL Importer

## 验收标准

- [ ] 导入不修改原 Pi 文件
- [ ] 保存未知字段
- [ ] 生成迁移报告

**相关文档**：[sessions](../docs/features/sessions.md) · [ADR-005 Pi JSONL 导入](../docs/adr/ADR-005-pi-jsonl-import-only.md) · [ROADMAP](../ROADMAP.md)
