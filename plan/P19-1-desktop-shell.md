# P19-1：GPUI Desktop Gate 与安全壳

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P13-2、P13-4、P13-7、P13-9、P13-10

**最终目的**：先用可退出的 Windows/macOS/Linux PoC 证明 GPUI 能承担 Pawork Desktop 的关键路径，再建立纯 Rust 安全壳；硬 Gate 未通过前不进入后续页面的大规模实现。

**涉及范围**：`apps/desktop` 候选骨架、`gui-client` 公开接入面、GPUI 精确候选 revision、`ui/projection/controller/platform` 四层、三平台 PoC/打包/更新证据、[ADR-035](../docs/adr/ADR-035-gpui-desktop.md)

## 细分步骤

1. **纯 Rust 候选骨架** —— 建立 GPUI `Application`、基础窗口和 `ui/projection/controller/platform` 四层；PoC 候选使用精确版本或 Git revision，禁止通配和跟随 `main`。目的：得到可复现的真实 standalone 样本。
2. **最小协议闭环** —— 只经 `gui-client` 完成 connect/handshake/auth/snapshot/event/command/disconnect，配 Mock GUI Server；视图不接触原始 frame。目的：证明无需 JavaScript bridge 也不破坏既有协议边界。
3. **Host 与原生能力 facade** —— 连接命名 instance；无 Host 时仅允许固定 `pawork` binary + 固定参数 bootstrap；验证 clipboard/dialog/notification/window，所有能力经 `platform` allowlist。目的：以应用边界替代 framework capability manifest。
4. **关键 UI/输入 PoC** —— 在真实 Windows/macOS/Linux 验证中文/日文 IME、emoji、键盘布局、缩放、多显示器、10k Timeline、100k Diff、Terminal 的 ANSI/OSC/CJK/10 MB 输出。目的：优先暴露 GPUI 与 Rust 组件生态的高风险缺口。
5. **Accessibility Gate** —— 用 Windows Narrator、macOS VoiceOver、Linux Orca 验证可访问树、焦点、全键盘与状态通知节流；AccessKit 基础存在不视为通过。目的：以实际读屏证据决定可交付性。
6. **发布路径 PoC** —— 三平台生成可安装产物，并在 staging 证明 Windows/macOS 签名、macOS notarization、签名 updater、篡改拒绝与中断回退。目的：验证 GPUI 周边而非只验证窗口能打开。
7. **边界、许可证与可逆性守卫** —— 拒绝 Desktop 链接 Core 业务 crate 或直接使用任意 fs/process/socket/http；生成完整依赖许可证清单，不复制 Zed GPL UI 代码；确认移除 PoC 后 Core/Protocol 无 migration。目的：让安全和回退可自动复核。
8. **Go/No-Go 决策** —— 汇总版本、toolchain、平台、性能、IME、Terminal、a11y、打包/更新与已知限制。Windows、IME、Terminal、a11y、signed updater 任一存在不可接受 blocker 时停止 GPUI 后续任务并重新评审 ADR-035。目的：不把硬风险延期到发布前。

## 主要产出物

- `apps/desktop` 纯 Rust GPUI PoC、安全壳与四层目录边界
- 最小协议闭环、`DesktopPlatform` allowlist facade 与固定 Host bootstrap
- 三平台 IME/Terminal/a11y/performance/package/sign/update 证据和 Go/No-Go 记录
- 精确依赖/toolchain 基线、crate/source/license 守卫与可逆回退证明

## 验收标准

- [ ] Windows/macOS/Linux 的真实 standalone GPUI 壳可从 clean checkout 构建、启动并完成最小协议闭环
- [ ] Windows 中文 IME 与三平台键盘/缩放输入通过；Terminal、10k Timeline、100k Diff 达到 PoC 门槛并保留可复现数据
- [ ] Windows Narrator、macOS VoiceOver、Linux Orca 的 P0 路径通过，不以 AccessKit crate 存在或 headless test 代替
- [ ] 三平台安装产物可生成，Windows/macOS staging 签名与 notarization、签名更新篡改拒绝/中断回退通过
- [ ] Desktop 只依赖 `gui-client` 等协议客户端层，不链接 Core 业务 crate；`ui` 无直接 fs/process/socket/http 入口
- [ ] Desktop 与 `pawork` 的构建/运行均不需要 Node/Bun/V8，不存在 React/TypeScript/WebView renderer
- [ ] GUI 退出后已进入 Core 的 Run/Task 继续执行
- [ ] GPUI 与组件依赖使用精确 pin，许可证清单区分 GPUI 与 Zed 源码；移除 PoC 不要求 Core/Protocol/data migration
- [ ] 硬 Gate 失败时 P19-2～P19-15 保持未开始，并记录 ADR-035 的重新评审与 ADR-034 回退决定

**相关文档**：[Desktop GUI](../docs/features/desktop-gui.md) · [GUI 连接](../docs/features/gui-connection.md) · [ADR-035](../docs/adr/ADR-035-gpui-desktop.md) · [ADR-034（回退记录）](../docs/adr/ADR-034-desktop-gui-client-boundary.md)

**依赖建议（2026-08）**：不在计划中写未经验证的 GPUI 版本号。PoC 从精确候选版本/revision 开始，三平台通过后才固化生产基线；禁止 `*` 或 `main`。Terminal、组件、AccessKit 接线、打包与 updater 均先审计再选型，不因 Zed 已实现同类能力就假定 Pawork 可直接复用。
