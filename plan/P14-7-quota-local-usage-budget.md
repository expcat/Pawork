# P14-7：本地用量累计与预算联动

> Phase 14 · 模型用量与额度监控 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P14-6、P18-8、P3-6、P2-7

**最终目的**：用本地真实发生的 Usage（P2-9）累计每个绑定模型的实际消耗，与远端额度对照、推算「预计何时触及限制」，并与既有预算控制（P3-6）联动，让用户在远端额度刷新滞后时仍能看到接近实时的剩余估计。

**涉及范围**：`quota-service`；复用 `agent-events` 的 Usage 事件、`model-registry` 定价

## 细分步骤

1. **本地用量累计** —— 目的：消费 P18-8 `Usage/Cost Ledger`，按「tenant × account × 供应商 × 模型 × 窗口」累计 token 与费用；不另建第二套 usage 事实源。
2. **远端 / 本地对照** —— 目的：用远端 `QuotaOverview` 作为基线，叠加本次刷新后的本地增量，推算当前剩余（`confidence = derived`）。
3. **触限时间推算** —— 目的：结合近期消耗速率，估算 Overall / 5h / 周 / 月各窗口的预计耗尽时间，供预警使用。
4. **与预算控制联动** —— 目的：向 `agent-engine` 预算（P3-6）暴露额度水位，支持「接近限额时降级 / 暂停 / 切换 provider」的软阈值，事件不静默停。
5. **不确定性表达** —— 目的：推算结果带误差区间与置信度，UI 与预算据此决定动作力度。
6. **测试** —— 目的：用 Usage 事件流验证累计、对照与触限推算。

## 主要产出物

- 本地用量累计与远端对照
- 触限时间推算 + 预算联动接口
- 累计 / 推算测试

## 验收标准

- [x] 远端额度刷新后，本地增量被正确叠加
- [x] 各窗口触限时间可推算并带置信度
- [x] 预算控制可消费额度水位，触限动作有事件可追溯
- [x] ledger 与 quota snapshot 可按 tenant/account 对账，重放不重复累计

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [context（token 预算）](../docs/features/context.md) · [agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
