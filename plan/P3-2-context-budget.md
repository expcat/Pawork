# P3-2：上下文构建与预算

> Phase 3 · Agent Loop · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-1

**最终目的**：实现确定性上下文构建（来源优先级 + token 估算 + output/thinking reserve + 超限前压缩触发），保证每次请求上下文可预测、不越界。

**涉及范围**：`context-engine`

## 细分步骤

1. **上下文来源优先级** —— 目的：确定性排序。
2. **token 估算** —— 目的：预算可控。
3. **output/thinking reserve** —— 目的：为输出与思考预留空间。
4. **超限前压缩触发** —— 目的：自动压缩而非截断。

## 主要产出物

- 上下文构建 + token 预算

## 验收标准

- [x] 构建确定性（同输入同输出）
- [x] 超限自动触发压缩

**相关文档**：[context](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：token 计数用 tiktoken-rs，仅对 OpenAI 系模型精确；其它 Provider 一律启发式估算 + 容差（与 [context](../docs/features/context.md) 约定一致）。
