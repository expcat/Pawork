# P0-7：错误与取消模型

> Phase 0 · 架构与协议冻结 · 状态：🟡未开始 · 依赖：P0-4、P0-5

**最终目的**：在跨 crate 层面统一错误类别与取消语义。否则各模块各自定义互不兼容的错误/取消，上层无法一致地 catch、重试与取消，会导致行为碎片化。

**涉及范围**：`agent-domain`、`provider-api`、`tool-api`

## 细分步骤

1. **定义统一错误类别** —— Provider/Tool/Internal/Cancelled/RateLimit/Timeout/Auth 等。目的：上层在一处即可归一处理。
2. **定义 CancellationToken** —— 跨 crate 共享的取消令牌（基于通知/event）。目的：provider/tool/loop 共用同一取消通道。
3. **规定错误转换约定** —— 各 crate 的 error 实现 Into 统一类别。目的：错误向上归一且不丢上下文。
4. **取消传播测试** —— 触发取消后 provider/tool 收到信号。目的：保证取消可达、不悬挂。

## 主要产出物

- 统一错误类别 + `CancellationToken` + 转换 trait

## 验收标准

- [ ] 跨 crate 错误类别统一
- [ ] 取消语义一致（provider 与 tool 都能被取消）
- [ ] 错误类别齐全（含 Cancelled/Timeout/RateLimit/Auth）

**相关文档**：[控制流](../docs/architecture/control-flow.md) · [ROADMAP](../ROADMAP.md)
