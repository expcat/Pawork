//! R2 Wave A 轨 2：窗口壳布局合同（F-01/F-02 / 响应式 / U1 不变量）。
//!
//! 合同（design/README.md §2）：宽窗三栏 TaskRail 288 / Workspace 弹性 /
//! Inspector 440，StatusBar 24；窗口宽 1080–1279 时 rail 收敛 240、
//! Inspector 折叠为 ActivityPopover 抽屉、Workspace ≥560。resolve 是
//! AppView::render 与本模块 #[gpui::test] 共享的唯一计算入口；探针主机
//! 复用生产 Panel / StatusBar 组件装配同构壳层（参照 u1_probe.rs，不挂
//! AppView / Platform / socket）。

use gpui::{div, prelude::*, px, Context, IntoElement, Pixels, Render, Window};

use super::components::panel::Panel;
use super::components::status_bar::StatusBar;
use super::theme::metrics;

/// 窄窗（1080–1279）TaskRail 宽度。
pub(crate) const RAIL_NARROW_WIDTH: f32 = 240.0;
/// 窄窗带宽上界（含）：内容宽 ≤1279 视为窄窗。
pub(crate) const NARROW_WIDTH_MAX: f32 = 1279.0;
/// 中央 Workspace 底线宽度。
pub(crate) const WORKSPACE_MIN_WIDTH: f32 = 560.0;
/// TaskRail 顶部 traffic-light 安全区高度：透明 titlebar 下按钮悬浮于
/// rail 左上（量图中心 ≈(25.5,23.5)），该带内不得出现交互控件。
pub(crate) const TRAFFIC_LIGHT_SAFE_HEIGHT: f32 = 36.0;

/// 一次 render 的壳层几何决定。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShellLayout {
    /// TaskRail 宽度：宽窗 = metrics::SIDEBAR_WIDTH（288），窄窗 = 240。
    pub(crate) rail_width: f32,
    /// Inspector 是否作为 440px 右栏参与布局。
    pub(crate) inspector_open: bool,
}

/// 由窗口内容宽度与用户 Inspector 偏好解析壳层几何。
///
/// 窄窗强制折叠 Inspector：1080–1279 合同要求默认折叠且 Workspace ≥560，
/// 而 240+440+560=1240 只覆盖带宽尾部，展开必然击穿底线；偏好值不在
/// resize 时改写，窗口加宽后按偏好自动恢复。
pub(crate) fn resolve(window_width: Pixels, inspector_preferred: bool) -> ShellLayout {
    let narrow = window_width <= px(NARROW_WIDTH_MAX);
    ShellLayout {
        rail_width: if narrow {
            RAIL_NARROW_WIDTH
        } else {
            metrics::SIDEBAR_WIDTH
        },
        inspector_open: !narrow && inspector_preferred,
    }
}

/// TaskRail 顶部 traffic-light 安全区：36px 无交互占位（F-01）。
pub(crate) fn rail_safe_area() -> impl IntoElement {
    div()
        .id("shell-rail-safe-area")
        .debug_selector(|| "shell-rail-safe-area".into())
        .h(px(TRAFFIC_LIGHT_SAFE_HEIGHT))
        .flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{size, Bounds, TestAppContext, VisualTestContext};

    /// 与 AppView::render 同构的壳层探针：resolve 决定几何，rail /
    /// workspace / inspector / StatusBar 用生产组件装配。
    struct ShellProbeHost {
        inspector_preferred: bool,
    }

    impl Render for ShellProbeHost {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let layout = resolve(window.viewport_size().width, self.inspector_preferred);
            let rail = div()
                .id("shell-rail")
                .debug_selector(|| "shell-rail".into())
                .flex()
                .child(Panel::side_right(px(layout.rail_width)).child(rail_safe_area()));
            let workspace = div()
                .id("shell-workspace")
                .debug_selector(|| "shell-workspace".into())
                .flex()
                .flex_col()
                .flex_1();
            let mut main = div().flex().flex_row().flex_1().child(workspace);
            if layout.inspector_open {
                main = main.child(
                    div()
                        .id("shell-inspector")
                        .debug_selector(|| "shell-inspector".into())
                        .flex()
                        .child(Panel::side_left(px(metrics::INSPECTOR_WIDTH))),
                );
            }
            div()
                .flex()
                .size_full()
                .child(rail)
                .child(div().flex().flex_col().flex_1().child(main).child(StatusBar::new()))
        }
    }

    fn mount_shell(cx: &mut TestAppContext, width: f32, height: f32) -> &mut VisualTestContext {
        let (_host, cx) =
            cx.add_window_view(|_window, _cx| ShellProbeHost { inspector_preferred: true });
        cx.simulate_resize(size(px(width), px(height)));
        cx.refresh().expect("refresh after mount");
        cx.run_until_parked();
        cx
    }

    fn assert_within(bounds: Bounds<Pixels>, width: f32, height: f32, label: &str) {
        assert!(
            bounds.origin.x >= px(0.) && bounds.origin.y >= px(0.),
            "{label} origin {bounds:?} escapes window origin"
        );
        assert!(
            bounds.origin.x + bounds.size.width <= px(width)
                && bounds.origin.y + bounds.size.height <= px(height),
            "{label} extent {bounds:?} escapes {width}x{height}"
        );
    }

    #[test]
    fn resolve_switches_rail_width_at_1280() {
        let narrow = resolve(px(1279.), true);
        assert_eq!(narrow.rail_width, RAIL_NARROW_WIDTH);
        assert!(!narrow.inspector_open, "narrow band must collapse inspector");

        let wide = resolve(px(1280.), true);
        assert_eq!(wide.rail_width, metrics::SIDEBAR_WIDTH);
        assert!(wide.inspector_open);

        assert!(!resolve(px(1440.), false).inspector_open);
        assert_eq!(resolve(px(1080.), true).rail_width, RAIL_NARROW_WIDTH);
    }

    #[gpui::test]
    fn wide_shell_matches_1440_contract(cx: &mut TestAppContext) {
        let cx = mount_shell(cx, 1440., 1024.);
        let rail = cx.debug_bounds("shell-rail").expect("rail bounds");
        let safe = cx
            .debug_bounds("shell-rail-safe-area")
            .expect("safe area bounds");
        let workspace = cx.debug_bounds("shell-workspace").expect("workspace bounds");
        let inspector = cx
            .debug_bounds("shell-inspector")
            .expect("inspector bounds");
        let status = cx
            .debug_bounds("shell-status-bar")
            .expect("status bar bounds");

        // F-02：TaskRail 288 全高贯通（透明 titlebar 下内容视口 = 1440×1024）。
        assert_eq!((rail.origin.x, rail.origin.y), (px(0.), px(0.)));
        assert_eq!(rail.size.width, px(metrics::SIDEBAR_WIDTH));
        assert_eq!(rail.size.height, px(1024.));
        // F-01：rail 顶部 ≥36px traffic-light 安全区（Panel p_2 上边距 8px）。
        assert_eq!(safe.origin.y, px(8.));
        assert_eq!(safe.size.height, px(TRAFFIC_LIGHT_SAFE_HEIGHT));

        // 分隔线：rail|workspace、workspace|inspector 严格邻接（1px 描边在
        // 所属面板盒内），无缝隙、无重叠、无双线。
        assert_eq!(workspace.origin.x, rail.origin.x + rail.size.width);
        assert_eq!(
            workspace.size.width,
            px(1440. - metrics::SIDEBAR_WIDTH - metrics::INSPECTOR_WIDTH)
        );
        assert_eq!(inspector.origin.x, workspace.origin.x + workspace.size.width);
        assert_eq!(inspector.size.width, px(metrics::INSPECTOR_WIDTH));
        assert_eq!(inspector.origin.x + inspector.size.width, px(1440.));
        assert_eq!(
            inspector.size.height,
            px(1024. - metrics::STATUS_BAR_HEIGHT)
        );

        // StatusBar 24px 连续横贯 workspace+inspector，不覆盖左栏账户区。
        assert_eq!(status.origin.x, rail.size.width);
        assert_eq!(status.size.width, px(1440. - metrics::SIDEBAR_WIDTH));
        assert_eq!(status.origin.y, px(1024. - metrics::STATUS_BAR_HEIGHT));
        assert_eq!(status.size.height, px(metrics::STATUS_BAR_HEIGHT));

        for (label, bounds) in [
            ("rail", rail),
            ("workspace", workspace),
            ("inspector", inspector),
            ("status", status),
        ] {
            assert_within(bounds, 1440., 1024., label);
        }
    }

    #[gpui::test]
    fn narrow_shell_collapses_inspector_at_1080(cx: &mut TestAppContext) {
        let cx = mount_shell(cx, 1080., 720.);
        let rail = cx.debug_bounds("shell-rail").expect("rail bounds");
        let workspace = cx.debug_bounds("shell-workspace").expect("workspace bounds");
        let status = cx
            .debug_bounds("shell-status-bar")
            .expect("status bar bounds");

        // 窄窗合同：rail 240；Inspector 默认折叠（偏好 true 也不参与布局，
        // workspace 精确 840 证明无 440 右栏）且 ≥560。
        assert_eq!(rail.size.width, px(RAIL_NARROW_WIDTH));
        assert_eq!(workspace.origin.x, rail.size.width);
        assert_eq!(workspace.size.width, px(1080. - RAIL_NARROW_WIDTH));
        assert!(workspace.size.width >= px(WORKSPACE_MIN_WIDTH));

        assert_eq!(status.origin.x, rail.size.width);
        assert_eq!(status.size.width, px(1080. - RAIL_NARROW_WIDTH));
        assert_eq!(status.origin.y, px(720. - metrics::STATUS_BAR_HEIGHT));
        assert_eq!(status.size.height, px(metrics::STATUS_BAR_HEIGHT));

        assert_within(rail, 1080., 720., "rail");
        assert_within(workspace, 1080., 720., "workspace");
        assert_within(status, 1080., 720., "status");
    }

    #[gpui::test]
    fn resize_collapses_and_restores_inspector(cx: &mut TestAppContext) {
        let cx = mount_shell(cx, 1440., 1024.);
        let wide_inspector = cx
            .debug_bounds("shell-inspector")
            .expect("inspector at 1440");
        assert_eq!(wide_inspector.size.width, px(metrics::INSPECTOR_WIDTH));

        cx.simulate_resize(size(px(1080.), px(720.)));
        cx.refresh().expect("refresh after shrink");
        cx.run_until_parked();
        let workspace = cx.debug_bounds("shell-workspace").expect("workspace at 1080");
        assert_eq!(workspace.size.width, px(1080. - RAIL_NARROW_WIDTH));

        // 偏好未被 resize 改写：加宽后 Inspector 自动恢复 440 右栏。
        cx.simulate_resize(size(px(1440.), px(1024.)));
        cx.refresh().expect("refresh after widen");
        cx.run_until_parked();
        let restored = cx
            .debug_bounds("shell-inspector")
            .expect("inspector restored at 1440");
        assert_eq!(restored.size.width, px(metrics::INSPECTOR_WIDTH));
    }
}
