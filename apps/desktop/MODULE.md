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
  ui/{mod.rs, text_input.rs, theme.rs, timeline.rs, timeline_entry.rs, approval_card.rs, input_area.rs, inspector.rs, changes.rs, resources.rs, task_rail.rs}
  ui/components/{mod.rs, button.rs, dropdown.rs, follow_scroll.rs, label.rs, list_row.rs, panel.rs, status_bar.rs}
```

无 crate `tests/`；deny-list 断言在 `platform.rs`。

## 对外入口/API 面

手动 argv（非 clap）：`--socket`、`--instance`、`--probe`、`--probe-smoke`。默认窗口 1440×1024。

- **ui**：`AppView`、`TextInput`；动作 `ApproveOnce` / `ApproveForRun` / `Deny` / `CancelRun` / `Fork` / `NewTask` / `ToggleInspector`。Fork 只对 reducer 标记的闭合 run 边界开放，动作入口再次校验。
- **ui（R8 波 D 增）**：Inspector 顶层 Changes / Terminal / Resources 三页签（默认 Terminal；cmd-i 与 TerminalCreated 强制展开不变；各页签独立滚动）；`ui/changes.rs` = Files / Summary 二级页签 + DiffView + ActivityPopover（折叠态 StatusBar 触发器弹出，点击摘要展开并定位 Changes）；`ui/resources.rs` = MCP 只读列表。拉取时机：页签切入 / Inspector 展开 / 会话切换 / Run 终态 / 手动刷新。
- **projection**：`DesktopProjection`（`from_snapshot` / `apply_event` / `apply_resume_outcome`…）；**不** import gpui / tokio / OS。Timeline 条目类型来自 `pawork_client::projection`；同 session 切 branch 也必须清空 timeline/seen/tombstone/tool anchor 后建立新 baseline。
- **controller**：`DesktopController`（`connect` / `send_message` / `approve` / `fork_session` / `terminal_*`）。握手 `client_name = "pawork-desktop"`；能力 `Events`、`Snapshots`、`Approvals`、`TerminalStreaming`。
- **controller（R8 波 D 增）**：`diff_list_files` / `diff_get` / `mcp_list` 查询构造与响应解析（`DiffFileSummary` / `DiffFileDetail` / `McpServerEntry` 视图模型，epoch 防过期覆盖；`mcp_list` 响应形状钉为 `{"servers":[{name,transport,state,tools,last_error}]}`）。
- **platform**：`default_socket_path` / `token_path_for_instance` 等。数据目录镜像 app 的规则（`PAWORK_DATA_DIR` → 平台默认 → `~/.pawork`），**不**依赖 `pawork-app`。缺 `gui.token` 则失败，禁止无认证静默连接。

## 依赖与被依赖

- **生产 `pawork-*`**：恰好 `{pawork-client}`（`desktop_production_pawork_deps_stay_client_only`）。
- **UI**：`gpui = "=0.2.2"`、`smol`、`tokio`。
- **deny-list**（生产禁止）：`pawork-app` / `engine` / `providers` / `storage` / `tools` / `git` / `protocol` 以及其余 `pawork-*`。protocol/transport 类型只经 client re-export。
- **被依赖**：无。独立二进制。

## 红线与注意事项

- GUI 不得直接访问 Provider、数据库、工具、Git、PTY、quota store。
- 断线不取消进行中的 Run（`probe-smoke` 的 `disconnect_survive`）。
- 空闲保活（R8 波 E，2026-08-24）：host `heartbeat_timeout` 为 30s，任意入站帧刷新；controller 事件泵连续 15 tick（≈15s）无事件即发 `heartbeat()`（client 既有 API，io AsyncMutex 支持泵内并发调用），心跳失败走既有断线路径（不取消 Run）。
- 不宣告 `ArtifactStreaming`（K-08）。
- `ui/theme.rs` 已落地（R8 波 A：六组 25 色 token + 字阶 + metrics，深色单主题，静态 `dark()` 访问器；波 B 追加 4 个 hover token，共 29 色）。
- `ui/components/` 已落地（R8 波 B）：Button（variant Primary/Ghost/Danger/Success/Raised/Icon + hover/active）、Dropdown/MenuPanel/MenuRow（`deferred(anchored())` 浮层 + occlude 滚轮无穿透组件机制）、FollowScroll + BackToBottom、Label/Badge、Panel、StatusBar、ListRow。五组菜单（grouping/scope/model/entry fork/workspace confirm）已全部浮层化；Escape/外点关闭与 `Option<MenuKind>` 单开互斥为宿主（ui/mod.rs）接线。 ui/ 域渲染拆分（R8 波 C）：Timeline 经 gpui `list()` 变高虚拟化（`ListAlignment::Bottom` 钉底，跟随/回底语义不变，timeline.rs）；TimelineEntryView / ApprovalCard / InputArea / Inspector / TaskRail 拆分为独立模块，mod.rs 波 C 拆分至 824 行，波 D 三页签接线后 1031 行（R8 波 E 用户拍板接受为终态口径）；TaskRail 长标题 `.truncate()` 单行省略。
- Changes 面为只读（用户拍板 2026-08-24）：git_stage / HunkStageService 接线顺延 ADR 候选；`@` 补全浮层与「已加载规则」分区无 Host 出口，登记候选（`@` 端到端展开属 host 侧 crates/app，不在本 crate）。

## 相关文档

- [docs/gui-design.md](../../docs/gui-design.md)
- [design/README.md](../../design/README.md)
- [plan/R8-gui-components.md](../../plan/R8-gui-components.md)
- [crates/client/MODULE.md](../../crates/client/MODULE.md)
- [代码地图总索引](../../docs/code-map/README.md)
