//! UI 主题 token（R8 波 A）。
//!
//! 深色单主题：全部颜色 token 的值与替换前的硬编码字面量逐值相等（视觉零变化），
//! 一个去重值对应一个字段，不合并、不改值。实态基线（2026-08-23）：mod.rs +
//! text_input.rs 共 92 处 rgb()/rgba() 颜色字面量、24 个去重值（任务书快照记 23
//! 值，快照遗漏 0x8a3b32，以实态为准）；连同审批按钮数组 3 个裸 u32 条目
//! （0x2f6fed/0x3d7a4a/0x8a3b32，原经 rgb(color) 间接消费）的全形式口径为 95 处
//! 消费点、25 个字段。
//!
//! 本波不做运行时切换；Global 实现仅是未来主题挂载点：计划在 main.rs 的
//! Application::run 闭包中 cx.set_global(theme::dark())（写入集外，本波不动）。
//!
//! R8 波 B（2026-08-24）追加 4 个 hover / active token（design/README.md §8.1）：
//! surface.hover / accent.hover / semantic.success_hover / semantic.danger_hover，
//! 字段数 25 → 29；active 复用 hover 色不另设 token。

use gpui::{rgb, rgba, Global, Rgba};

/// 六组颜色 token 宿主。
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// 大面积背景。
    pub bg: BackgroundColors,
    /// 组件面（按钮 / 输入框 / 选中面）。
    pub surface: SurfaceColors,
    /// 描边与分隔线。
    pub border: BorderColors,
    /// 文字色阶与内容角色色。
    pub text: TextColors,
    /// 交互主色与焦点 / 选区。
    pub accent: AccentColors,
    /// 警示 / 危险语义色。
    pub semantic: SemanticColors,
}

/// 未来主题挂载点（见模块文档）；波 A 仅静态 dark() 访问。
impl Global for Theme {}

#[derive(Debug, Clone, Copy)]
pub struct BackgroundColors {
    /// #1e1e1e：根背景。
    pub base: Rgba,
    /// #161616：侧栏 / Inspector / 状态栏等面板背景。
    pub panel: Rgba,
    /// #1a1a1a：菜单 / 浮层背景与其未选中项。
    pub menu: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceColors {
    /// #2a2a2a：可用态控件面 / 选中面 / 输入框背景。
    pub raised: Rgba,
    /// #242424：禁用态控件面。
    pub disabled: Rgba,
    /// #343434：surface.raised 控件与选中行的 hover / active 背景（R8 波 B）。
    pub hover: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct BorderColors {
    /// #2e2e2e：面板分隔线。
    pub subtle: Rgba,
    /// #3a3a3a：浮层描边；亦作禁用态操作按钮背景（值 1:1，不拆分）。
    pub strong: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct TextColors {
    /// #e8e8e8：正文。
    pub primary: Rgba,
    /// #c8c8c8：面板内强调正文（项目名 / 终端输出）。
    pub emphasis: Rgba,
    /// #9a9a9a：次要文字。
    pub secondary: Rgba,
    /// #7f7f7f：辅助文字（时间 / 提示）。
    pub tertiary: Rgba,
    /// #8f8f8f：禁用态控件文字。
    pub disabled: Rgba,
    /// #5a5a5a：不可用 Fork 文字。
    pub ghost: Rgba,
    /// #ffffff：主色 / 危险按钮上的文字。
    pub on_accent: Rgba,
    /// #d7d7ff：Assistant 消息。
    pub assistant: Rgba,
    /// #9cdcfe：工具调用名。
    pub tool: Rgba,
    /// #b8b8b8：审批卡详情。
    pub detail: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct AccentColors {
    /// #2f6fed：交互主色（选中 / 主按钮 / 焦点环）。
    pub primary: Rgba,
    /// #5b9dff55（RGBA）：输入框选区高亮。
    pub selection: Rgba,
    /// #3d7bf0：主按钮 hover / active 背景（R8 波 B）。
    pub hover: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticColors {
    /// #3d7a4a：允许运行操作背景（Allow for run）。
    pub success_bg: Rgba,
    /// #4a8c58：允许运行按钮 hover / active 背景（R8 波 B）。
    pub success_hover: Rgba,
    /// #f0d58c：警示文字。
    pub warning_text: Rgba,
    /// #8a6d3b：警示描边。
    pub warning_border: Rgba,
    /// #2a2418：警示背景。
    pub warning_bg: Rgba,
    /// #f48771：错误文字。
    pub danger_text: Rgba,
    /// #8a3b32：危险操作背景（Cancel）。
    pub danger_bg: Rgba,
    /// #9c463c：危险操作按钮 hover / active 背景（R8 波 B）。
    pub danger_hover: Rgba,
}

/// 静态深色主题访问器（波 A 单主题；值与替换前字面量逐值相等）。
pub fn dark() -> Theme {
    Theme {
        bg: BackgroundColors {
            base: rgb(0x1e1e1e),
            panel: rgb(0x161616),
            menu: rgb(0x1a1a1a),
        },
        surface: SurfaceColors {
            raised: rgb(0x2a2a2a),
            disabled: rgb(0x242424),
            hover: rgb(0x343434),
        },
        border: BorderColors {
            subtle: rgb(0x2e2e2e),
            strong: rgb(0x3a3a3a),
        },
        text: TextColors {
            primary: rgb(0xe8e8e8),
            emphasis: rgb(0xc8c8c8),
            secondary: rgb(0x9a9a9a),
            tertiary: rgb(0x7f7f7f),
            disabled: rgb(0x8f8f8f),
            ghost: rgb(0x5a5a5a),
            on_accent: rgb(0xffffff),
            assistant: rgb(0xd7d7ff),
            tool: rgb(0x9cdcfe),
            detail: rgb(0xb8b8b8),
        },
        accent: AccentColors {
            primary: rgb(0x2f6fed),
            selection: rgba(0x5b9dff55),
            hover: rgb(0x3d7bf0),
        },
        semantic: SemanticColors {
            success_bg: rgb(0x3d7a4a),
            success_hover: rgb(0x4a8c58),
            warning_text: rgb(0xf0d58c),
            warning_border: rgb(0x8a6d3b),
            warning_bg: rgb(0x2a2418),
            danger_text: rgb(0xf48771),
            danger_bg: rgb(0x8a3b32),
            danger_hover: rgb(0x9c463c),
        },
    }
}

/// 字阶（px 数值；Pixels 无法在本 crate 外 const 构造，消费点经 gpui::px 转换）。
pub mod font {
    /// 11px：提示 / 标签 / 次级行。
    pub const XS: f32 = 11.0;
    /// 12px：正文与控件。
    pub const SM: f32 = 12.0;
    /// 13px：标题与输入框正文。
    pub const BASE: f32 = 13.0;
}

/// 间距 / 尺寸常量（px 数值；消费点经 gpui::px 转换）。
pub mod metrics {
    /// 1：「···」条目菜单触发器的水平内边距（R8 波 B）。
    pub const PADDING_XS: f32 = 1.0;
    /// 2：ghost 文本按钮（Inspector 开合 / 状态栏开合）的水平内边距（R8 波 B）。
    pub const PADDING_SM: f32 = 2.0;
    /// 18：项目级新建按钮边长。
    pub const ICON_SMALL: f32 = 18.0;
    /// 22：全局新建 / 分组按钮边长。
    pub const ICON_MEDIUM: f32 = 22.0;
    /// 28：分组按钮宽度。
    pub const ICON_LARGE: f32 = 28.0;
    /// 24：底部状态栏高度。
    pub const STATUS_BAR_HEIGHT: f32 = 24.0;
    /// 288：TaskRail 侧栏宽度。
    pub const SIDEBAR_WIDTH: f32 = 288.0;
    /// 440：Inspector 面板宽度。
    pub const INSPECTOR_WIDTH: f32 = 440.0;
    /// 88：Composer 最小高度。
    pub const COMPOSER_MIN_HEIGHT: f32 = 88.0;
    /// 220：Composer 最大高度。
    pub const COMPOSER_MAX_HEIGHT: f32 = 220.0;
    /// 8：Composer 多行高度计算的内边距余量。
    pub const COMPOSER_TEXT_INSET: f32 = 8.0;
    /// 1：IME marked range 下划线厚度。
    pub const UNDERLINE_THICKNESS: f32 = 1.0;
    /// 2：输入光标宽度。
    pub const CURSOR_WIDTH: f32 = 2.0;
    /// 1：滚动余量判定阈值。
    pub const SCROLL_EPSILON: f32 = 1.0;
    /// 16：贴底吸附阈值。
    pub const SCROLL_BOTTOM_SLACK: f32 = 16.0;
    /// 0：行高保护性比较。
    pub const ZERO: f32 = 0.0;
}
