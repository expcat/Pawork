# P19-3：Design System、Accessibility 与本地化

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-1

**最终目的**：建立一套轻量、可访问、跨平台稳定的 GPUI 组件与主题原语，使后续页面复用相同的布局、键盘、焦点、状态与文本规则，而不是在收尾阶段补 accessibility。

**涉及范围**：`apps/desktop/ui` 的 GPUI 组件/主题模块、design tokens、图标、i18n message catalog、component/visual harness

## 细分步骤

1. **Design tokens** —— 以 Rust token 结构定义颜色、间距、字号、圆角、层级、动效与 Windows/macOS/Linux 字体 fallback，由所 pin GPUI UI 层统一消费。目的：主题和平台差异可控。
2. **应用布局原语** —— Activity Bar、Sidebar、Main、Inspector、Bottom Panel、Split Pane、Command/Toast/Modal/Popover 的 GPUI Element/组件。目的：保护 Timeline/Diff 主工作区。
3. **交互组件** —— Button/Input/Select/Tabs/Tree/List/Badge/Progress/Empty/Error/Skeleton，统一 focus handle、keybinding 与状态样式。目的：减少页面自造组件。
4. **Accessibility** —— 经所 pin GPUI/AccessKit 路径暴露平台可访问树，并在 Windows UI Automation、macOS NSAccessibility、Linux AT-SPI 上实测；覆盖全键盘导航、焦点陷阱/恢复、状态通知节流、对比度、缩放、reduced-motion。目的：P0 路径从第一天可用。
5. **本地化** —— zh-CN/en-US catalog、复数/日期/额度单位、长文本布局和缺失 key 检查。目的：避免业务组件硬编码文案。
6. **组件证据** —— GPUI render harness 测试 + 固定 viewport/theme 的 visual baseline。目的：跨页面复用可审查。

## 主要产出物

- Rust design tokens、双主题与应用布局组件（GPUI）
- 可访问交互原语（经三平台实测的可访问树）、zh-CN/en-US catalog
- Component gallery、键盘/a11y/visual tests

## 验收标准

- [ ] 基础组件在 light/dark、高对比、200% zoom 与 reduced-motion 下可用
- [ ] 所有交互原语可全键盘操作，焦点顺序和恢复有测试
- [ ] 状态不只依赖颜色；流式/通知通过可访问状态语义通知且节流，不轰炸读屏
- [ ] zh-CN/en-US 无硬编码业务文案，长文本不破坏主布局
- [ ] 稳定组件有人工确认的 visual baseline

**相关文档**：[Desktop GUI](../docs/features/desktop-gui.md) · [测试体系](../docs/quality/testing.md) · [P19-16](P19-16-desktop-gate.md)

**依赖建议（2026-08）**：UI 原语以 GPUI 为唯一框架自建（Entity/Element/action/keybinding）；所 pin GPUI/AccessKit 路径必须经三平台真实读屏验证，不能从依赖存在推定完整 accessibility；图标使用仓库 SVG/矢量绘制，不为单一原语引入第二套 UI 或样式框架。
