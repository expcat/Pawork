# P2-2：SSE 解析器

> Phase 2 · 首个真实 Provider · 状态：🟡未开始 · 依赖：P2-1

**最终目的**：实现健壮的 SSE（Server-Sent Events）流解析，正确处理跨 chunk、Unicode 边界，为流式 provider 响应提供可靠解析。

**涉及范围**：`provider-runtime`

## 细分步骤

1. **事件/字段解析** —— data/event/id/retry。目的：标准 SSE 语义。
2. **跨 chunk 拼接** —— 目的：分片到达不丢。
3. **Unicode 边界处理** —— 目的：多字节字符不被截断。
4. **fuzz 目标** —— 目的：乱码不崩溃。

## 主要产出物

- SSE 解析器 + fuzz 测试

## 验收标准

- [ ] 跨 chunk、Unicode 边界正确
- [ ] fuzz 不 panic

**相关文档**：[providers](../docs/features/providers.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：自实现，参考 eventsource-stream 的状态机与 TS eventsource-parser（Pi / Vercel 使用的实现）；eventsource-stream 采用率虽高（约 1.5k dependents）但维护停滞，且本任务的 fuzz / 畸形输入要求明确，不直接引入。
