# ADR-034：Desktop GUI 是独立协议客户端与可重建视图

- **状态**：Superseded
- **取代说明**：2026-08-10 被 [ADR-035 GPUI 原生四层客户端](ADR-035-gpui-desktop.md) 取代——协议客户端与可重建视图的边界结论被继承，仅平台选型（Tauri/React/WebView）被取代。本文作为「可恢复回退」保留；若 [P19-1](../../plan/P19-1-desktop-shell.md) PoC 硬 Gate 失败，可按 ADR-035 可逆条款重新激活本文路径。
- **日期**：2026-08-09

## 背景

Phase 13 冻结 GUI Connection Protocol、Local/Remote Transport、Snapshot/Event Replay 与多 GUI 运行时，但真实 Desktop Client 尚未规划落地。若 Tauri GUI 直接链接 `core-runtime`、访问 SQLite/Provider/Tool，或把 renderer store 当成权威业务状态，会形成第二个 Core、绕过 Policy，并让本地与远程 GUI 产生两套行为。另一方面，纯 renderer 直接处理未校验 frame、通用系统 capability 与远程 HTML，也会把 WebView compromise 扩大为本机权限。

## 决策

Phase 19 在 `apps/desktop` 实现 Tauri 2 + React + TypeScript Desktop Client，并采用以下单向边界：

```text
React Renderer
    ↓ typed Tauri bridge（command/event）
apps/desktop/src-tauri
    ↓ gui-client
transport-api（Local / Remote）
    ↓
gui-server → app-service → Core
```

- `apps/desktop/src-tauri` 只依赖 `gui-client`、生成协议类型所需的最小 crate 与 Tauri 官方插件；不得依赖 `core-runtime`、`app-service`、数据库、Provider、Tool、Git 或 Sandbox 实现。
- React renderer 不直接打开 Unix Socket/Named Pipe、访问 Provider/API、数据库或 workspace 文件。所有业务 Query/Command/Event/Snapshot 经 Tauri typed bridge 与 GUI Connection Protocol 传递。
- Tauri bridge 负责 Transport、握手/认证、frame/version/size 校验、重连和 Artifact chunk 搬运；renderer 不接触原始 credential、Protected Blob 明文或未验证 frame。
- Desktop store 是 Snapshot/Event 的 materialized projection。它可缓存于内存，必要时保存非敏感恢复提示，但必须能完全丢弃并重建；业务事实、revision 与 ownership 只以 Core 为准。
- 所有写操作发送 `AppCommandEnvelope(command_id, idempotency_key, expected_revision, source, identity)`。允许视觉上的 pending 状态，但 Core 拒绝、事件回放或新 Snapshot 必须覆盖 optimistic 展示；安全动作默认不 optimistic commit。
- GUI 可发现并连接已有 `pawork` instance；若提供一键启动，只能调用专用、固定参数的 Host bootstrap，不暴露通用 shell，且 GUI 退出不得取消已进入 Core 的任务。
- Node/pnpm 仅用于 `apps/desktop` 构建、测试和 lockfile；`pawork` 二进制不嵌入 Node/Bun/V8，运行 Core 不要求安装前端工具链。
- Tauri capability 按窗口最小授权；默认不启用通用 shell/fs/http/sql。启用 dialog、clipboard、notification、updater 等能力时逐项限定 scope，并使用严格 CSP、无远程脚本、无 raw HTML。
- Markdown/Tool/Terminal 内容默认不可信：按文本或受限 AST 渲染，外链与图片 scheme allowlist，Artifact 通过协议读取，不通过任意本地路径加载。

## 多窗口与版本语义

每个窗口有独立 `GuiClientId`、订阅和焦点状态；同一 Desktop 进程可共享只读连接资源，但不能绕过 Core 的 revision/approval 竞争。协议版本不兼容、sequence gap 无法补齐或 identity 变化时停止敏感命令并请求 Snapshot/重新认证，不猜测状态。

## 后果

- Phase 19 可以在 Phase 13 稳定后开发 Shell、Projection 与主交互，同时用 Mock bridge 隔离尚未完成的后端能力。
- Desktop 需要独立的 Node lockfile、WebView 三平台测试、Tauri capability/CSP 审计、code signing/notarization 与 updater 签名流程；这些不进入 Rust Core 的普通 L1。
- GUI 本地离线编辑、直接 Provider 调用、直接 workspace 文件管理与“嵌入 Core”均不在首轮范围；需要离线能力时先新增 versioned protocol/ADR。

## 相关

- [Desktop GUI](../features/desktop-gui.md) · [GUI 连接](../features/gui-connection.md) · [GUI Connection Protocol](../architecture/api-surface.md)
- [ADR-017 GUI 不直接访问底层](ADR-017-gui-no-direct-access.md) · [ADR-022 GUI 经 CLI 连接](ADR-022-gui-connects-via-cli.md) · [ADR-026 GUI 断线安全](ADR-026-gui-disconnect-safe.md) · [ADR-030 Core 唯一权威](ADR-030-core-sole-source-of-truth.md)
- [ROADMAP Phase 19](../../ROADMAP.md) · [P19-16 Desktop Gate](../../plan/P19-16-desktop-gate.md)
