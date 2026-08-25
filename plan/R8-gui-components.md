# R8 — GUI 组件化与 Desktop 收口（T12）

> 状态：🔵 **仅剩 K-03 人工走查签字**；波 A–E 自动化部分与整阶段审计已全部收口（2026-08-24~25，提交 528ab3d + 审计修复），交付细节、四项用户拍板（D1–D4）、审计发现与根因志均已存档于 [docs/history.md](../docs/history.md)「R8 阶段存档」。签字完成后本阶段转 🟢，本任务书随收口存档删除。
>
> 已交付概要：`ui/theme.rs` 30 色 token + 字阶/metrics；`ui/components/` 7 模块 11 组件；五组菜单 `deferred(anchored())` 浮层化；Timeline gpui `list()` 变高虚拟化；ui/ 拆分六模块（mod.rs ≈1031 行，D1 拍板口径；审计修复后实测 1034）；K-04 只读 Changes 面（Files/Summary/DiffView/ActivityPopover；git_stage/HunkStageService 接线顺延 ADR 候选）；K-06 Resources MCP 只读面 + 「@」host 端到端；desktop 空闲心跳修复（D3）。组件清单见 [docs/gui-design.md](../docs/gui-design.md) §9。

## 剩余工作：K-03 人工验收

- 验收清单与已取证证据：[docs/gui-design.md](../docs/gui-design.md) 附录 A（A.1 自动化已取证 7+1 项；A.2 人工走查十一项待签字；A.3 漂移定夺 D1–D4 已拍板；A.4 已收口免重复项）。
- 走查环境建议：1440×1024 对照 [design/](../design/README.md) 三图基准；长会话（千级事件）滚动与启动时间不回退。
- 已知缺口（验收时确认范围无新增，不作为签字阻塞）：菜单 ↑/↓ 导航与 grouping/scope 触发器 tab stop 未实现；窄窗响应式固定 288px（D2 拍板接受）；Entry 菜单滚动卸载短暂失联（D4 拍板接受）；DiffView 横滚无自动门禁。登记详情见 [ROADMAP.md](../ROADMAP.md) §4。

## 退出标准

- [x] theme + components 落地；`rgb()` 硬编码清零；mod.rs ≤1031 行（D1 拍板修订）；菜单 anchored/deferred
- [x] Timeline 虚拟化 + hover/active 全态；design 基准与实现一致
- [x] K-04 只读面 / K-06 交付（HunkStageService 与「@」补全 query 登记候选）
- [x] probe-smoke + desktop 41/41 + 整阶段审计收口
- [ ] **K-03 人工验收签字**（gui-design.md 附录 A.2 十一项 + 验收签字栏）
