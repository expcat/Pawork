# P18-6：RoutingPolicy Chain

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢有界完成 · 交付成熟度：PartialWired（account route → lease 与 tenant policy 已进入 run；Health / model capability / credential 直传仍待后续） · 依赖：P18-2、P18-4、P18-5、P2-7、P15-8

**最终目的**：建立可组合、可解释、可确定性测试的路由策略链，支持 priority、weighted round robin、fill-first 与 fallback，而不把账号选择塞进 `ModelProvider` 或 Agent Engine。

**涉及范围**：`provider-control::routing`；`model-registry` / capability negotiation；route decision events

## 细分步骤

1. **候选与上下文** —— 定义 `RouteContext`、`RouteCandidate`、`RouteDecision`，携带 tenant/session/agent/capability/budget；目的：输入完整且不含 Secret。
2. **固定过滤链** —— capability → injected tenant policy → health → priority bucket；首轮可使用 `local/default` policy，完整 RBAC 接线由 P18-9 完成。目的：禁止低优先级穿透并为越权过滤保留强制入口。
3. **选择策略** —— 实现 `SingleCandidate`、round robin、smooth weighted round robin、fill-first；目的：覆盖单账号兼容与生产账号池。
4. **fallback chain** —— 显式区分 same credential / credential / model / provider / protocol；目的：每个动作可审计、可关闭。
5. **解释与属性测试** —— route decision 记录候选淘汰原因；proptest 验证 priority、权重分布和 deterministic seed；目的：可复核。

## 主要产出物

- `RoutingPolicy` 与 policy chain
- 四种基础策略 + 显式 fallback plan
- route explanation event 与 property tests

## 验收标准

- [x] 旧配置默认 `SingleCandidate`，行为与升级前一致
- [x] 高 priority 候选未耗尽时低 priority 不会被选中
- [x] weighted routing 分布在容差内且可用固定 seed 重放
- [x] route decision 不含明文 Secret，并可解释每个过滤/回退动作

## 验证记录（2026-08-13）

- Validation Level: L1
- Affected crates: `provider-control`
- Validated: `cargo test -p provider-control`（126 unit/property + 10 error-matrix）；`cargo clippy -p provider-control --all-targets -- -D warnings`；`cargo check -p provider-control --no-default-features`；`cargo fmt -p provider-control -- --check`；独立 DeepSeek reviewer PASS
- Targeted regressions: capability/policy/health/priority 固定链、Round-Robin/SWRR 确定性与权重周期、Fill-First 高优先级耗尽下沉、fallback fail-closed、HalfOpen 只读 plan 不耗探针、circuit 拒绝不误记 Healthy、每个淘汰动作可解释、无 Secret/provider-name 特例
- Full workspace gate: NOT RUN（未命中升级条件）

**相关文档**：[provider-control-plane](../docs/features/provider-control-plane.md) · [models](../docs/features/models.md) · [tenant-audit](../docs/features/tenant-audit.md) · [ROADMAP](../ROADMAP.md)
