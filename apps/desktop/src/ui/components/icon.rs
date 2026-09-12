//! GUI3-05：单色 SVG 图标。`svg()` 由 gpui 渲成 alpha mask，颜色走
//! `text_color`；`TestAppContext` 的 asset source 为 `()`，测试里只占位。

use std::borrow::Cow;

use gpui::{prelude::*, px, svg, AssetSource, Pixels, SharedString, Svg};

use crate::ui::theme::metrics;

/// 登记的动作 / 类型图标。路径与 `apps/desktop/assets/icons/*.svg` 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Send,
    Cancel,
    Search,
    Find,
    Plus,
    Grouping,
    Clock,
    Settings,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    More,
    Copy,
    ArrowDown,
    ArrowUp,
    Refresh,
    Collapse,
    File,
    Check,
    RadioDot,
    RadioRing,
    Link,
    Edit,
    Archive,
    Inspector,
    Terminal,
    Changes,
    Resources,
    Turns,
    Branch,
    Task,
    Page,
    SettingsRow,
    Providers,
    Network,
    Approvals,
    Tools,
    Appearance,
    Advanced,
    About,
    Project,
}

impl Icon {
    /// 资产路径（`icons/foo.svg`），供 `AssetSource::load` 与 `svg().path()` 共用。
    pub const fn path(self) -> &'static str {
        match self {
            Self::Send => "icons/send.svg",
            Self::Cancel => "icons/cancel.svg",
            Self::Search => "icons/search.svg",
            Self::Find => "icons/find.svg",
            Self::Plus => "icons/plus.svg",
            Self::Grouping => "icons/grouping.svg",
            Self::Clock => "icons/clock.svg",
            Self::Settings => "icons/settings.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::ChevronLeft => "icons/chevron-left.svg",
            Self::More => "icons/more.svg",
            Self::Copy => "icons/copy.svg",
            Self::ArrowDown => "icons/arrow-down.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::Refresh => "icons/refresh.svg",
            Self::Collapse => "icons/collapse.svg",
            Self::File => "icons/file.svg",
            Self::Check => "icons/check.svg",
            Self::RadioDot => "icons/radio-dot.svg",
            Self::RadioRing => "icons/radio-ring.svg",
            Self::Link => "icons/link.svg",
            Self::Edit => "icons/edit.svg",
            Self::Archive => "icons/archive.svg",
            Self::Inspector => "icons/inspector.svg",
            Self::Terminal => "icons/terminal.svg",
            Self::Changes => "icons/changes.svg",
            Self::Resources => "icons/resources.svg",
            Self::Turns => "icons/turns.svg",
            Self::Branch => "icons/branch.svg",
            Self::Task => "icons/task.svg",
            Self::Page => "icons/page.svg",
            Self::SettingsRow => "icons/settings-row.svg",
            Self::Providers => "icons/providers.svg",
            Self::Network => "icons/network.svg",
            Self::Approvals => "icons/approvals.svg",
            Self::Tools => "icons/tools.svg",
            Self::Appearance => "icons/appearance.svg",
            Self::Advanced => "icons/advanced.svg",
            Self::About => "icons/about.svg",
            Self::Project => "icons/project.svg",
        }
    }

    pub const fn all() -> &'static [Icon] {
        &[
            Self::Send,
            Self::Cancel,
            Self::Search,
            Self::Find,
            Self::Plus,
            Self::Grouping,
            Self::Clock,
            Self::Settings,
            Self::ChevronDown,
            Self::ChevronRight,
            Self::ChevronLeft,
            Self::More,
            Self::Copy,
            Self::ArrowDown,
            Self::ArrowUp,
            Self::Refresh,
            Self::Collapse,
            Self::File,
            Self::Check,
            Self::RadioDot,
            Self::RadioRing,
            Self::Link,
            Self::Edit,
            Self::Archive,
            Self::Inspector,
            Self::Terminal,
            Self::Changes,
            Self::Resources,
            Self::Turns,
            Self::Branch,
            Self::Task,
            Self::Page,
            Self::SettingsRow,
            Self::Providers,
            Self::Network,
            Self::Approvals,
            Self::Tools,
            Self::Appearance,
            Self::Advanced,
            Self::About,
            Self::Project,
        ]
    }
}

/// 默认 20px 图标（主操作）。颜色由调用方或父级 `text_color` 染色。
pub fn icon(icon: Icon) -> Svg {
    icon_sized(icon, px(metrics::ICON_SIZE))
}

/// 指定边长（chip / 行内 16px，工具勾 14px 等）。
pub fn icon_sized(icon: Icon, size: impl Into<Pixels>) -> Svg {
    let size = size.into();
    svg().path(icon.path()).size(size).flex_none()
}

/// `include_bytes!` 静态表；不引入 rust-embed。
pub struct Assets;

fn icon_bytes(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "icons/send.svg" => include_bytes!("../../../assets/icons/send.svg").as_slice(),
        "icons/cancel.svg" => include_bytes!("../../../assets/icons/cancel.svg").as_slice(),
        "icons/search.svg" => include_bytes!("../../../assets/icons/search.svg").as_slice(),
        "icons/find.svg" => include_bytes!("../../../assets/icons/find.svg").as_slice(),
        "icons/plus.svg" => include_bytes!("../../../assets/icons/plus.svg").as_slice(),
        "icons/grouping.svg" => include_bytes!("../../../assets/icons/grouping.svg").as_slice(),
        "icons/clock.svg" => include_bytes!("../../../assets/icons/clock.svg").as_slice(),
        "icons/settings.svg" => include_bytes!("../../../assets/icons/settings.svg").as_slice(),
        "icons/chevron-down.svg" => {
            include_bytes!("../../../assets/icons/chevron-down.svg").as_slice()
        }
        "icons/chevron-right.svg" => {
            include_bytes!("../../../assets/icons/chevron-right.svg").as_slice()
        }
        "icons/chevron-left.svg" => {
            include_bytes!("../../../assets/icons/chevron-left.svg").as_slice()
        }
        "icons/more.svg" => include_bytes!("../../../assets/icons/more.svg").as_slice(),
        "icons/copy.svg" => include_bytes!("../../../assets/icons/copy.svg").as_slice(),
        "icons/arrow-down.svg" => include_bytes!("../../../assets/icons/arrow-down.svg").as_slice(),
        "icons/arrow-up.svg" => include_bytes!("../../../assets/icons/arrow-up.svg").as_slice(),
        "icons/refresh.svg" => include_bytes!("../../../assets/icons/refresh.svg").as_slice(),
        "icons/collapse.svg" => include_bytes!("../../../assets/icons/collapse.svg").as_slice(),
        "icons/file.svg" => include_bytes!("../../../assets/icons/file.svg").as_slice(),
        "icons/check.svg" => include_bytes!("../../../assets/icons/check.svg").as_slice(),
        "icons/radio-dot.svg" => include_bytes!("../../../assets/icons/radio-dot.svg").as_slice(),
        "icons/radio-ring.svg" => include_bytes!("../../../assets/icons/radio-ring.svg").as_slice(),
        "icons/link.svg" => include_bytes!("../../../assets/icons/link.svg").as_slice(),
        "icons/edit.svg" => include_bytes!("../../../assets/icons/edit.svg").as_slice(),
        "icons/archive.svg" => include_bytes!("../../../assets/icons/archive.svg").as_slice(),
        "icons/inspector.svg" => include_bytes!("../../../assets/icons/inspector.svg").as_slice(),
        "icons/terminal.svg" => include_bytes!("../../../assets/icons/terminal.svg").as_slice(),
        "icons/changes.svg" => include_bytes!("../../../assets/icons/changes.svg").as_slice(),
        "icons/resources.svg" => include_bytes!("../../../assets/icons/resources.svg").as_slice(),
        "icons/turns.svg" => include_bytes!("../../../assets/icons/turns.svg").as_slice(),
        "icons/branch.svg" => include_bytes!("../../../assets/icons/branch.svg").as_slice(),
        "icons/task.svg" => include_bytes!("../../../assets/icons/task.svg").as_slice(),
        "icons/page.svg" => include_bytes!("../../../assets/icons/page.svg").as_slice(),
        "icons/settings-row.svg" => {
            include_bytes!("../../../assets/icons/settings-row.svg").as_slice()
        }
        "icons/providers.svg" => include_bytes!("../../../assets/icons/providers.svg").as_slice(),
        "icons/network.svg" => include_bytes!("../../../assets/icons/network.svg").as_slice(),
        "icons/approvals.svg" => include_bytes!("../../../assets/icons/approvals.svg").as_slice(),
        "icons/tools.svg" => include_bytes!("../../../assets/icons/tools.svg").as_slice(),
        "icons/appearance.svg" => include_bytes!("../../../assets/icons/appearance.svg").as_slice(),
        "icons/advanced.svg" => include_bytes!("../../../assets/icons/advanced.svg").as_slice(),
        "icons/about.svg" => include_bytes!("../../../assets/icons/about.svg").as_slice(),
        "icons/project.svg" => include_bytes!("../../../assets/icons/project.svg").as_slice(),
        _ => return None,
    })
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(icon_bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let prefix = path.trim_end_matches('/');
        Ok(Icon::all()
            .iter()
            .map(|icon| SharedString::from(icon.path()))
            .filter(|registered| {
                if prefix.is_empty() || prefix == "icons" {
                    registered.starts_with("icons/")
                } else {
                    registered.as_ref() == prefix
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{Assets, Icon};
    use gpui::AssetSource;

    #[test]
    fn assets_load_every_registered_icon() {
        let assets = Assets;
        let all = Icon::all();
        let mut paths = Vec::new();
        for icon in all {
            let path = icon.path();
            assert!(!paths.contains(&path), "duplicate Icon path {path}");
            paths.push(path);
            let bytes = assets
                .load(path)
                .expect("asset load")
                .unwrap_or_else(|| panic!("missing bytes for {path}"));
            let trimmed = std::str::from_utf8(&bytes)
                .unwrap_or_else(|_| panic!("{path} is not utf-8"))
                .trim_start();
            assert!(
                trimmed.starts_with("<svg"),
                "{path} does not start with <svg>"
            );
        }
        let listed = assets.list("icons").expect("list icons");
        assert_eq!(listed.len(), all.len());
        assert!(assets.load("icons/missing.svg").unwrap().is_none());
    }
}
