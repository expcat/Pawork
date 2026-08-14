# P18-10：ClientAdapter Framework / Session Registry

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢有界完成 · 交付成熟度：HostWired（ACP 经 ClientAdapterHost；Codex / Claude 生产入口未装配） · 依赖：P18-1、P18-2、P18-9、P0-8、P13-1、P13-2

**最终目的**：为 Codex、Claude、ACP 等外部 Agent Client 提供统一协议适配契约、能力协商和 authoritative Session Registry，同时保持 GUI Connection Protocol 独立。

**涉及范围**：新增 `client-adapter-api`；`app-service` adapter host；`session-store` capability/ownership projection；`core-api`

## 细分步骤

1. **Adapter contract** —— 定义 `ClientAdapter` / `ClientAdapterFactory`、frame、canonical client/core event 与显式 `ProtocolUnsupported`；目的：客户端专有 JSON 不进 core。
2. **能力协商** —— protocol/client version、feature flags、capability snapshot 带 version 持久化；目的：禁止按 client 名假设能力。
3. **Session Registry** —— 保存 client/core session id、connection id、ownership epoch、revision、loaded/subscribed/executing；目的：共享磁盘不替代 ownership。
4. **Host/GUI 边界** —— adapter 经 `app-service` 进入同一 Core，不消费 GUI frame、不持有 Provider credential；目的：遵守唯一 Host/事实源。
5. **Mock adapter contracts** —— 覆盖 decode/encode、unknown field、unsupported capability、disconnect/reattach、stale owner；目的：给专属 adapter 统一基线。

## 主要产出物

- `client-adapter-api` + Mock adapter
- versioned capability snapshot / Session Registry
- app-service adapter host 与 contract tests

## 验收标准

- [x] adapter 只做协议翻译/身份提取/能力协商，不做业务或账号决策
- [x] 不支持能力显式失败或声明降级，不静默丢字段
- [x] ownership epoch/revision 冲突拒绝陈旧写入并可重同步
- [x] GUI、Codex、Claude、ACP channel 不互用协议 frame，且共享同一 Core

## 验证记录（2026-08-12）

- `cargo test -p client-adapter-api -p session-store -p app-service --all-targets` 的相关 adapter/registry 定向集通过：`client-adapter-api` 10、`session-store` 65、`app-service` adapter contract 6。
- `cargo clippy -p client-adapter-api -p session-store -p app-service --all-targets -- -D warnings`：通过。
- 验证覆盖 capability snapshot、真实 Core session 绑定、ownership epoch/revision CAS、disconnect/reattach 与 stale owner 拒绝。
- Validation Level：L1；Full workspace gate：NOT RUN（未命中升级条件）。

**相关文档**：[client-adapters](../docs/features/client-adapters.md) · [gui-connection](../docs/features/gui-connection.md) · [ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ROADMAP](../ROADMAP.md)
