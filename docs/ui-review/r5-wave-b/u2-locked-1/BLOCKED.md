# u2-locked-1 — 基础设施失败轮（2026-08-28 19:39 +0800）

首轮 U2 九场景在窗口 probe 阶段超时：macOS 控制台处于锁定态（ioreg IOConsoleLocked=true），新窗口无法上屏、AX 树退化为嵌套 AXApplication、窗口尺寸回落 1412×1004。对照验证：同环境回退到 HEAD（fc863b6）构建，窗口行为完全相同 → 非 Wave B 代码回归。脚本随后在 probe 后卡住未退出（另登记脚本健壮性改进：probe 超时后应干净退出）。

解除方式：用户解锁屏幕后重跑 scripts/ui-r5-wave-b-states.sh。
