# pawork-desktop（apps/desktop）

独立 GPUI 进程：TaskRail + Timeline + Composer。业务依赖 **仅** `pawork-client`。

## 职责

本机单窗口 Agent 壳。经 GUI Connection Protocol 连接 `pawork gui serve`，不嵌入 Core、不直连 Provider / 数据库 / 工具。四层：`ui` 渲染、`projection` 纯状态、`controller` 调 client、`platform` 发现 socket/token 与 tokio runtime。

## 模块树

```
src/
  main.rs
  controller.rs
  projection.rs
  platform.rs
  ui/{mod.rs, text_input.rs}
```

无 crate `tests/`；deny-list 断言在 `platform.rs`。

## 对外入口/API 面

手动 argv（非 clap）：`--socket`、`--instance`、`--probe`、`--probe-smoke`。默认窗口 1440×1024。

- **ui**：`AppView`、`TextInput`；动作 `ApproveOnce` / `ApproveForRun` / `Deny` / `CancelRun` / `NewTask` / `ToggleInspector`。
- **projection**：`DesktopProjection`（`from_snapshot` / `apply_event` / `apply_resume_outcome`…）；**不** import gpui / tokio / OS。Timeline 条目类型来自 `pawork_client::projection`。
- **controller**：`DesktopController`（`connect` / `send_message` / `approve` / `fork_session` / `terminal_*`）。握手 `client_name = "pawork-desktop"`；能力 `Events`、`Snapshots`、`Approvals`、`TerminalStreaming`。
- **platform**：`default_socket_path` / `token_path_for_instance` 等。数据目录镜像 app 的规则（`PAWORK_DATA_DIR` → 平台默认 → `~/.pawork`），**不**依赖 `pawork-app`。缺 `gui.token` 则失败，禁止无认证静默连接。

## 依赖与被依赖

- **生产 `pawork-*`**：恰好 `{pawork-client}`（`desktop_production_pawork_deps_stay_client_only`）。
- **UI**：`gpui = "=0.2.2"`、`smol`、`tokio`。
- **deny-list**（生产禁止）：`pawork-app` / `engine` / `providers` / `storage` / `tools` / `git` / `protocol` 以及其余 `pawork-*`。protocol/transport 类型只经 client re-export。
- **被依赖**：无。独立二进制。

## 红线与注意事项

- GUI 不得直接访问 Provider、数据库、工具、Git、PTY、quota store。
- 断线不取消进行中的 Run（`probe-smoke` 的 `disconnect_survive`）。
- 不宣告 `ArtifactStreaming`（K-08）。
- R8 才会建 `ui/theme.rs` 与 `ui/components/`；当前 `ui/` 仍薄，不要把组件化写成已落地。
- Changes / `@` / Resources 面仍是 K-04 / K-06，未在本树。

## 相关文档

- [docs/gui-design.md](../../docs/gui-design.md)
- [design/README.md](../../design/README.md)
- [plan/R8-gui-components.md](../../plan/R8-gui-components.md)
- [crates/client/MODULE.md](../../crates/client/MODULE.md)
- [代码地图总索引](../../docs/code-map/README.md)
