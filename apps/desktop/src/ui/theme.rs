//! UI-1 / GUI3-08 共享深色工作台 token。
//! 近黑画布 + 更深侧栏拉开层次，高对比正文；蓝色只承担主动作 / 焦点。
//! 色板、字阶、圆角、间距与有限动效由这里统一提供。普通面板无阴影，
//! 仅浮层与 Composer 保留 elevation；运行时仍为单一 dark 主题。

use gpui::{rgb, rgba, Global, Rgba};

/// 七组颜色 token 宿主。
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
    /// Changes Diff 浅底（不复用 Allow / Deny 按钮色）。
    pub diff: DiffColors,
}

/// 未来主题挂载点（见模块文档）；当前仍由 dark() 按帧读取 palette。
impl Global for Theme {}

#[derive(Debug, Clone, Copy)]
pub struct BackgroundColors {
    /// #141416：根 / Workspace 背景。
    pub base: Rgba,
    /// #0e0e10：侧栏 / Inspector / 状态栏等面板背景。
    pub panel: Rgba,
    /// #1c1c1f：菜单 / 浮层背景与其未选中项。
    pub menu: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceColors {
    /// #222226：可用态控件面 / 选中面 / 输入框背景。
    pub raised: Rgba,
    /// #19191c：禁用态控件面。
    pub disabled: Rgba,
    /// #2c2c31：surface.raised 控件与选中行的 hover / active 背景。
    pub hover: Rgba,
    /// #18181b：raised 控件的 pressed 背景；比 hover 更沉，不靠缩放表达按下。
    pub pressed: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct BorderColors {
    /// #2a2a30：面板分隔线。
    pub subtle: Rgba,
    /// #3f3f46：浮层描边；亦作禁用态操作按钮背景（值 1:1，不拆分）。
    pub strong: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct TextColors {
    /// #f2f2f3：正文。
    pub primary: Rgba,
    /// #dedee2：面板内强调正文（项目名 / 终端输出 / Assistant 消息）。
    pub emphasis: Rgba,
    /// #aaaab2：次要文字（含工具调用名，收敛自 text.tool）。
    pub secondary: Rgba,
    /// #9b9ba4：辅助文字（时间 / 提示）。
    pub tertiary: Rgba,
    /// #8f8f8f：禁用态控件文字。
    pub disabled: Rgba,
    /// #5a5a5a：不可用 Fork 文字。
    pub ghost: Rgba,
    /// #ffffff：主色 / 危险按钮上的文字。
    pub on_accent: Rgba,
    /// #b8b8b8：审批卡详情。
    pub detail: Rgba,
    /// #9b9ba4（不透明）：composer 占位文字；适用于交互面。
    pub placeholder: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct AccentColors {
    /// #2f6fed：交互主色（选中 / 主按钮 / 焦点环）。
    pub primary: Rgba,
    /// #5b9dff55（RGBA）：输入框选区高亮。
    pub selection: Rgba,
    /// #3270e8：主按钮 hover / active 背景（满足白字对比度）。
    pub hover: Rgba,
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticColors {
    /// #3d7a4a：允许运行操作背景（Allow for run）。
    pub success_bg: Rgba,
    /// #438251：允许运行按钮 hover / active 背景（满足白字对比度）。
    pub success_hover: Rgba,
    /// #74c94c：状态点绿（running 状态前景）。
    pub success_fg: Rgba,
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

/// Diff 行专用浅底；不得与 semantic.success_bg / danger_bg 按钮色相同。
#[derive(Debug, Clone, Copy)]
pub struct DiffColors {
    /// 整行 addition 浅底。
    pub addition_bg: Rgba,
    /// 整行 deletion 浅底。
    pub deletion_bg: Rgba,
    /// 行号 gutter 文字（与 text.tertiary 同值，独立 token 防误用按钮色）。
    pub gutter_fg: Rgba,
}

/// 深色主题访问器。运行时使用 UI-1 单一 dark palette；
/// 不读取 macOS Increase Contrast，也不派生第二套可访问 palette。
pub fn dark() -> Theme {
    Theme {
        bg: BackgroundColors {
            base: rgb(0x141416),
            panel: rgb(0x0e0e10),
            menu: rgb(0x1c1c1f),
        },
        surface: SurfaceColors {
            raised: rgb(0x222226),
            disabled: rgb(0x19191c),
            hover: rgb(0x2c2c31),
            pressed: rgb(0x18181b),
        },
        border: BorderColors {
            subtle: rgb(0x2a2a30),
            strong: rgb(0x3f3f46),
        },
        text: TextColors {
            primary: rgb(0xf2f2f3),
            emphasis: rgb(0xdedee2),
            secondary: rgb(0xaaaab2),
            tertiary: rgb(0x9b9ba4),
            disabled: rgb(0x8f8f8f),
            ghost: rgb(0x5a5a5a),
            on_accent: rgb(0xffffff),
            detail: rgb(0xb8b8b8),
            placeholder: rgb(0x9b9ba4),
        },
        accent: AccentColors {
            primary: rgb(0x2f6fed),
            selection: rgba(0x5b9dff55),
            hover: rgb(0x3270e8),
        },
        semantic: SemanticColors {
            success_bg: rgb(0x3d7a4a),
            success_hover: rgb(0x438251),
            success_fg: rgb(0x74c94c),
            warning_text: rgb(0xf0d58c),
            warning_border: rgb(0x8a6d3b),
            warning_bg: rgb(0x2a2418),
            danger_text: rgb(0xf48771),
            danger_bg: rgb(0x8a3b32),
            danger_hover: rgb(0x9c463c),
        },
        diff: DiffColors {
            addition_bg: rgb(0x1b2a22),
            deletion_bg: rgb(0x2a1c1e),
            gutter_fg: rgb(0x9b9ba4),
        },
    }
}

/// 有限动效：面板开合 180ms、Switch 切换 120ms；hover / pressed / focus
/// 即时反馈。不循环闪动（流式 caret 为静态条，Reduce Motion 无需分支）。
pub mod motion {
    pub const PANEL_DURATION: std::time::Duration = std::time::Duration::from_millis(180);
    pub const CONTROL_DURATION: std::time::Duration = std::time::Duration::from_millis(120);

    pub fn ease_out_cubic(delta: f32) -> f32 {
        1.0 - (1.0 - delta.clamp(0.0, 1.0)).powi(3)
    }
}

/// 字阶以 16px 根字号的 rem 表达；100% 时与冻结 px 值逐项相等，窗口调整
/// `rem_size` 后统一变为 125% / 150%，几何 token 仍保持 px 不变。
pub mod font {
    use gpui::Rems;

    pub const BASE_REM_PIXELS: f32 = 16.0;

    pub const fn from_pixels(pixels: f32) -> Rems {
        Rems(pixels / BASE_REM_PIXELS)
    }

    #[cfg(test)]
    pub const fn default_pixels(size: Rems) -> f32 {
        size.0 * BASE_REM_PIXELS
    }

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub enum TextScale {
        #[default]
        Percent100,
        Percent125,
        Percent150,
    }

    impl TextScale {
        pub const fn percent(self) -> u16 {
            match self {
                Self::Percent100 => 100,
                Self::Percent125 => 125,
                Self::Percent150 => 150,
            }
        }

        pub const fn rem_pixels(self) -> f32 {
            BASE_REM_PIXELS * self.percent() as f32 / 100.0
        }

        pub const fn increase(self) -> Self {
            match self {
                Self::Percent100 => Self::Percent125,
                Self::Percent125 | Self::Percent150 => Self::Percent150,
            }
        }

        pub const fn decrease(self) -> Self {
            match self {
                Self::Percent150 => Self::Percent125,
                Self::Percent125 | Self::Percent100 => Self::Percent100,
            }
        }
    }

    /// 22px：Workspace Header 任务标题。
    pub const HEADER_TITLE: Rems = from_pixels(18.0);
    /// 12px：次级信息（相对时间 / 项目计数 / 连接行 / 时间戳）。
    pub const BODY_SM: Rems = from_pixels(12.0);
    /// 16px：Timeline 正文与 section title。
    pub const BODY: Rems = from_pixels(16.0);
    /// 20px：页面与应用标题。
    pub const TITLE: Rems = from_pixels(20.0);
    /// 20px：主操作图标字形（OPT-D 签字：可见图标 20–22px）。
    pub const ICON: Rems = from_pixels(20.0);
    /// 11px：提示 / 标签 / 次级行。
    pub const XS: Rems = from_pixels(11.0);
    /// 12px：meta 与紧凑辅助文字。
    pub const SM: Rems = from_pixels(12.0);
    /// 14px：控件、列表与输入框正文。
    pub const BASE: Rems = from_pixels(14.0);
    /// 等宽字体族（R8 波 D DiffView 代码区；本仓当前仅 macOS 构建）。
    pub const MONO: &str = "Menlo";
}

/// 间距 / 尺寸常量（px 数值；消费点经 gpui::px 转换）。
pub mod metrics {
    // ── Foundation 组件节奏（P0-1）──
    /// 基础间距只使用 4 / 8 / 12 / 16 / 24 / 32 六档。
    pub const SPACE_1: f32 = 4.0;
    pub const SPACE_2: f32 = 8.0;
    pub const SPACE_3: f32 = 12.0;
    pub const SPACE_4: f32 = 16.0;
    pub const SPACE_6: f32 = 24.0;
    pub const SPACE_8: f32 = 32.0;
    /// UI-1：6px，共享几何。
    pub const CONTROL_RADIUS: f32 = 6.0;
    pub const INPUT_MENU_RADIUS: f32 = 8.0;
    pub const SURFACE_RADIUS: f32 = 12.0;
    /// GUI3-08：Composer 卡片圆角（用户气泡仍走 SURFACE_RADIUS）。
    pub const COMPOSER_RADIUS: f32 = 16.0;
    /// GUI3-08：流式助手正文末尾静态插入条。
    pub const STREAM_CARET_WIDTH: f32 = 2.0;
    pub const STREAM_CARET_HEIGHT: f32 = 14.0;
    /// GUI3-08：Changes / Resources 加载骨架条高度。
    pub const SKELETON_BAR_HEIGHT: f32 = 8.0;
    /// 键盘焦点环宽度；组件 focus state 只改视觉，不缩放控件。
    pub const FOCUS_RING_WIDTH: f32 = 2.0;
    /// GUI2-06：供应商卡头单行高度（48–56px 合同取中档）。
    pub const SETTINGS_PROVIDER_HEADER_HEIGHT: f32 = 52.0;
    /// GUI2-06：Settings 导航分组标题行高（11–12px 字 + 留白）。
    pub const SETTINGS_NAV_GROUP_HEIGHT: f32 = 20.0;
    /// GUI2-06：Settings 查找输入固定高度（与 36px 动作命中区对齐）。
    pub const SETTINGS_SEARCH_INPUT_HEIGHT: f32 = 36.0;
    /// GUI2-06：审批 radio 外环直径。
    pub const SETTINGS_RADIO_OUTER: f32 = 16.0;
    /// GUI2-06：审批 radio 选中内实心直径。
    pub const SETTINGS_RADIO_INNER: f32 = 8.0;
    /// GUI2-06：查找定位行左侧 accent 标识宽度（覆盖层，不进盒模型）。
    pub const SETTINGS_LOCATE_MARK_WIDTH: f32 = 2.0;
    /// GUI2-06：供应商卡头状态色点。
    pub const SETTINGS_PROVIDER_STATUS_DOT: f32 = 8.0;
    /// 普通 icon button 命中区（OPT-D 签字：主操作命中区 ≥36×36）；
    /// Composer Send / Cancel 另有 36px 专用槽。
    pub const ICON_BUTTON_SIZE: f32 = 36.0;
    /// GUI3-05：可见 SVG 图标默认边长（命中区仍走 ICON_BUTTON_SIZE）。
    pub const ICON_SIZE: f32 = 20.0;
    /// GUI3-05：chip / 行内 / Settings 导航图标边长。
    pub const ICON_SM: f32 = 16.0;
    /// 通用菜单几何。
    pub const MENU_ANCHOR_GAP: f32 = SPACE_2;
    pub const MENU_MIN_WIDTH: f32 = 220.0;
    pub const MENU_MAX_WIDTH: f32 = 360.0;
    pub const MENU_ROW_HEIGHT: f32 = 34.0;
    pub const MENU_PADDING: f32 = SPACE_2;
    /// 1：「···」条目菜单触发器的水平内边距（R8 波 B）。
    pub const PADDING_XS: f32 = 1.0;
    /// 2：ghost 文本按钮（Inspector 开合 / Header 动作）的水平内边距（R8 波 B）。
    pub const PADDING_SM: f32 = 2.0;
    // ── TaskRail 几何（R3 Wave A，state-a/c 量图取档；render 与 AX 树共用单一来源）──
    /// 20：rail 内容左右统一 inset（量图 16–22 区间取值，标题 / scope / 右缘锚点统一）。
    pub const RAIL_CONTENT_INSET: f32 = 20.0;
    /// 12：rail 内层水平内边距（Panel p_2 帧 8 + 12 = 统一 inset 20）。
    pub const RAIL_INNER_PAD: f32 = 12.0;
    /// 36：rail 角标按钮边长（grouping / 全局与定向新建 / gear；
    /// OPT-D 签字：主操作命中区 ≥36×36）。
    pub const RAIL_ICON_BUTTON_SIZE: f32 = ICON_BUTTON_SIZE;
    /// 32：会话行改名 / 归档按钮边长（OPT-D 签字约束：Session 行动作
    /// hit area ≥32×32，行高 44 内垂直居中）。
    pub const RAIL_SESSION_ACTION_SIZE: f32 = 32.0;
    /// 10：rail 状态圆点直径（量图 Ø10–11）。
    pub const RAIL_STATUS_DOT_SIZE: f32 = 10.0;
    /// 36：顶部筛选 / 分组行高。
    pub const RAIL_TOP_ROW_HEIGHT: f32 = 36.0;
    /// 36：标题行高（量图 grouping 钮区 y49–84 = 36）。
    pub const RAIL_TITLE_ROW_HEIGHT: f32 = 36.0;
    /// 10：标题行 → scope 行纵向间距（内容顶 52 + 36 + 10 = scope 顶 98，
    /// 与量图 scope 盒 y98 精确对齐；文字到底 33 的读数是行底→文字带口径）。
    pub const RAIL_TITLE_SCOPE_GAP: f32 = 10.0;
    /// 18：筛选行（断线时 Reconnect）到列表首行的间距。
    pub const RAIL_LIST_TOP_GAP: f32 = 18.0;
    /// 24：日期桶头行高（GUI3-06：36 收为 24，12px medium secondary 不变）。
    pub const RAIL_BUCKET_HEADER_HEIGHT: f32 = 24.0;
    /// 20：日期桶头距上一组的纵向间距（量图「两桶头间距 380」反推 21，
    /// 4px 半格取 20；「距连接行底 42」是文字带口径不作行距）。
    pub const RAIL_BUCKET_TOP_GAP: f32 = 20.0;
    /// 2：日期桶头 → 桶内首个项目头（200+36+2 = 项目头顶 238 ±2 内）。
    pub const RAIL_BUCKET_TO_PROJECT_GAP: f32 = 2.0;
    /// 2：项目头 → 首个任务行（238+44+2 = 任务行顶 284，与量图精确对齐）。
    pub const RAIL_PROJECT_TO_TASK_GAP: f32 = 2.0;
    /// 44：任务行 / 项目头行高（量图 43–44 / 43–46 取 44）；100% / 125% 任务行与项目头。
    pub const RAIL_TASK_ROW_HEIGHT: f32 = 44.0;
    /// 36：150% 字号任务行（项目头仍 44，命中区与改名 / 归档 32×32 不变）。
    pub const RAIL_TASK_ROW_HEIGHT_COMPACT: f32 = 36.0;
    /// 16：标题截断槽右侧渐隐宽。
    pub const RAIL_TITLE_FADE_WIDTH: f32 = 16.0;
    /// Task 行 ListRow 水平 padding（与 `.px_2()` / 动作 `right(px(8))` 钉死 8px）。
    pub const RAIL_TASK_PAD_X: f32 = SPACE_2;

    /// 任务行高：仅 150% 顶档收为 36px，100% / 125% 保持 44。
    pub const fn rail_task_row_height(scale: super::font::TextScale) -> f32 {
        match scale {
            super::font::TextScale::Percent150 => RAIL_TASK_ROW_HEIGHT_COMPACT,
            _ => RAIL_TASK_ROW_HEIGHT,
        }
    }

    /// 标题槽相对行左缘的起点与宽度（状态点与尾槽占用同源；render / AX 共用）。
    pub fn rail_session_title_layout(
        row_width: f32,
        has_status_dot: bool,
        reserve_trailing: bool,
    ) -> (f32, f32) {
        let mut left = RAIL_TASK_PAD_X;
        if has_status_dot {
            left += RAIL_STATUS_DOT_SIZE + SPACE_2;
        }
        let right = if reserve_trailing {
            RAIL_TASK_PAD_X + SPACE_2 + RAIL_SESSION_ACTION_SIZE * 2.0
        } else {
            RAIL_TASK_PAD_X
        };
        (left, (row_width - left - right).max(0.0))
    }
    /// 8：项目块间距（量图 任务→下个项目头 52–54 − 行高 44）。
    pub const RAIL_PROJECT_BLOCK_GAP: f32 = 8.0;
    /// 56：项目计数 / 相对时间共用的右对齐元信息槽；100%/150% 均保留
    /// `now` / `244d` 与三位计数的稳定列，不让长标题挤占。
    pub const RAIL_META_SLOT_WIDTH: f32 = 56.0;
    /// UI-1：30px，共享几何。
    pub const STATUS_BAR_HEIGHT: f32 = 30.0;
    // ── Workspace Header / Timeline 几何（R4 Wave A，state-a §2.2/§2.3 与
    // state-b §2 量图取档；render 与 AX 树共用单一来源）──
    /// GUI2-01：Header 上下各 12px 内边距。
    pub const HEADER_SAFE_STRIP: f32 = 12.0;
    /// GUI2-01：Header 最小高度，内容可撑高。
    pub const HEADER_HEIGHT: f32 = 64.0;
    /// 28：Workspace 内容统一左 inset（标题 x328 / 首标签 x328 / 工具面板
    /// x326，相对 workspace 左缘 300，量图 26–28 取 28）。
    pub const TIMELINE_CONTENT_INSET: f32 = 28.0;
    /// UI-1：24px，共享几何。
    pub const HEADER_INSET_RIGHT: f32 = 24.0;
    /// UI-1：12px，共享几何。
    pub const HEADER_TITLE_META_GAP: f32 = 12.0;
    /// UI-1：6px，共享几何。
    pub const HEADER_STATUS_DOT_SIZE: f32 = 6.0;
    /// 40：Header 右侧动作按钮宽（量图 40×37）。
    pub const HEADER_ACTION_WIDTH: f32 = 40.0;
    /// 37：Header 右侧动作按钮高（量图 40×37）。
    pub const HEADER_ACTION_HEIGHT: f32 = 37.0;
    /// UI-1：6px，共享几何。
    pub const HEADER_ACTION_RADIUS: f32 = 6.0;
    /// 4：Header 右侧相邻动作槽（Activity 与 inspector-expand）的间距
    ///（OPT-4b；render 与 AX 几何同源）。
    pub const HEADER_ACTION_GAP: f32 = 4.0;
    /// GUI3-07：Header branch / live 状态 chip 圆角（Raised 底，4/6px 取 6）。
    pub const HEADER_CHIP_RADIUS: f32 = 6.0;
    /// GUI3-07：Header chip 水平内边距。
    pub const HEADER_CHIP_PAD_X: f32 = 6.0;
    /// GUI3-07：Header chip 垂直内边距。
    pub const HEADER_CHIP_PAD_Y: f32 = 4.0;
    /// UI-3：880px 居中阅读列，窄窗保留两侧 CONTENT_INSET。
    pub const TIMELINE_READABLE_WIDTH: f32 = 880.0;
    /// 28：Header 底到首条 Timeline 标签顶（state-a y104→132）。
    pub const TIMELINE_TOP_GAP: f32 = 28.0;
    /// UI-3：16px 正文、26px 行高。
    pub const MSG_LINE_HEIGHT: f32 = 26.0;
    /// UI-3：作者行与正文间距 12px。
    pub const MSG_LABEL_BODY_GAP: f32 = 12.0;
    /// UI-3：用户消息浅底卡片内边距。
    pub const MSG_USER_INSET_X: f32 = 16.0;
    pub const MSG_USER_INSET_Y: f32 = 12.0;
    /// UI-3：正文段落间隙 12px。
    pub const MSG_PARAGRAPH_GAP: f32 = 12.0;
    /// UI-3：32px 消息间距，计入虚拟列表条目高度。
    pub const MSG_ENTRY_GAP: f32 = 32.0;
    /// 8：Tool activity / Run summary 与 Composer 使用同一主要 surface 圆角。
    pub const TOOL_GROUP_RADIUS: f32 = 8.0;
    /// UI-3：轻量工具摘要命中行。
    pub const TOOL_GROUP_HEADER_HEIGHT: f32 = 36.0;
    /// 15：Tool activity 面板内左 inset（量图图标 x341，面板 x326）。
    pub const TOOL_GROUP_INNER_INSET: f32 = 15.0;
    /// 52：Tool 行高（量图行距 ≈54 − 分隔线 2）。
    pub const TOOL_ROW_HEIGHT: f32 = 52.0;
    /// UI-3：工具行分隔线 1px。
    pub const TOOL_ROW_DIVIDER: f32 = 1.0;
    /// 14：Tool 行状态 ✓ 直径（量图 Ø14）。
    pub const TOOL_CHECK_SIZE: f32 = 14.0;
    /// UI-3：上文到工具摘要的间距 16px。
    pub const TOOL_GROUP_TOP_GAP: f32 = 16.0;
    /// 12：Tool 面板 → Run 摘要卡间距（量图 13 取 12）。
    pub const SUMMARY_CARD_GAP: f32 = 12.0;
    /// 40：Run 摘要卡 ✓ 状态圆直径（量图 Ø40）。
    pub const SUMMARY_CHECK_CIRCLE: f32 = 40.0;
    /// 168：Run 摘要卡动作按钮宽（量图 168×40 / 168×39）。
    pub const SUMMARY_BUTTON_WIDTH: f32 = 168.0;
    /// 40：Run 摘要卡动作按钮高。
    pub const SUMMARY_BUTTON_HEIGHT: f32 = 40.0;
    /// 8：Run 摘要卡动作按钮圆角（量图 r≈8–10±2 取 8）。
    pub const SUMMARY_BUTTON_RADIUS: f32 = 8.0;
    /// 20：摘要卡两动作按钮间距（量图 19 取 20）。
    pub const SUMMARY_BUTTON_GAP: f32 = 20.0;
    /// UI-3：摘要卡底到 Timeline 页脚 12px。
    pub const TIMELINE_FOOTER_GAP: f32 = 12.0;
    /// 288：TaskRail 侧栏宽度。
    pub const SIDEBAR_WIDTH: f32 = 288.0;
    /// 440：Inspector 面板宽度。
    pub const INSPECTOR_WIDTH: f32 = 440.0;
    /// UI-4：输入卡片常态高 110px，不含外围留白与元信息。
    pub const COMPOSER_MIN_HEIGHT: f32 = 110.0;
    /// 220：Composer 面板增长上限（超限后输入内部滚动属 Wave B）。
    pub const COMPOSER_MAX_HEIGHT: f32 = 220.0;
    /// 8：Composer 输入行高计算的内边距余量（TextInput py_1 上下各 4）。
    pub const COMPOSER_TEXT_INSET: f32 = 8.0;
    /// UI-4：卡片内边距 16px，输入与动作间距 12px。
    pub const COMPOSER_PAD: f32 = 16.0;
    pub const COMPOSER_GAP: f32 = 12.0;
    /// 上下两条 1px 边框的总高度。
    pub const COMPOSER_BORDER: f32 = 2.0;
    /// 与 Timeline 同列；两侧至少 28px，顶部 16px、底部 24px。
    pub const COMPOSER_OUTER_X: f32 = 28.0;
    pub const COMPOSER_OUTER_TOP: f32 = 16.0;
    pub const COMPOSER_OUTER_BOTTOM: f32 = 24.0;
    pub const COMPOSER_META_GAP: f32 = 8.0;
    /// 以 100% 字号为基准；元信息随字号增长。
    pub const COMPOSER_META_HEIGHT: f32 = 24.0;
    pub const COMPOSER_MODEL_WIDTH: f32 = 220.0;
    /// 28：Composer 输入区单行最小高（行高 20 + py_1 上下 4+4）。
    pub const COMPOSER_INPUT_MIN_HEIGHT: f32 = 28.0;
    /// UI-4：模型与发送统一 36px 命中区。
    pub const COMPOSER_FOOTER_CONTROL: f32 = 36.0;
    /// 36：Composer Send / Cancel 同槽圆形按钮边长（OPT-D 命中区 ≥36×36）。
    pub const COMPOSER_SEND_SIZE: f32 = 36.0;
    /// COMPOSER_MIN_HEIGHT 的面板语义别名。
    pub const COMPOSER_PANEL_MIN_HEIGHT: f32 = COMPOSER_MIN_HEIGHT;
    /// 220：COMPOSER_MAX_HEIGHT 的面板语义别名。
    pub const COMPOSER_PANEL_MAX_HEIGHT: f32 = COMPOSER_MAX_HEIGHT;
    /// 1：IME marked range 下划线厚度。
    pub const UNDERLINE_THICKNESS: f32 = 1.0;
    /// 2：输入光标宽度。
    pub const CURSOR_WIDTH: f32 = 2.0;
    /// 1：滚动余量判定阈值。
    pub const SCROLL_EPSILON: f32 = 1.0;
    /// 16：贴底吸附阈值。
    pub const SCROLL_BOTTOM_SLACK: f32 = 16.0;
    /// 320：ActivityPopover 宽度（design/README.md §8.5）。
    pub const ACTIVITY_POPOVER_WIDTH: f32 = 320.0;
    /// 144：当前只有 Changes 摘要时按内容收缩，不为未实现的 Agent 状态留空。
    pub const ACTIVITY_POPOVER_HEIGHT: f32 = 144.0;
    /// UI-1 / GUI2-05：48px 面板头（名称选择器 + 关闭）。
    pub const INSPECTOR_TAB_HEIGHT: f32 = 48.0;
    /// 100：历史顶层页签槽宽；现仅作选择器最小宽度参考。
    pub const INSPECTOR_TAB_WIDTH: f32 = 100.0;
    /// 36：窄窗中央「返回对话」条高度。
    pub const INSPECTOR_BACK_HEIGHT: f32 = 36.0;
    /// UI-1：40px，共享几何。
    pub const CHANGES_TAB_HEIGHT: f32 = 40.0;
    /// 96：Changes 二级单个页签的固定命中宽度。
    pub const CHANGES_TAB_WIDTH: f32 = 96.0;
    /// 2：选中页签的 accent 下划线厚度。
    pub const TAB_UNDERLINE_HEIGHT: f32 = 2.0;
    /// 200：Changes · Files 文件清单的最大高度（超出滚动，下方留给 DiffView）。
    pub const CHANGES_FILE_LIST_MAX_HEIGHT: f32 = 200.0;
    /// 88：Changes · Summary 行标签列宽。
    pub const SUMMARY_LABEL_WIDTH: f32 = 88.0;
    /// 20：Changes 文件行状态色标槽。
    pub const CHANGES_FILE_GLYPH_WIDTH: f32 = 20.0;
    /// 32：Changes 文件清单紧凑行高（28–32 合同取 32）。
    pub const CHANGES_FILE_ROW_HEIGHT: f32 = 32.0;
    /// 72：Changes 文件状态文字固定右对齐槽（摘要 / 旧布局参考）。
    pub const CHANGES_FILE_STATUS_WIDTH: f32 = 72.0;
    /// 76：Changes 文件增删统计固定右对齐槽。
    pub const CHANGES_FILE_DELTA_WIDTH: f32 = 76.0;
    /// 32：Diff 左右行号列宽。
    pub const DIFF_LINE_NUMBER_WIDTH: f32 = 32.0;
    /// 16：Diff +/- 符号列。
    pub const DIFF_SIGN_WIDTH: f32 = 16.0;
    /// 24：历史单列 gutter 宽；现由行号列 + 符号列替代，保留常量供对照。
    pub const DIFF_GUTTER_WIDTH: f32 = 24.0;
    /// 36：Diff 当前文件只读 header 高度。
    pub const DIFF_HEADER_HEIGHT: f32 = 36.0;
    /// 0：行高保护性比较。
    pub const ZERO: f32 = 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 相对亮度（sRGB → 线性化 → 加权）。
    fn relative_luminance(color: Rgba) -> f64 {
        fn channel(value: f32) -> f64 {
            let value = f64::from(value);
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    /// WCAG 对比度（亮色在前，取值与参数顺序无关）。
    fn contrast_ratio(foreground: Rgba, background: Rgba) -> f64 {
        let foreground = relative_luminance(foreground);
        let background = relative_luminance(background);
        let (lighter, darker) = if foreground >= background {
            (foreground, background)
        } else {
            (background, foreground)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    fn assert_contrast_approx(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 0.05,
            "contrast {actual:.4} not within ±0.05 of {expected:.4}"
        );
    }

    /// UI-1：全部次要文字在实际最亮交互面上达到 AA。
    #[test]
    fn wcag_text_on_surface_pairs_meet_aa() {
        let theme = dark();
        for foreground in [
            theme.text.secondary,
            theme.text.tertiary,
            theme.text.placeholder,
        ] {
            for background in [
                theme.bg.base,
                theme.bg.panel,
                theme.bg.menu,
                theme.surface.raised,
                theme.surface.hover,
            ] {
                assert!(contrast_ratio(foreground, background) >= 4.5);
            }
        }
        assert_eq!(theme.text.placeholder.a, 1.0);
    }

    /// §2.1：白字（on_accent）× accent.hover ≈ 4.55、× success_hover ≈ 4.61。
    #[test]
    fn wcag_on_accent_over_hover_actions_match_frozen_targets() {
        let theme = dark();
        assert_contrast_approx(
            contrast_ratio(theme.text.on_accent, theme.accent.hover),
            4.55,
        );
        assert_contrast_approx(
            contrast_ratio(theme.text.on_accent, theme.semantic.success_hover),
            4.61,
        );
    }

    /// GUI3-08：近黑画布与更深侧栏拉开层次；有限动效不循环。
    #[test]
    fn gui3_08_surface_hierarchy_and_finite_motion_tokens() {
        let theme = dark();
        assert_eq!(theme.bg.base, rgb(0x141416));
        assert_eq!(theme.bg.panel, rgb(0x0e0e10));
        assert_eq!(theme.bg.menu, rgb(0x1c1c1f));
        assert_eq!(theme.surface.raised, rgb(0x222226));
        assert_eq!(theme.surface.hover, rgb(0x2c2c31));
        assert_eq!(theme.border.subtle, rgb(0x2a2a30));
        assert_eq!(motion::PANEL_DURATION, std::time::Duration::from_millis(180));
        assert_eq!(
            motion::CONTROL_DURATION,
            std::time::Duration::from_millis(120)
        );
        assert!((motion::ease_out_cubic(0.0) - 0.0).abs() < f32::EPSILON);
        assert!((motion::ease_out_cubic(1.0) - 1.0).abs() < f32::EPSILON);
        assert!(motion::ease_out_cubic(0.5) > 0.5);
    }

    /// §2.1：状态点绿槽位落地为不透明 #74c94c，不得回落到 success_bg。
    #[test]
    fn success_fg_is_opaque_status_dot_green() {
        let theme = dark();
        assert_eq!(theme.semantic.success_fg, rgb(0x74c94c));
    }

    #[test]
    fn text_scale_steps_and_foundation_tokens_match_accepted_tiers() {
        use font::TextScale;

        assert_eq!(TextScale::Percent100.decrease(), TextScale::Percent100);
        assert_eq!(TextScale::Percent100.increase(), TextScale::Percent125);
        assert_eq!(TextScale::Percent125.increase(), TextScale::Percent150);
        assert_eq!(TextScale::Percent150.increase(), TextScale::Percent150);
        assert_eq!(TextScale::Percent150.decrease(), TextScale::Percent125);
        assert_eq!(TextScale::Percent100.rem_pixels(), 16.0);
        assert_eq!(TextScale::Percent125.rem_pixels(), 20.0);
        assert_eq!(TextScale::Percent150.rem_pixels(), 24.0);
        assert_eq!(font::default_pixels(font::HEADER_TITLE), 18.0);
        assert_eq!(font::default_pixels(font::TITLE), 20.0);
        assert_eq!(font::default_pixels(font::BODY), 16.0);
        assert_eq!(font::default_pixels(font::BASE), 14.0);
        assert_eq!(font::default_pixels(font::BODY_SM), 12.0);
        assert_eq!(font::default_pixels(font::SM), 12.0);
        assert_eq!(font::default_pixels(font::XS), 11.0);
        assert_eq!(
            [
                metrics::SPACE_1,
                metrics::SPACE_2,
                metrics::SPACE_3,
                metrics::SPACE_4,
                metrics::SPACE_6,
                metrics::SPACE_8,
            ],
            [4.0, 8.0, 12.0, 16.0, 24.0, 32.0]
        );
        assert_eq!(metrics::CONTROL_RADIUS, 6.0);
        assert_eq!(metrics::INPUT_MENU_RADIUS, 8.0);
        assert_eq!(metrics::SURFACE_RADIUS, 12.0);
        assert_eq!(metrics::COMPOSER_RADIUS, 16.0);
        assert_eq!(metrics::STREAM_CARET_WIDTH, 2.0);
        assert_eq!(metrics::STREAM_CARET_HEIGHT, 14.0);
        assert_eq!(metrics::SKELETON_BAR_HEIGHT, 8.0);
        assert_eq!(metrics::FOCUS_RING_WIDTH, 2.0);
        assert_eq!(metrics::SETTINGS_PROVIDER_HEADER_HEIGHT, 52.0);
        assert_eq!(metrics::SETTINGS_NAV_GROUP_HEIGHT, 20.0);
        assert_eq!(metrics::SETTINGS_SEARCH_INPUT_HEIGHT, 36.0);
        assert_eq!(metrics::SETTINGS_RADIO_OUTER, 16.0);
        assert_eq!(metrics::SETTINGS_RADIO_INNER, 8.0);
        assert_eq!(metrics::SETTINGS_LOCATE_MARK_WIDTH, 2.0);
        assert_eq!(metrics::SETTINGS_PROVIDER_STATUS_DOT, 8.0);
        assert_eq!(metrics::ICON_BUTTON_SIZE, 36.0);
        assert_eq!(metrics::ICON_SIZE, 20.0);
        assert_eq!(metrics::ICON_SM, 16.0);
        assert_eq!(metrics::MENU_ANCHOR_GAP, 8.0);
        assert_eq!(metrics::MENU_MIN_WIDTH, 220.0);
        assert_eq!(metrics::MENU_MAX_WIDTH, 360.0);
        assert_eq!(metrics::MENU_ROW_HEIGHT, 34.0);
        assert_eq!(metrics::MENU_PADDING, 8.0);
    }

    /// TaskRail 几何继续冻结，字阶改用 P0 Foundation 层级。
    #[test]
    fn task_rail_geometry_and_font_constants_match_frozen_tiers() {
        assert_eq!(font::default_pixels(font::TITLE), 20.0);
        assert_eq!(font::default_pixels(font::BODY), 16.0);
        assert_eq!(font::default_pixels(font::BODY_SM), 12.0);
        assert_eq!(metrics::RAIL_CONTENT_INSET, 20.0);
        assert_eq!(metrics::RAIL_INNER_PAD, 12.0);
        assert_eq!(metrics::RAIL_ICON_BUTTON_SIZE, 36.0);
        assert_eq!(metrics::RAIL_STATUS_DOT_SIZE, 10.0);
        assert_eq!(metrics::RAIL_TOP_ROW_HEIGHT, 36.0);
        assert_eq!(metrics::RAIL_TITLE_ROW_HEIGHT, 36.0);
        assert_eq!(metrics::RAIL_TITLE_SCOPE_GAP, 10.0);
        assert_eq!(metrics::RAIL_LIST_TOP_GAP, 18.0);
        assert_eq!(metrics::RAIL_BUCKET_HEADER_HEIGHT, 24.0);
        assert_eq!(metrics::RAIL_BUCKET_TOP_GAP, 20.0);
        assert_eq!(metrics::RAIL_BUCKET_TO_PROJECT_GAP, 2.0);
        assert_eq!(metrics::RAIL_PROJECT_TO_TASK_GAP, 2.0);
        assert_eq!(metrics::RAIL_TASK_ROW_HEIGHT, 44.0);
        assert_eq!(metrics::RAIL_TASK_ROW_HEIGHT_COMPACT, 36.0);
        assert_eq!(
            metrics::rail_task_row_height(font::TextScale::Percent100),
            44.0
        );
        assert_eq!(
            metrics::rail_task_row_height(font::TextScale::Percent125),
            44.0
        );
        assert_eq!(
            metrics::rail_task_row_height(font::TextScale::Percent150),
            36.0
        );
        assert_eq!(metrics::RAIL_TITLE_FADE_WIDTH, 16.0);
        assert_eq!(
            metrics::rail_session_title_layout(248.0, false, true),
            (8.0, 160.0)
        );
        assert_eq!(metrics::RAIL_SESSION_ACTION_SIZE, 32.0);
        assert_eq!(metrics::RAIL_PROJECT_BLOCK_GAP, 8.0);
        assert_eq!(metrics::RAIL_META_SLOT_WIDTH, 56.0);
    }

    /// R4 Wave A Workspace Header / Timeline 几何合同（state-a §2.2/§2.3 与
    /// state-b §2 量图取档）：钉住数值防静默漂移。
    #[test]
    fn workspace_timeline_geometry_constants_match_frozen_tiers() {
        assert_eq!(font::default_pixels(font::HEADER_TITLE), 18.0);
        assert_eq!(
            font::default_pixels(font::from_pixels(metrics::MSG_LINE_HEIGHT)),
            26.0
        );
        assert_eq!(metrics::HEADER_SAFE_STRIP, 12.0);
        assert_eq!(metrics::HEADER_HEIGHT, 64.0);
        assert_eq!(metrics::TIMELINE_CONTENT_INSET, 28.0);
        assert_eq!(metrics::HEADER_INSET_RIGHT, 24.0);
        assert_eq!(metrics::HEADER_TITLE_META_GAP, 12.0);
        assert_eq!(metrics::HEADER_STATUS_DOT_SIZE, 6.0);
        assert_eq!(metrics::HEADER_ACTION_WIDTH, 40.0);
        assert_eq!(metrics::HEADER_ACTION_HEIGHT, 37.0);
        assert_eq!(metrics::HEADER_ACTION_RADIUS, 6.0);
        assert_eq!(metrics::HEADER_ACTION_GAP, 4.0);
        assert_eq!(metrics::HEADER_CHIP_RADIUS, 6.0);
        assert_eq!(metrics::HEADER_CHIP_PAD_X, 6.0);
        assert_eq!(metrics::HEADER_CHIP_PAD_Y, 4.0);
        assert_eq!(metrics::TIMELINE_READABLE_WIDTH, 880.0);
        assert_eq!(metrics::TIMELINE_TOP_GAP, 28.0);
        assert_eq!(metrics::MSG_LINE_HEIGHT, 26.0);
        assert_eq!(metrics::MSG_LABEL_BODY_GAP, 12.0);
        assert_eq!(metrics::MSG_PARAGRAPH_GAP, 12.0);
        assert_eq!(metrics::MSG_ENTRY_GAP, 32.0);
        assert_eq!(metrics::TOOL_GROUP_RADIUS, 8.0);
        assert_eq!(metrics::TOOL_GROUP_HEADER_HEIGHT, 36.0);
        assert_eq!(metrics::TOOL_GROUP_INNER_INSET, 15.0);
        assert_eq!(metrics::TOOL_ROW_HEIGHT, 52.0);
        assert_eq!(metrics::TOOL_ROW_DIVIDER, 1.0);
        assert_eq!(metrics::TOOL_CHECK_SIZE, 14.0);
        assert_eq!(metrics::TOOL_GROUP_TOP_GAP, 16.0);
        assert_eq!(metrics::SUMMARY_CARD_GAP, 12.0);
        assert_eq!(metrics::SUMMARY_CHECK_CIRCLE, 40.0);
        assert_eq!(metrics::SUMMARY_BUTTON_WIDTH, 168.0);
        assert_eq!(metrics::SUMMARY_BUTTON_HEIGHT, 40.0);
        assert_eq!(metrics::SUMMARY_BUTTON_RADIUS, 8.0);
        assert_eq!(metrics::SUMMARY_BUTTON_GAP, 20.0);
        assert_eq!(metrics::TIMELINE_FOOTER_GAP, 12.0);
    }

    /// GUI2-05：面板头 48、中央返回条 36、Changes 紧凑行 32、Diff 行号列。
    #[test]
    fn inspector_tabs_and_activity_popover_constants_match_frozen_tiers() {
        assert_eq!(metrics::INSPECTOR_TAB_HEIGHT, 48.0);
        assert_eq!(metrics::INSPECTOR_TAB_WIDTH, 100.0);
        assert_eq!(metrics::INSPECTOR_BACK_HEIGHT, 36.0);
        assert_eq!(metrics::CHANGES_TAB_HEIGHT, 40.0);
        assert_eq!(metrics::CHANGES_TAB_WIDTH, 96.0);
        assert_eq!(metrics::TAB_UNDERLINE_HEIGHT, 2.0);
        assert_eq!(metrics::ACTIVITY_POPOVER_WIDTH, 320.0);
        assert_eq!(metrics::ACTIVITY_POPOVER_HEIGHT, 144.0);
        assert_eq!(metrics::CHANGES_FILE_GLYPH_WIDTH, 20.0);
        assert_eq!(metrics::CHANGES_FILE_ROW_HEIGHT, 32.0);
        assert_eq!(metrics::CHANGES_FILE_STATUS_WIDTH, 72.0);
        assert_eq!(metrics::CHANGES_FILE_DELTA_WIDTH, 76.0);
        assert_eq!(metrics::DIFF_LINE_NUMBER_WIDTH, 32.0);
        assert_eq!(metrics::DIFF_SIGN_WIDTH, 16.0);
        assert_eq!(metrics::DIFF_GUTTER_WIDTH, 24.0);
        assert_eq!(metrics::DIFF_HEADER_HEIGHT, 36.0);
        assert_ne!(dark().diff.addition_bg, dark().semantic.success_bg);
        assert_ne!(dark().diff.deletion_bg, dark().semantic.danger_bg);
        assert_eq!(dark().diff.gutter_fg, dark().text.tertiary);
    }

    /// UI-4 卡片预算；外围元信息不抢占输入区增长上限。
    #[test]
    fn composer_geometry_constants_match_frozen_tiers() {
        let laid_out = metrics::COMPOSER_BORDER
            + metrics::COMPOSER_PAD * 2.0
            + metrics::COMPOSER_INPUT_MIN_HEIGHT
            + metrics::COMPOSER_GAP
            + metrics::COMPOSER_SEND_SIZE;
        assert_eq!(laid_out, metrics::COMPOSER_PANEL_MIN_HEIGHT);
        assert_eq!(laid_out, 110.0);
        assert_eq!(metrics::COMPOSER_PANEL_MAX_HEIGHT, 220.0);
        assert_eq!(
            metrics::COMPOSER_FOOTER_CONTROL,
            metrics::COMPOSER_SEND_SIZE
        );
        assert_eq!(metrics::COMPOSER_SEND_SIZE, 36.0);
        assert_eq!(metrics::COMPOSER_OUTER_X, 28.0);
        assert_eq!(metrics::COMPOSER_OUTER_BOTTOM, 24.0);
    }
}
