# P13-1：app-service 完整化 + 统一 Command Source

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：Phase 3（P3-1~P3-10）、P0-8

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

- [ ] app-service 是 CLI 与 GUI 共享的唯一入口
- [ ] CLI 与 GUI 命令均记录来源与身份
- [ ] 网络重试不会重复创建 Run 或消息

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md) · [ADR-017](../docs/adr/ADR-017-gui-no-direct-access.md) · [ADR-024](../docs/adr/ADR-024-shared-app-service-event-hub.md) · [ROADMAP](../ROADMAP.md)
