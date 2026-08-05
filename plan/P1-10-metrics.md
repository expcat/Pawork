# P1-10：Metrics

> Phase 1 · 基础设施 · 状态：🟡未开始 · 依赖：P1-9

**最终目的**：实现关键 metrics 采集（初始化时间 / 首 token / tool 耗时 / token / 内存 / backlog），为性能门禁提供量化依据。

**涉及范围**：`diagnostics`

## 细分步骤

1. **定义指标集** —— 目的：统一口径。
2. **采集与导出** —— 目的：可采集、可观测。
3. **与基准/门禁挂钩** —— 目的：回归可见。

## 主要产出物

- metrics 模块

## 验收标准

- [ ] 关键指标可采集（初始化/首 token/tool 耗时/token/内存/backlog）

**相关文档**：[observability](../docs/features/observability.md) · [性能目标](../docs/quality/performance-targets.md) · [ROADMAP](../ROADMAP.md)
