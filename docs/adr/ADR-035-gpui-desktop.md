# ADR-035：Desktop GUI 采用 GPUI 原生四层客户端

- **状态**：Accepted
- **日期**：2026-08-10
- **取代**：[ADR-034 Desktop GUI 独立协议客户端与可重建视图](ADR-034-desktop-gui-client-boundary.md) 中的 Tauri/React/WebView 技术选型；其独立进程、协议边界与可重建状态原则继续有效。

## 背景

Phase 19 尚未开始，仓库中没有已实现的 Tauri Desktop、React 页面、WebView bridge 或前端构建链。当前切换只需要修订设计与未来实现，不涉及既有用户数据或运行时迁移。

Pawork 已把 Desktop 定义为连接 `pawork` Host 的独立协议客户端。该边界与 UI framework 无关，因此可以移除 JavaScript/WebView bridge，同时保留 Core 单一事实源、GUI Connection Protocol、Snapshot/Event replay、revision 与 Policy 语义。

GPUI 仍是 pre-1.0 framework，Windows standalone、IME、accessibility、Terminal、打包和 updater 的成熟度不能从 Zed 的产品表现直接推定。采用 GPUI 必须先经过可退出的三平台硬 Gate。

## 决策

Phase 19 的目标实现改为纯 Rust GPUI Desktop，数据路径固定为：

```text
GPUI Views
    ↓
Desktop Projection / Controller
    ↓
gui-client → transport-api → gui-server → app-service → Core
```

`apps/desktop` 采用四层单向边界：

- `ui`：GPUI View/Element、主题与交互，只读取 projection；不得直接访问文件系统、进程、socket、HTTP 或协议 frame。
- `projection`：纯 Rust、可脱离 GPUI 测试的 Snapshot/Event 状态机；可整体丢弃并重建，不保存权威业务事实。
- `controller`：唯一业务 I/O 入口，只调用 `gui-client` 完成 Query、Command、订阅、重连和 Artifact 读取。
- `platform`：剪贴板、文件选择、通知、窗口、deep link、single-instance、更新与固定 Host bootstrap 的最小 allowlist facade；不暴露通用 shell/fs/http。

同时作出以下约束：

1. `apps/desktop` 不链接 `core-runtime`、`app-service`、数据库、Provider、Tool、Git 或 Sandbox 实现；依赖图和源码 deny rule 必须自动守卫该边界。
2. Desktop 的构建和运行链不引入 Node、Bun、V8、React、TypeScript renderer 或 WebView。TypeScript schema 生成继续服务非 Rust 客户端与契约校验。
3. P19-1 先执行三平台 GPUI PoC 硬 Gate，覆盖 Windows/macOS/Linux standalone、中文 IME、10k Timeline、100k Diff、Terminal、真实读屏、原生能力、签名打包和验签 updater。Windows、IME、Terminal、accessibility、signed updater 任一出现不可接受 blocker，即停止后续 GPUI 页面开发，重新评审本 ADR，并可恢复 ADR-034 记录的 Tauri 路线。
4. PoC 使用的 GPUI 候选必须锁定精确版本或 Git revision；禁止通配版本和跟随 `main`。通过三平台 Gate 后才把已验证 revision 固化为生产基线，并记录 Rust toolchain、系统依赖和已知平台限制。
5. 不从 Zed 复制 GPL UI 代码。GPUI 与所有组件、Terminal、Markdown、打包和更新依赖在采用前执行许可证、来源与维护性审计。
6. 不预设 GPUI 比 Tauri 更快、更省内存或包体更小。启动、RSS、GPU、Timeline、Diff、Terminal 和安装包指标均以 Pawork fixture 的可复现测量为准。
7. 打包与 updater 不在本 ADR 中指定具体工具；P19-15 根据 P19-1 Gate 证据选择并锁定平台工具链，必须覆盖签名、篡改拒绝、中断恢复和回退。

## 后果

- Desktop 与协议客户端统一为 Rust，消除 JavaScript↔Rust bridge 与 WebView/DOM 内容面。
- Pawork 需要自行拥有组件、accessibility、Terminal renderer、平台能力 facade、打包与更新工程，开发与维护成本高于原 Tauri 计划。
- Core、协议、数据库与业务事件格式不因 UI framework 改变；Gate 失败或未来回退不需要业务数据 migration。
- P19-2 之后的页面任务只有在 P19-1 硬 Gate 通过后才进入大规模实现；P19-16 仍以真实三平台证据作为 Phase 19 退出条件。

## 相关

- [Desktop GUI](../features/desktop-gui.md) · [GUI Connection Protocol](../architecture/api-surface.md) · [workspace 结构](../architecture/workspace-layout.md)
- [ADR-017 GUI 不直接访问底层](ADR-017-gui-no-direct-access.md) · [ADR-022 GUI 经 CLI 连接](ADR-022-gui-connects-via-cli.md) · [ADR-030 Core 单一事实源](ADR-030-core-sole-source-of-truth.md)
- [P19-1 GPUI Desktop Gate](../../plan/P19-1-desktop-shell.md) · [P19-15 发布链](../../plan/P19-15-packaging-updater.md) · [P19-16 Desktop Gate](../../plan/P19-16-desktop-gate.md)
