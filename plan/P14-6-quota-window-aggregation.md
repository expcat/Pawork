# P14-6：多窗口额度聚合与归一

> Phase 14 · 模型用量与额度监控 · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：单 provider × 凭据 scope 的聚合；全部绑定项聚合延 P18 binding enumeration） · 依赖：P14-1、P14-5

**最终目的**：把各适配器按窗口返回的零散快照聚合成「一个供应商 × 绑定凭据」的多窗口视图，统一重置倒计时口径，并维护本地缓存，使上层（预算、UI）拿到的是一致、可读的额度状态，而非各供应商原始片段。

**涉及范围**：`quota-service`

## 细分步骤

1. **多窗口聚合视图** —— 目的：定义 `QuotaOverview`，将 Overall / Rolling5h / Weekly / Monthly 的快照与本地推算（P14-7）合并到一个视图，缺失窗口标注 `Unknown`。
2. **重置倒计时归一** —— 目的：把供应商的绝对重置时间、相对倒计时、滚动窗口起始时间统一为「下次重置时间 + 剩余时长」，处理时区与滚动窗口边界不确定性。
3. **来源优先级** —— 目的：同一窗口多适配器可用时按 exact > derived > scraped 排序，并记录最终来源；scraped 数据不静默覆盖 exact。
4. **本地缓存** —— 目的：按「供应商 × 凭据 × 窗口」缓存快照与 TTL，避免 UI 频繁查询触发远程请求；缓存命中标注 `stale` 程度。
5. **并发获取与隔离** —— 目的：并发拉取多窗口时单适配器失败不影响其他窗口，失败信息汇总为部分可用状态。
6. **测试** —— 目的：用 Mock 适配器验证聚合、优先级、缓存命中与部分失败。

## 主要产出物

- `QuotaOverview` 聚合视图与缓存
- 重置倒计时归一与来源优先级逻辑
- 聚合单元测试

## 验收标准

- [x] 一个供应商的多窗口额度以统一视图呈现
- [x] 不同来源的同一窗口按可信度排序，不静默覆盖
- [x] 缓存生效，标注新鲜度；单窗口失败不影响整视图

**实现边界（2026-08-11 review-remediation）**：`QuotaOverview` 当前为单 provider scope（一个 provider × 绑定凭据）的多窗口聚合；app-service 查询要求显式 `provider_id`，未指定时返回 validation error，不再静默选择「首个已注册 provider」或默认 ID；「所有绑定 provider/model」的聚合语义待 P18 binding enumeration 成为事实源后批量查询。

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [ROADMAP](../ROADMAP.md)
