# Settings 活动线（暂停）

> **暂停（2026-09-04）**：活动线是 [CLN 内部收敛](cleanup.md)。SET-0～SET-6g 已完成，过程已写入 [history.md](../docs/history.md)。本文件只保留尚未归档的 SET-7 缺口。产品规则仍以 [Settings Feature Spec](../docs/spec/settings.md) 为准。

## SET-7 — 真窗口与人工收口 ⚠️

未完成、不得用自动化冒充通过：

- 四家供应商可用认证路径；Kimi / xAI OAuth 的 device flow / refresh / 取消与浏览器切换。
- remote model list 成功路径（固定回退可见不算远端成功）。
- API key 替换失败保旧；Host 重启后默认项恢复；Connected → Disconnected / Connecting 全态。
- About 在断线 / 旧 Host 时隐藏；对照 `pawork doctor --json` 的数据目录与当前协商 API。
- 1440×1024、125%、Cmd+= / Cmd+- / Cmd+0、Tab / Enter、1080×720 长页滚动与折叠线以下 AX 几何。
- SET-3 已知缺口：Settings 页 AX 卡片几何为固定估值、不随滚动；1080×720 折叠线以下卡片 AX rect 与视觉错位。
- 人工签字：视觉层级、secure input、OAuth 浏览器切换、VoiceOver。

2026-09-04 Computer Use 已部分取证（Reconnect、Connected 的 Advanced/About、外观 150%→重启 100%、DeepSeek Set default / Remove）。完整记录见 [history.md](../docs/history.md) 同日条目。CLN 收口后由用户决定是否恢复本阶段。
