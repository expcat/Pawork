# P13-1：app-service 完整化 + 统一 Command Source

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：Phase 3（P3-1~P3-10）、P0-8

**最终目的**：完整化 app-service（全部 Command/Query/Event、状态聚合、事件限流、任务监督、错误转换），并落地统一 Command Source：CLI 与 GUI 命令进入同一 Command Router，统一转换为 `AppCommandEnvelope`（command_id/source/identity/idempotency_key），保证可追溯来源与去重。app-service 是 CLI 与 GUI 共享的唯一稳定入口（[ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md)/[017](../docs/adr/ADR-017-gui-no-direct-access.md)/[024](../docs/adr/ADR-024-shared-app-service-event-hub.md)）。

**涉及范围**：`app-service`、`core-api`

## 细分步骤

1. **补全 Command/Query/Event** —— 目的：覆盖 CLI 与 GUI 全部需求。
2. **统一 Command Router 与来源记录** —— 目的：CLI/GUI 命令同路径、可追溯、可去重。
3. **状态聚合与事件限流** —— 目的：客户端不被淹没。
4. **任务监督与错误转换** —— 目的：Run 稳健、错误友好。
5. **唯一入口校验** —— 目的：CLI/GUI 不绕过 app-service。

## 主要产出物

- 完整 `app-service` + Command Router + `AppCommandEnvelope`/`CommandSource` 落地

## 验收标准

- [x] app-service 是 CLI 与 GUI 共享的唯一入口
- [x] CLI 与 GUI 命令均记录来源与身份
- [x] 网络重试不会重复创建 Run 或消息

## 实现记录（2026-08-10）

- `CommandRouter`（dispatch / dispatch_query）作为 CLI 与 GUI 的唯一命令/查询
  入口：`AppCommandEnvelope`（command_id / source / identity / idempotency_key /
  expected_revision）统一路由，`CommandSource` 区分 LocalCli / LocalGui /
  RemoteGui，来源与身份计数记录在册。
- `IdempotencyStore`：同 command_id / idempotency_key 重放返回首次响应，错误
  响应不缓存；`RateLimiter`（窗口 + 缓冲）与 `RunSupervisor`（真实
  ProviderLoop，并发上限、取消、失败释放 lease）；`ApprovalRegistry` 集中
  审批；`AggregateState` 聚合 workspace / session / run / approval / provider /
  artifact / gui_client / terminal。
- 所有错误统一转为 `core_api::ErrorContext` 并包装为 `AppResponse::Error`，
  CLI 与 GUI 看到同一错误协议，不泄漏 Secret。
- 测试：38 项（app-service 单测 + router_integration / run_lifecycle 集成），
  覆盖来源记录、幂等重放、限流、Run 生命周期与审批。

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md) · [ADR-017](../docs/adr/ADR-017-gui-no-direct-access.md) · [ADR-024](../docs/adr/ADR-024-shared-app-service-event-hub.md) · [ROADMAP](../ROADMAP.md)
