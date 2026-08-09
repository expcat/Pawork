# P2-4：Partial JSON 拼接

> Phase 2 · 首个真实 Provider · 状态：🟢已完成 · 依赖：P2-3

**最终目的**：实现 tool arguments 跨多 chunk 的 partial JSON 增量拼接，支持多个 tool call 并行流的实时组装。

**涉及范围**：`provider-runtime`

## 细分步骤

1. **增量 JSON 缓冲与拼接** —— 目的：跨 chunk 组装 arguments。
2. **多 tool call 并行索引** —— 目的：多个 tool call 同时流式。
3. **容错（部分无效）** —— 目的：不因单 chunk 异常崩溃。
4. **多场景测试** —— 目的：覆盖典型分片。

## 主要产出物

- Partial JSON 拼接器

## 验收标准

- [x] 多 tool call 并行流可正确组装

**相关文档**：[providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：自实现，需要确定性修复语义；可参考 llmx / tool-parser / Vercel partial-json（现有 crate 采用率低、测试弱，仅作设计参照）。
