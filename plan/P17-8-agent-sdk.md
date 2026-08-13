# P17-8：Rust / JSON Agent SDK（Client 与 Headless）

> Phase 17 · Ecosystem & Host Compatibility · 状态：✅已实现（HostWired） · 交付成熟度：HostWired（历史代码交付≠产品验收） · 依赖：P0-8、P13-1

**最终目的**：在 `core-api` 之上提供 Rust Client SDK 与 Headless JSON 两条程序化接入。唯一运行 Core 的正式宿主仍是 `pawork` 二进制：脚本/CI 通过 `pawork headless --json-stdio` 收发 NDJSON，Rust 应用通过 SDK 启动或连接该模式，不把 Core 链接进第二个宿主。两者都复用稳定 Command/Query/Event 类型，也不取代 GUI Connection Protocol。

```text
Rust Application / IDE / CI
        → Agent SDK / Headless JSON adapter
        → pawork Host（唯一 Core 宿主）
        → app-service → Core
```

**涉及范围**：新增 `agent-sdk` client crate 与编译进 `cli-host` 的 `headless-json` 适配层；复用 `core-api`、`app-service`、`agent-events`、`cli-renderer`；不新增二进制、不提供 Core constructor、不动 `gui-protocol` / `gui-server`。

## 细分步骤

1. **Rust Client API 面** —— 目的：在 `agent-sdk` 暴露 `spawn_pawork`/`create_session`/`send`/`cancel`/`subscribe`/`fork`/`resume`，通过 `pawork headless` 的稳定 stdio framing 映射 AppCommand/AppQuery/AppEvent；应用只链接 client library，不能实例化 Core 或成为第二正式宿主。
2. **稳定 API 版本与兼容策略** —— 目的：为 Rust SDK 定义语义化版本与弃用策略，区分「稳定面」与「实验面」，确保 Core 内部重构不破坏已发布 SDK 调用方。
3. **Headless JSON 适配层** —— 目的：实现无头模式——进程读 NDJSON 请求（对应 `AppCommand`）、写 NDJSON 事件（对应 `AppEvent`），作为脚本/CI 的语言无关入口；帧定义来自 `core-api` 类型，不另造协议，输出编码复用 `cli-renderer` 既有 JSON 序列化。
4. **`pawork headless` 模式接线** —— 目的：在 `cli-host` 暴露无头 JSON 模式（与 `serve`/`shell`/`run` 并列），复用现有 `app-service` 装配，不修改 GUI 通道；无头模式不渲染 TUI/CLI 文本，只产 JSON 事件流。
5. **与 GUI Connection Protocol 边界隔离** —— 目的：明确「SDK/Headless 不是 GUI」——它们消费同一 `app-service`、同一 Event Hub，但走自己的接入语义；GUI 协议帧不向 SDK/Headless 泄漏，反之亦然。
6. **定向 / Mock 测试与示例** —— 目的：Rust SDK 用 Mock Provider 覆盖完整生命周期（建会话→发消息→收事件→取消→fork/resume）；Headless JSON 用端到端脚本断言「请求 JSON 进→事件 JSON 出」的往返；附最小 client 示例。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `agent-sdk` crate：稳定 Rust client API + `pawork` 进程生命周期 + 版本策略
- `headless-json` 适配层：NDJSON 请求/事件 + `pawork headless` 模式
- 定向测试（Rust 生命周期 / Headless 往返）+ 最小 client 示例

## 验收标准

- [x] Rust SDK 以稳定 typed API 驱动 `pawork headless`，不导出 Core constructor、不形成第二宿主二进制
- [x] Headless JSON 模式以 NDJSON 收发 `AppCommand`/`AppEvent`，无 TUI/CLI 文本输出
- [x] SDK 与 Headless 都建立在 `core-api` 之上，不取代 GUI Connection Protocol
- [x] SDK 有明确稳定/实验面划分与语义化版本策略
- [x] 定向 / Mock smoke 覆盖 Rust 生命周期与 Headless JSON 往返
- [x] **（P16-10 延期接线）compat 命令入口**：headless/SDK 经 `core-api`/CLI 暴露 `import_compat` 入口与导入历史查询（替代 `cli-host` 的 `placeholder_for_command`），导入产物经 `app-service` 持久化。P16-10 已在 library 层收敛 compat 内部正确性（单事务原子写、session-scoped ID、参数保真、import identity，见 [P16-10](P16-10-review-remediation.md)）；本任务只接命令入口，不重做存储语义。见 [p16-review §2.4/§3](../docs/review/p16-review.md) 与 [plan/README Phase 16 登记](README.md)。

## 验证记录（2026-08-12）

- `cargo test -p agent-sdk -p headless-json -p cli-host -p app-pawork --all-targets`：通过（含真实 `pawork headless` 进程往返与 capability/backpressure/request-id 回归）。
- `cargo clippy -p agent-sdk -p headless-json -p cli-host -p app-pawork --all-targets -- -D warnings`：通过。
- Validation Level：L1；Full workspace gate：NOT RUN（未命中升级条件）。

**相关文档**：[CLI Host](../docs/features/cli-host.md) · [GUI 连接与多客户端](../docs/features/gui-connection.md) · [P17-9 IDE Host Adapter](P17-9-ide-host-adapter.md) · [ADR-021 CLI 与 Core 同进程](../docs/adr/ADR-021-cli-core-same-process.md) · [ADR-030 Core 单一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [workspace-layout](../docs/architecture/workspace-layout.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：不新增第三方依赖；`headless-json` 是 `cli-host` 内部 adapter，`agent-sdk` 只依赖公开 schema/framing 与进程 client，不依赖 `core-runtime`。`pawork` 仍是唯一正式宿主，任何进程内 Core 嵌入提案须另走 ADR。
