# Wave C AX 闸门取证笔记

日期：2026-08-26。helper：`scripts/ui-ax-dump.swift`（`swiftc -O` → `/tmp/pawork-r1wc-ax/ui-ax-dump`）。证据目录即本目录。

## 1. 链路（真实命令）

1. `swiftc -O -o /tmp/pawork-r1wc-ax/ui-ax-dump scripts/ui-ax-dump.swift` → COMPILE_OK。
2. `target/debug/examples/ui_fixture seed --root /tmp/pawork-ui-ax.P1HLu6` → `ui_fixture seed ok`（workspaces=3 sessions=7 events=263）。
3. 同一二进制 `serve --root …`，barrier `host_ready` 出现后 socket=`/tmp/pawork-ui-ax.P1HLu6/data/pawork-gui.sock`。
4. `PAWORK_DATA_DIR=<root>/data PAWORK_UI_BARRIER_DIR=<root>/barriers target/debug/pawork-desktop --socket <root>/data/pawork-gui.sock`。host log：`accepted connection connection-0`。
5. 轮询 helper `--pid <desktop>` 直到 CGWindowList 命中 owner PID；写出 `ax-tree.txt`，`screencapture -x -o -l 6956 window.png`。
6. 按 cmdline 精确杀掉自己启动的 desktop/host（SIGINT 后已退出），`rmtree` 仅 `/tmp/pawork-ui-ax.P1HLu6`（含 `.pawork-ui-fixture`）。

`cargo build -p pawork-desktop --offline` 尝试了两次（默认 incremental 与 `CARGO_INCREMENTAL=0`）。两次 rustc 在编译 `pawork_desktop` 后 CPU≈0%、无产物写出，约 1–4 分钟后中止以免占死全会话唯一 Cargo 槽。实际拉起的是已有 `target/debug/pawork-desktop`（mtime 2026-08-25 15:07:28；src 更新于 2026-08-26）。AX 结论绑定 gpui `=0.2.2` 窗口实现，不依赖本次未编进的 Desktop 源改动。

## 2. 窗口与截图

- Desktop PID 59768，CGWindowList：`wid=6956 owner="pawork-desktop" title="" layer=0 offscreen alpha=1.0 bounds={0,0,0,0}`。
- `screencapture -l 6956` 仍产出 `window.png`：PNG 1440×1056 RGBA，99880 bytes。画面含 TaskRail（Pawork / All projects / fixture 会话列表）、Composer（`mock / fixture-model`、Message Pawork、Cancel/Send）、Inspector（Changes/Terminal/Resources）、Terminal 面板与 traffic lights。可确认是真 Host+真 Desktop 的 Pawork 窗口，不是空壳。
- CG 报 0×0/offscreen 与截图像素不一致，属窗口列表元数据滞后；截图与 AX 的 `AXWindow` 同时存在，取证有效。

## 3. AX 树（`ax-tree.txt` 原文摘要）

helper 进程 `AXIsProcessTrusted()=true`。递归 dump role/subrole/title/value/identifier/description/actions，max-depth=12。

```
role=AXApplication title="pawork-desktop"
  role=AXWindow subrole=AXStandardWindow actions=[AXRaise]
    role=AXButton subrole=AXCloseButton actions=[AXPress]
    role=AXButton subrole=AXFullScreenButton ... actions=[AXPress,AXZoomWindow,AXShowMenu]
      role=AXGroup
    role=AXButton subrole=AXMinimizeButton actions=[AXPress]
    role=AXStaticText
```

summary：`nodes=7 truncated=0`；roles `AXApplication=1 AXButton=3 AXGroup=1 AXStaticText=1 AXWindow=1`；`identifiers (none)`。

未见：`AXTextField` / `AXTextArea`、`AXList`/`AXOutline`、`AXTabGroup`/`AXRadioButton`、自定义 `AXButton`（非 traffic lights）、任何 `identifier=`、Composer/TaskRail/Timeline/Inspector 的 title/value/description。唯一 custom_hint 是应用名 `title="pawork-desktop"`，不是控件语义。

## 4. 裁决

**FAIL（AX 闸门未过）。** 真窗口上的 Accessibility 树只有 `AXWindow` + 系统 traffic lights（Close / FullScreen / Minimize）+ 空 `AXStaticText`。Pawork 自定义控件（TaskRail 会话、`+`、Grouping、Composer 输入、Cancel/Send、Inspector tabs、Terminal）在 AX 中不存在 role/label/value/action/identifier 映射。

这与 `docs/UI_Review.md` §8.1 及 `plan/R1-ui-visual-contract.md` §4 的失败条件一致：不能把坐标驱动或截图差分宣称成 U2 语义定位通过。U3 截图管线本身可用（本目录 `window.png`），但不能替代 AX。

有界后续（需用户/ADR，本 spike 不改生产代码）：

1. 精确 revision 升级 gpui，使 AccessKit/macOS AX 暴露自定义 view；
2. 有限 backport 当前 0.2.2 的 AX/AccessKit 补丁；
3. 保持 GPUI 0.2.2，由 Desktop 侧实现原生等价 AX bridge（须先经 ADR 决策，且不得绕过既有 UI gate）。

对照：gpui-0.2.2 源码无 `accessibility` / `accesskit` / `AXUIElement` / `NSAccessibility` 命中；NSView 为自绘 layer，不把 GPUI element 注册进 AX。本取证是运行时确认，不是静态猜测。

