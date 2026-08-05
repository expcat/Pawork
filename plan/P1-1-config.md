# P1-1：配置系统

> Phase 1 · 基础设施 · 状态：🟡未开始 · 依赖：P0-8

**最终目的**：建立确定性配置系统（用户全局 / 工作区 / session 配置合并），为后续 Agent Loop 的上下文与策略提供可预测的配置来源，避免配置扫描顺序导致非确定性结果。

**涉及范围**：`context-engine` 或独立 config crate

## 细分步骤

1. **定义配置层级 schema** —— user global / workspace / session / run。目的：明确配置来源与优先级。
2. **实现优先级合并** —— 内置 < 用户全局 < profile < 工作区 < session < run。目的：确定性合并，不依赖扫描顺序。
3. **配置发现与加载** —— 跨平台配置目录、工作区配置。目的：可定位配置文件。
4. **合并确定性测试** —— 相同输入产生相同结果。目的：保证可复现。

## 主要产出物

- config 模块 + 合并逻辑 + 测试

## 验收标准

- [ ] 配置合并不依赖扫描顺序（确定性）
- [ ] 跨平台配置路径正确

**相关文档**：[context](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：开工前先决定「独立 config crate 还是并入 context-engine」（倾向独立 config crate：P1-1 早于 context-engine 使用点、合并语义独立），选定后按 [workspace-layout §7](../docs/architecture/workspace-layout.md) 登记。解析用 toml + serde；合并逻辑可参考 config-rs，但全局 / 工作区 / session / CLI 的确定性优先级合并必须自实现。
