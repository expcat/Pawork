# P6-6：图片输入

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 依赖：P6-1、P6-2、P6-3

**最终目的**：统一图片输入（image content part），让 Agent 可接收并传递图片给多模态模型。

**涉及范围**：`provider-*`、`agent-domain`

## 细分步骤

1. **image content part 规范** —— 目的：canonical 图片表示。
2. **provider 差异处理** —— 目的：各 provider 图片格式对齐。
3. **大小/格式约束** —— 目的：防超限。
4. **多 provider 测试** —— 目的：一致可用。

## 主要产出物

- 图片输入统一

## 验收标准

- [ ] 图片可作为 content part 传入主要 provider

**相关文档**：[providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)
