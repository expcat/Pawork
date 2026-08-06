# P6-7：Prompt Cache

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 依赖：P6-1、P6-2

**最终目的**：实现 prompt cache 控制（主要 Anthropic），降低重复上下文成本。

**涉及范围**：`provider-*`

## 细分步骤

1. **缓存控制点** —— 目的：标记可缓存前缀。
2. **cache hit 透传** —— 目的：usage 反映命中。
3. **与上下文构建协作** —— 目的：稳定前缀提升命中。
4. **命中测试** —— 目的：可验证。

## 主要产出物

- Prompt Cache 控制

## 验收标准

- [ ] 缓存命中可体现在 usage

**相关文档**：[providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：Anthropic 需显式 cache_control 标记；OpenAI 的 prompt caching 为自动命中（usage 中体现），故依赖含 P6-1。
