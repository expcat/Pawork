# P19-3：Design System、Accessibility 与本地化

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1

**最终目的**：建立一套轻量、可访问、跨平台稳定的视觉与交互原语，使后续页面复用相同的布局、键盘、焦点、状态与文本规则，而不是在收尾阶段补 accessibility。

**涉及范围**：`apps/desktop/src/ui`、design tokens、主题、图标、i18n message catalog、component/visual harness

## 细分步骤

1. **Design tokens** —— 定义颜色、间距、字号、圆角、层级、动效与 Windows/macOS/Linux 字体 fallback。目的：主题和平台差异可控。
2. **应用布局原语** —— Activity Bar、Sidebar、Main、Inspector、Bottom Panel、Split Pane、Command/Toast/Modal/Popover。目的：保护 Timeline/Diff 主工作区。
3. **交互组件** —— Button/Input/Select/Tabs/Tree/List/Badge/Progress/Empty/Error/Skeleton，状态与 focus ring 一致。目的：减少页面自造组件。
4. **Accessibility** —— 全键盘导航、焦点陷阱/恢复、ARIA live region、对比度、缩放、reduced-motion。目的：P0 路径从第一天可用。
5. **本地化** —— zh-CN/en-US catalog、复数/日期/额度单位、长文本布局和缺失 key 检查。目的：避免业务组件硬编码文案。
6. **组件证据** —— renderer test + 固定 viewport/theme 的 visual baseline。目的：跨页面复用可审查。

## 主要产出物

- Design tokens、双主题与应用布局组件
- 可访问交互原语、zh-CN/en-US catalog
- Component gallery、键盘/a11y/visual tests

## 验收标准

- [ ] 基础组件在 light/dark、高对比、200% zoom 与 reduced-motion 下可用
- [ ] 所有交互原语可全键盘操作，焦点顺序和恢复有测试
- [ ] 状态不只依赖颜色；流式/通知使用适当 live region 且不轰炸读屏
- [ ] zh-CN/en-US 无硬编码业务文案，长文本不破坏主布局
- [ ] 稳定组件有人工确认的 visual baseline

**相关文档**：[Desktop GUI](../docs/features/desktop-gui.md) · [测试体系](../docs/quality/testing.md) · [P19-16](P19-16-desktop-gate.md)

**依赖建议（2026-08）**：优先原生语义 HTML + CSS variables + Pawork 自有组件；图标使用仓库 SVG，不为单一原语引入整套 UI framework。
