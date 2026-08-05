# P0-12：基准框架骨架

> Phase 0 · 架构与协议冻结 · 状态：🟡未开始 · 依赖：P0-1

**最终目的**：建立性能基准目录与计时口径说明，使后续 Phase 有统一基准落点（ADR-020），避免事后才补基准导致无法回归对比。

**涉及范围**：`benches`

## 细分步骤

1. **建立 benches 目录与分组** —— Core/Git/Provider/Model/Command/GUI。目的：区分不同耗时来源。
2. **编写空基准与计时口径说明** —— 目的：统一测量方法。
3. **接入 CI（可选 nightly）** —— 目的：回归可见。

## 主要产出物

- `benches` 骨架 + 计时口径文档 + 可运行空基准

## 验收标准

- [ ] 可运行空基准
- [ ] 计时口径有文档说明

**相关文档**：[性能目标](../docs/quality/performance-targets.md) · [ADR-020 性能安全门槛](../docs/adr/ADR-020-performance-security-gate.md) · [ROADMAP](../ROADMAP.md)
