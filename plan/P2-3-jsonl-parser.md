# P2-3：JSON Lines 解析器

> Phase 2 · 首个真实 Provider · 状态：🟢已完成 · 依赖：P2-1

**最终目的**：实现 JSON Lines 流解析，正确处理 provider 提前断开与错误事件，为非 SSE 的流式 provider 提供解析。

**涉及范围**：`provider-runtime`

## 细分步骤

1. **逐行 JSON 解析** —— 目的：JSONL 语义。
2. **提前断开处理** —— 目的：连接中断可识别。
3. **错误事件归一** —— 目的：错误流可表达。
4. **fuzz 目标** —— 目的：乱码不崩溃。

## 主要产出物

- JSONL 解析器 + fuzz 测试

## 验收标准

- [x] 提前断开可识别
- [x] fuzz 不 panic

**相关文档**：[providers](../docs/features/providers.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
