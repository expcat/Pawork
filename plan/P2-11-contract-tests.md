# P2-11：Provider Contract Tests

> Phase 2 · 首个真实 Provider · 状态：🟢已完成 · 依赖：P2-5

**最终目的**：建立统一 Provider Contract Test 套件（ADR-015），让每个 provider 行为可横向对比、可回归，新增 provider 须过套件才能合并。

**涉及范围**：`test-support`、`provider-openai-compatible`

## 细分步骤

1. **用例集** —— text/tool/multi-tool/cancel/timeout/ratelimit/malformed/partial/reconnect/overflow。目的：覆盖关键差异点。
2. **统一断言** —— 目的：行为可对比。
3. **OpenAI-compatible 过套件** —— 目的：首个 provider 达标。
4. **CI 接入** —— 目的：回归保护。

## 主要产出物

- Contract Test 套件 + OpenAI-compatible 结果

## 验收标准

- [x] text/tool/cancel/timeout/ratelimit/context overflow/流中断 全部通过

**相关文档**：[providers](../docs/features/providers.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
