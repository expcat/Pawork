# P2-10：重试与错误归一化

> Phase 2 · 首个真实 Provider · 状态：🟢已完成 · 依赖：P0-4

**最终目的**：实现单次 Provider 调用内的 transport/protocol 错误归一、建议重试时间与 bounded backoff。账号健康、credential failover、model/provider/protocol fallback 不在本任务决定，由 P18-5/P18-6 消费 `UpstreamFailure` 后处理。

**涉及范围**：`provider-runtime`

## 细分步骤

1. **可重试判定** —— 限流/超时/5xx/连接错误。目的：区分可重试与不可重试。
2. **建议重试时间（Retry-After 等）** —— 目的：尊重服务端节流。
3. **退避策略** —— 指数 + 抖动。目的：避免雪崩。
4. **错误类别归一测试** —— 目的：一致。
5. **控制面边界** —— `ClientCancelled`、`InvalidRequest`、`ContextTooLarge`、`ProtocolIncompatible` 只输出 canonical failure，不在 `provider-runtime` 轮换 credential；目的：避免 transport retry 与 account scheduling 耦合。

## 主要产出物

- 重试与错误归一化

## 验收标准

- [x] 错误类别齐全
- [x] 退避遵守 Retry-After
- [x] 本任务不读取账号池、不轮换 credential、不按 Provider 名在 core 走特例

**相关文档**：[providers](../docs/features/providers.md) · [provider-control-plane](../docs/features/provider-control-plane.md) · [P18-5](P18-5-error-health.md) · [P18-6](P18-6-routing-policy.md) · [ROADMAP](../ROADMAP.md)
