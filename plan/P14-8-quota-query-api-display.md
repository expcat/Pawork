# P14-8：Quota 查询 API 与 CLI / GUI 展示

> Phase 14 · 模型用量与额度监控 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P14-6、P13-1

**最终目的**：把额度监控能力暴露为稳定查询接口，并在 CLI 与 GUI 上以脱敏、可读的方式展示「绑定模型用量 / 剩余额度 / 各窗口重置倒计时 / 数据来源」，让用户能直观掌握每个绑定模型的用量与限制情况。

**涉及范围**：`core-api`（Query 类型）、`app-service`、`cli-command` / `cli-renderer`、`gui-protocol`（随 P13-7 生成 TS）

## 细分步骤

1. **Query 类型定义** —— 目的：在 `core-api` 增加 `QuotaOverviewQuery`（按 provider / 凭据 / 窗口查询），返回 `QuotaOverview`；脱敏，不含明文凭据。
2. **app-service 接入** —— 目的：`app-service` 经 `quota-service` 服务该查询，记录命令来源（CLI/GUI），与既有 Command Router 一致。
3. **CLI 展示** —— 目的：`pawork usage`（或 `pawork quota`）子命令，文本 / JSON 输出每个绑定模型的多窗口额度、重置倒计时与来源标签。
4. **GUI 协议** —— 目的：`gui-protocol` 暴露查询与额度变更事件，供额度面板订阅刷新；随 P13-7 生成一致 TS 类型。
5. **来源与可信度可视化** —— 目的：UI 明确区分 exact / derived / scraped 与「需重新登录」「抓取失败」等状态，避免把推算 / 抓取数据呈现为精确事实。
6. **测试** —— 目的：契约测试覆盖 Query 往返、CLI 输出快照、GUI 协议生成类型一致。

## 主要产出物

- `QuotaOverviewQuery` 与 app-service 接入
- `pawork usage` 子命令
- GUI 协议查询 / 事件与 TS 生成
- 契约 / 快照测试

## 验收标准

- [x] CLI 与 GUI 能查询并展示各绑定模型的多窗口额度
- [x] 输出脱敏，不含明文凭据
- [x] 数据来源与可信度被清晰标注
- [x] 生成的 TS 类型与 Rust 一致

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [cli-host](../docs/features/cli-host.md) · [gui-connection](../docs/features/gui-connection.md) · [ROADMAP](../ROADMAP.md)
