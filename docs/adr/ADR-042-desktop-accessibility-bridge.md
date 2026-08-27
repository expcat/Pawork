# ADR-042：Desktop 原生 Accessibility bridge（GPUI 0.2.2 补救）

- **状态**：Accepted（用户 2026-08-26 确认）
- **日期**：2026-08-26

## 背景

R1 Wave C 对真 fixture Host + 真 `pawork-desktop` 窗口做了 macOS Accessibility（AX）取证：截图中的 Pawork 三栏 UI 完整，但系统 AX 树只有 `AXApplication`、`AXWindow`、三个 traffic-light button 与空 `AXStaticText`，没有任何 Pawork 自定义控件的 role / label / value / action / identifier。证据见 [`docs/ui-review/wave-c/ax-gate/`](../ui-review/wave-c/ax-gate/)。源码复核也确认 crates.io `gpui = 0.2.2` 没有 AccessKit 或等价的元素级 AX 导出。

这使 R1 的 U2 真窗口语义驱动与 VoiceOver 可用性同时失败。任务书禁止把坐标点击伪装成语义定位，因此必须在三条补救路线中做出决策：

1. 升级到 Zed 当前 git revision：上游已把 GPUI 拆为多个未发布包，实际接近 vendor 整棵 GPUI，改动与验证面最大。
2. vendor 0.2.2 并有限 backport 上游 AX PR：目标 PR 是后续仍连续修复的中间态，同时改变 ADR-035 精确锁定产物。
3. 保持 GPUI 0.2.2，在 `pawork-desktop` 侧提供等价原生 AX bridge。

用户按推荐选择选项 3。

## 决策

### D1 — 保持 ADR-035，AX bridge 只属于 Desktop

- `gpui = 0.2.2` 精确锁定不动，不 patch crates.io、不修改 GPUI 源码。
- macOS bridge 由 `pawork-desktop` 自己承载：从 GPUI `Window` 的公开 raw-window-handle 取得 `GPUIView`，把 Pawork 虚拟 AX 元素树挂到该 `NSView`。
- bridge 只依赖 UI 当前已拥有的状态，不访问 Provider、数据库、工具或 Core；Desktop → client 的唯一业务依赖红线不变。
- 非 macOS 保留同形 no-op facade；本 ADR 不声称 Windows / Linux 已获得平台 AX 实现。

### D2 — 显式语义模型是唯一输入，不反推像素或 GPUI 私有树

- Desktop 定义平台无关的 `AxTree` / `AxNode`：稳定 `identifier`、role、可本地化 label、value / description、enabled / focused / selected、bounds、action 与 children。
- `identifier` 与用户可见 label 分离；自动化只依赖稳定 identifier / role，不依赖本地化文本或坐标。
- 语义树由 `AppView` 的 canonical UI 状态和布局 metric 显式构建；不得 OCR 截图、遍历 GPUI 私有 frame、复用调试 inspector 或根据 Provider 名称走特例。
- 初始冻结面覆盖窗口主要分区、TaskRail 会话、Timeline 可见内容、审批主路径、Composer、Inspector 页签与可见控制；后续新增可见交互时必须同批补 AX 节点。

### D3 — 原生对象、几何、焦点与通知遵循 AppKit 合同

- 虚拟控件用 `NSAccessibilityElement` 子类表达，设置 role / label / value / description / identifier / enabled / selected / focused、parent、screen frame 与 `accessibilityFrameInParentSpace`。
- `GPUIView` 作为 AX root/group，提供 children、hit-test 与 focused UI element；坐标由 GPUI 顶左原点转换为 AppKit parent/screen 坐标。
- 语义树变化发 layout-changed；焦点或值变化发对应 AX notification。无变化的 render 不重建原生树。
- 原生对象与 Rust action state 的 retain/release、dealloc、window close 清理必须有明确所有权；不得把 Secret 或任意协议 payload 复制进 AX label/value。

### D4 — AX action 回到既有 AppView 入口

- Press / focus / set-value 等 AX action 经 GPUI foreground executor 与目标 `WindowHandle<AppView>` 回到 UI 线程，再调用与鼠标/键盘相同的 `AppView` 入口和既有 enable gate。
- AX 不直调 controller，不绕过连接态、审批 fail-closed、run 进行中禁用、workspace 确认或 IME composing 规则。
- 原生回调只携带已登记的 typed action；未知 identifier / action fail-closed 返回未处理。

### D5 — 依赖与验证边界

- 仅 macOS target 增加与 GPUI 0.2.2 已使用版本对齐的 `objc` / `cocoa` 与 `raw-window-handle` 直接依赖；不新增 crate、不引入 JS runtime。
- 必须先有平台无关模型测试（唯一 identifier、层级、hit-test / focus、动态状态），再跑 Desktop 定向测试。
- 放行证据必须包含真窗口 `scripts/ui-ax-dump.swift` 输出：Pawork 自定义节点、稳定 identifier、role / label / value、主路径 `AXPress`，以及至少一次 AX action 驱动后的可观察状态变化。仅编译通过不算解除 AX 闸门。

## 后果

- 收益：保住 ADR-035 和当前构建闭包；U2 真窗口 driver 与 VoiceOver 共用同一语义源，不为测试另造坐标协议。
- 成本：Desktop 每个新增/重构的可见交互都要维护语义节点与 action 映射；布局 metric 变化须同步 AX bounds 测试。
- 风险：Objective-C runtime 与 Rust 生命周期是 unsafe 边界，限定在单一 macOS 模块并用最小公开 facade 隔离；真窗口取证作为回归门禁。
- 本 ADR 不改 GUI wire、事件信封、storage schema 或 `pawork-client` API。

## 回滚

删除 Desktop AX facade、macOS target 依赖与 AppView 接线即可回到取证前状态；无磁盘 / wire 迁移。但回滚会重新使 R1 U2 与 VoiceOver 闸门失败，不能作为完成态发布。

## 相关

- [R1 Wave C AX bridge 通过取证](../ui-review/wave-c/ax-bridge/notes.md)
- [Desktop 包级 Spec](../spec/crates/desktop.md)
- [架构事实源与 ADR 索引](../architecture.md)
- [AX 失败取证](../ui-review/wave-c/ax-gate/notes.md)
- [AX bridge 通过取证](../ui-review/wave-c/ax-bridge/notes.md)
