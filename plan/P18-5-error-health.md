# P18-5：ErrorClassifier、Health、Cooldown 与 Circuit Breaker

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢库层有界完成 · 交付成熟度：LibraryBuilt（classifier / health / cooldown / circuit 已验证，正式 Route 尚未注入 HealthRuntime） · 依赖：P18-3、P2-10、P2-11、P15-9

**最终目的**：把 transport retry、credential/account failover 与 protocol fallback 分开，使失败只影响正确 scope，不把请求错误或客户端取消误判为账号故障。

**涉及范围**：`provider-control::error_classifier` / `health`；各 `provider-*` classifier 扩展点；contract fixtures

## 细分步骤

1. **canonical classification** —— 定义 `FailureClass`、`FailureScope`、`HealthImpact`、retry/failover 字段；目的：动作不直接绑定 HTTP status。
2. **Provider-specific classifier** —— 允许 adapter 识别 400 中的 account blocked、402、429 scope 与协议错误；目的：特例不泄漏进 core。
3. **Health 状态机** —— Healthy/Degraded/CoolingDown/BillingBlocked/Disabled 与 scope-aware Retry-After；目的：账号、模型、Provider 分别恢复。
4. **Circuit breaker** —— bounded retry、half-open probe、成功复原与连续失败阈值；目的：避免故障风暴。
5. **错误矩阵测试** —— 覆盖 401 refresh-once、402、provider-specific 400、429 有/无 Retry-After、QuotaExceeded（hard/soft 区分）、5xx、cancel、context-too-large、protocol incompatible、stream interruption；目的：锁定行为。

## 主要产出物

- `ErrorClassifier` / `ClassifiedFailure` / `HealthState`
- Provider classifier 扩展点与 cooldown/circuit runtime
- 错误矩阵 contract tests

## 验收标准

- [x] `ClientCancelled`、`InvalidRequest`、`ContextTooLarge` 不触发账号轮换或健康惩罚
- [x] `ProtocolIncompatible` 不盲目轮询所有 credential
- [x] 401 只 refresh-once，402/account blocked 可 failover，429 尊重 scope/Retry-After
- [x] core 不出现按 Provider 名分支

## 验证记录（2026-08-13）

- Validation Level: L1
- Affected crates: `provider-control`
- Validated: `cargo fmt -p provider-control -- --check`；`cargo test -p provider-control`（100 unit + 10 error-matrix）；`cargo clippy -p provider-control --all-targets -- -D warnings`；`cargo check -p provider-control --no-default-features`；独立 DeepSeek reviewer 复审通过
- Targeted regressions: 401 refresh-once、billing blocked 显式恢复、429 scope/Retry-After、默认 4xx fail-closed、5xx provider scope、cancel/client error 的 half-open probe 归还、并发 probe 上限、stale success 不清 cooldown、Provider 扩展 classifier 与无 Provider 名称分支
- Full workspace gate: NOT RUN（未命中升级条件）

**相关文档**：[providers](../docs/features/providers.md) · [provider-control-plane](../docs/features/provider-control-plane.md) · [P2-10](P2-10-retry-error.md) · [ROADMAP](../ROADMAP.md)
