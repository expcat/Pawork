# 热点深描

跨包热路径。先读对应 crate 的 `MODULE.md`，再按需打开本目录。函数级细节以源码为准。

| 文档 | 路径 |
| --- | --- |
| [Agent loop](agent-loop.md) | `pawork-app` → `pawork-engine` → tools / policy |
| [GUI 连接](gui-connection.md) | desktop / headless → client → transport → protocol → `GuiServer` |
| [事件持久化与重放](event-persist.md) | domain 信封 → storage session → protocol projection |
| [凭证与脱敏](credentials.md) | auth locator → providers stream → `apps/pawork` Redactor |

不在本层展开的：R6 分支 lineage、R7 沙箱 profile、R8 GUI 组件库（阶段任务书见 `plan/R*.md`）。
