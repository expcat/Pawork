# P1-11：诊断包导出

> Phase 1 · 基础设施 · 状态：🟡未开始 · 依赖：P1-9

**最终目的**：实现诊断包导出（版本 / OS / Provider / 模型 / DB schema / 插件 / MCP / 脱敏日志），便于排障且默认不含 secret / 消息 / 文件内容。

**涉及范围**：`diagnostics`

## 细分步骤

1. **收集环境与运行态信息** —— 目的：排障上下文。
2. **脱敏与裁剪** —— 目的：隐私安全。
3. **导出打包** —— 目的：可分享。

## 主要产出物

- 诊断包导出

## 验收标准

- [ ] 默认不含 secret / 消息内容 / 文件内容

**相关文档**：[observability](../docs/features/observability.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
