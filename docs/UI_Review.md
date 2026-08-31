# Pawork Desktop UI Review

> 2026-08-31 重置。旧阶段截图、差分图和过程性结论已经清理；本文只记录当前真实主路径的审查结论，不保存新的仓库内截图。
>
> 初始视觉基准：[design/README.md](../design/README.md) · 当前计划：[ROADMAP.md](../ROADMAP.md) · 行为事实源：[gui-design.md](gui-design.md)

## 1. 当前健康度

| 路径 | 状态 | 当前证据口径 |
| --- | --- | --- |
| 正式构建与启动 | ✅ 已验证 | 独立 `desktop` 实例，正式 Host/Desktop，无 fixture、seed 或测试 profile |
| 添加项目 | ✅ 已验证 | Scope 菜单 `Add project…` → 系统目录选择器 → Host `workspace_add` → UI 显示真实目录名 |
| 对话与文件 | ✅ 已验证 | 新建 Task、发送真实 Provider 消息、审批 `write_file`，磁盘生成两行标记文件 |
| Git Changes | ✅ 已验证 | UI 显示 untracked、`+2 / −0` 与两行 diff，和仓库命令行事实一致 |
| Terminal | ✅ 已验证 | 受现有 Policy 闸约束创建真实 PTY，执行 `pwd` 与输出标记；纯文本展示过滤 ANSI/VT 控制序列 |
| 重启恢复 | ✅ 片 1 | schema v13 Session→Workspace 归属经真实 Host 重启恢复；Task/Timeline/Changes 保持，Terminal 进程不恢复但 workspace/cwd 诚实恢复，新 PTY 仍位于同一仓库 |

## 2. 设计与可用性结论

优势：三栏信息架构清晰；Scope、Task、Timeline、Changes 与 Terminal 的主路径均有原生可访问节点；空态与 unavailable 状态没有用假内容填充。真实文件写入后，Changes 能把文件状态、增删统计和正文 diff 放在同一面板内，核对成本低；重启重放也不再把 `resources.injected` 等信息诊断误显示为 Error。

当前风险：Terminal 是纯文本输出视图，不是完整 VT emulator；三张初始设计图尚未完成同状态人工视觉签字；完整 Terminal stop/close 与 live exit/failure 仍以 UI 本地诚实实现绕过（P3 边界）。Host 多项目注册表与按会话路由（P1 片 2B）及 Desktop 添加 / 切换 / 重开项目与新建 / 续聊会话五流程接线（片 2C 审计 + 真窗口验收）均已落地，P1 收口。以上未决项进入 [ROADMAP](../ROADMAP.md) 后续项。

Accessibility 口径：本轮真实路径已通过稳定 identifier 驱动项目、消息、Changes 与 Terminal。后续仍需完成 VoiceOver 阅读顺序、完整键盘回环和 100%/125%/150% 全状态人工验收。

## 3. 证据纪律

- 功能结论必须同时有真实窗口状态与源码外事实（文件、Git、Host 或 PTY 输出）。
- 当前运行截图只随当轮报告展示，不重新写入 `docs/ui-review/`。
- 仓库长期只保留 [design/README.md](../design/README.md) 列出的三张初始设计图。
- 旧 R/Wave 编号、截图路径和通过数量不再是当前状态事实；需要追溯时查看 Git 历史与 [history.md](history.md)。

## 4. 下一门禁

P1 片 2B 已完成 Host 项目注册表与按会话路由；下一步用正式 Desktop UI 验收添加 / 切换 / 重开项目及新建 / 续聊会话，之后才进入完整视觉与 Accessibility 签字。
