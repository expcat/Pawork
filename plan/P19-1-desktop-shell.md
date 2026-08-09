# P19-1：Desktop Shell 与安全边界

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P13-2、P13-4、P13-7、P13-9、P13-10

**最终目的**：建立可在 Windows/macOS/Linux 启动的 Tauri + React Desktop Shell，并把“独立协议客户端、最小系统权限、Node 不进入 Host”变成可自动检查的物理边界，为后续所有 GUI 页面提供安全宿主。

**涉及范围**：`apps/desktop`、`gui-client` 的公开接入面、前端 lockfile、Desktop 构建/测试脚本、[ADR-034](../docs/adr/ADR-034-desktop-gui-client-boundary.md)

## 细分步骤

1. **Tauri/React/Vite 骨架** —— 建立 `src-tauri`、TypeScript renderer、开发/构建入口与 pinned Node/package-manager 配置。目的：三平台使用同一可复现壳。
2. **Typed bridge 边界** —— Tauri backend 只包装 `gui-client` 的 connect/disconnect/query/command/event，不暴露任意 invoke。目的：防止 renderer 绕过 GUI Connection Protocol。
3. **Host 发现与启动** —— 连接命名 instance；无 Host 时仅允许固定 `pawork` binary + 固定 serve/service 参数的专用 bootstrap。目的：提供开箱体验而不引入通用 shell。
4. **Capability/CSP** —— 默认拒绝 shell/fs/http/sql，按窗口最小启用 dialog/clipboard/notification/updater；禁止远程脚本与 raw HTML。目的：收敛 WebView compromise 的影响面。
5. **边界守卫** —— 加依赖/manifest 检查，拒绝 desktop 链接 core-runtime/app-service/Provider/Tool/Git，拒绝 `pawork` 引入 Node 运行时。目的：让架构红线可持续验证。
6. **三平台 shell smoke** —— 验证冷启动、窗口恢复、连接页、退出不取消 Host 任务。目的：证明独立生命周期。

## 主要产出物

- `apps/desktop` Tauri + React + TypeScript + Vite 骨架与 lockfile
- 最小 Tauri capabilities/CSP、typed bridge 与 Host bootstrap
- crate/package 边界守卫和三平台 shell smoke

## 验收标准

- [ ] Windows/macOS/Linux 可构建并打开 Connection Shell
- [ ] Desktop 只依赖 `gui-client` 等协议客户端层，不链接 Core 业务 crate
- [ ] 通用 shell/fs/http/sql capability 默认不可用，CSP 不允许远程脚本/raw HTML
- [ ] Node 仅是 `apps/desktop` 构建依赖，运行 `pawork` 不需要 Node/Bun/V8
- [ ] GUI 退出后已进入 Core 的 Run/Task 继续执行
- [ ] L1：renderer build/typecheck + Tauri mock/shell smoke 通过

**相关文档**：[Desktop GUI](../docs/features/desktop-gui.md) · [GUI 连接](../docs/features/gui-connection.md) · [ADR-034](../docs/adr/ADR-034-desktop-gui-client-boundary.md)

**依赖建议（2026-08）**：采用 Tauri 2、React、TypeScript、Vite 与官方最小插件；开工时锁定兼容小版本。参考 [Tauri Frontend](https://v2.tauri.app/start/frontend/) 与 [Capabilities](https://v2.tauri.app/security/capabilities/)。
