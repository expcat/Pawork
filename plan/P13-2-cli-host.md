# P13-2：CLI Host 装配与运行模式

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 依赖：P13-1（P13-3 并行交付）

**最终目的**：完成 CLI/Core 一体化装配——`core-runtime` 装配完整 Core，`cli-host` 将 Core、CLI 与 GUI Server 装配到同一进程，`pawork` 成为 Core 的唯一正式二进制。落地一次性/服务/交互/系统服务四种运行模式、Event Hub 与 CLI Renderer，使 CLI 既能独立运行，也能作为 GUI 宿主（[ADR-021](../docs/adr/ADR-021-cli-core-same-process.md)/[025](../docs/adr/ADR-025-cli-is-sole-host.md)/[026](../docs/adr/ADR-026-gui-disconnect-safe.md)）。

**涉及范围**：`core-runtime`、`cli-host`、`cli-command`、`cli-renderer`、`subscription-hub`、`apps/pawork`

## 细分步骤

1. **core-runtime 装配** —— 初始化 Core、打开数据库与 Artifact Store、启动 Agent Engine、装配各引擎。目的：完整 Core 运行时。
2. **cli-host 一体化装配** —— Core + CLI + GUI Server 同进程。目的：单一二进制宿主。
3. **Event Hub 与 CLI Renderer** —— Core Event 统一扇出到 CLI 渲染器与订阅中心；CLI 实时输出来自 Event Hub。目的：CLI 显示与 GUI 一致。
4. **四种运行模式** —— 一次性（`run`，可 `--serve`）、服务（`serve`）、交互（`shell`）、系统服务（`service install/start/stop`）。目的：覆盖部署形态。
5. **单/多实例与退出策略** —— 默认实例与命名实例；按模式判断退出条件。目的：可靠生命周期。

## 主要产出物

- `core-runtime` / `cli-host` 装配、`pawork` 运行模式、Event Hub + CLI Renderer

## 验收标准

- [x] `pawork serve` 可独立启动完整 Core（`serve --once` 有 e2e 测试）
- [x] CLI 可在无 GUI 时执行 Agent 任务（`run` 经 Event Hub 等待终态并流式输出）
- [x] Core 与 CLI 编译为同一个正式二进制（`apps/pawork` 装配 core-runtime + cli-host）
- [x] CLI 实时输出与 GUI 状态一致（run / watch 从 Event Hub 订阅同一全局序列事件流）
- [x] GUI 断线/关闭不迫使 CLI/Core 退出（run 任务由 supervisor 独立收敛；serve 的 GUI Server 装配位为 trait，断线语义见 ADR-026，P13-4 落地后验证）

## 落地记录（P13-2）

- `subscription-hub`：`EventHub::publish` 强制重写 `global_sequence`（AtomicU64 单调分配），ring buffer 保留最近 4096 条，`earliest_available` / `current` / `replay(from, to)`，tokio broadcast 有界订阅（慢消费者 Lagged）。
- `core-runtime`：`CoreRuntime` 装配 `AppService` + `EventHub` + `EventPump`（默认 10ms 轮询 `router.drain_events()` → `hub.publish`），`register_provider` 透传，`shutdown` 停止 pump；`from_parts` 注入 builder 保持 app-service 现有 API 与测试不变。
- `cli-host`：`execute` 统一经命令/查询信封路由；四种模式——`run`（workspace→session→run start→Hub 等待终态→流式输出，`--serve` 保持）、`serve`（等信号，GUI Server 装配位 `GuiServerHost` trait）、`shell`（REPL：/run /cancel /sessions /workspaces /approve /status /watch /connect）、`service`（Windows sc 注册 / systemd unit / launchd plist 模板，install 默认 dry-run）。
- `cli-command`：补 `run retry` 子命令；`service install/start/stop` 带 `--apply`（默认 dry-run）。
- `cli-renderer`：新增 `render_event(&AppEventEnvelope, OutputFormat)` 流式渲染。
- `apps/pawork`：装配 core-runtime + tracing（RUST_LOG）+ 信号 + 退出码；`tests/modes.rs` 覆盖 run / serve / shell / service 四模式。

**验证**：`cargo test -p subscription-hub -p core-runtime -p cli-host -p cli-command -p cli-renderer -p pawork`、`cargo fmt --all -- --check`、`cargo clippy -p <上述包> -- -D warnings` 全绿；Event Hub 全局序列连续（validate_after）与 replay 正确有单测覆盖。

**相关文档**：[CLI Host](../docs/features/cli-host.md) · [总体架构](../docs/architecture/overview.md) · [ADR-021](../docs/adr/ADR-021-cli-core-same-process.md) · [ADR-025](../docs/adr/ADR-025-cli-is-sole-host.md) · [ADR-026](../docs/adr/ADR-026-gui-disconnect-safe.md) · [ROADMAP](../ROADMAP.md)
