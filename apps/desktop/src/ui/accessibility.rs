//! ADR-042 Desktop Accessibility 语义模型与平台 bridge facade。
//!
//! 语义树显式来自 AppView 状态，不读取 GPUI 私有 frame、不做 OCR/像素反推。
//! macOS 原生对象与 Objective-C runtime 隔离在 `accessibility/macos.rs`。

use std::collections::BTreeSet;

#[cfg(not(target_os = "macos"))]
use gpui::Window;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::AxBridge;
mod app;

/// AX 坐标使用 GPUI content 的顶左原点、逻辑像素。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AxRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl AxRect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f32::is_finite)
            && self.width >= 0.0
            && self.height >= 0.0
    }

    #[cfg(test)]
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxRole {
    Group,
    Button,
    TextArea,
    StaticText,
    List,
    ListItem,
    TabGroup,
    Tab,
}

impl AxRole {
    #[cfg(target_os = "macos")]
    pub(super) const fn macos_name(self) -> &'static str {
        match self {
            Self::Group => "AXGroup",
            Self::Button => "AXButton",
            Self::TextArea => "AXTextArea",
            Self::StaticText => "AXStaticText",
            Self::List => "AXList",
            Self::ListItem => "AXRow",
            Self::TabGroup => "AXTabGroup",
            Self::Tab => "AXRadioButton",
        }
    }
}

/// 原生调用只携带受支持的 action kind；identifier 在 AppView 侧白名单映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxAction {
    Press,
    Focus,
    SetValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxRequest {
    pub identifier: String,
    pub action: AxAction,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AxNode {
    pub identifier: String,
    pub role: AxRole,
    pub label: String,
    pub value: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub focused: bool,
    pub selected: bool,
    pub bounds: AxRect,
    pub actions: Vec<AxAction>,
    pub children: Vec<AxNode>,
}

impl AxNode {
    pub fn new(
        identifier: impl Into<String>,
        role: AxRole,
        label: impl Into<String>,
        bounds: AxRect,
    ) -> Self {
        Self {
            identifier: identifier.into(),
            role,
            label: label.into(),
            value: None,
            description: None,
            enabled: true,
            focused: false,
            selected: false,
            bounds,
            actions: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn action(mut self, action: AxAction) -> Self {
        if !self.actions.contains(&action) {
            self.actions.push(action);
        }
        self
    }

    pub fn child(mut self, child: AxNode) -> Self {
        self.children.push(child);
        self
    }

    fn validate_into(&self, seen: &mut BTreeSet<String>) -> Result<(), String> {
        if self.identifier.trim().is_empty() {
            return Err("AX identifier must not be empty".into());
        }
        if !seen.insert(self.identifier.clone()) {
            return Err(format!("duplicate AX identifier: {}", self.identifier));
        }
        if !self.bounds.is_valid() {
            return Err(format!("invalid AX bounds: {}", self.identifier));
        }
        if self.label.contains('\0')
            || self.identifier.contains('\0')
            || self
                .value
                .as_ref()
                .is_some_and(|value| value.contains('\0'))
            || self
                .description
                .as_ref()
                .is_some_and(|value| value.contains('\0'))
        {
            return Err(format!("AX text contains NUL: {}", self.identifier));
        }
        for child in &self.children {
            child.validate_into(seen)?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn hit_test(&self, x: f32, y: f32) -> Option<&AxNode> {
        if !self.bounds.contains(x, y) {
            return None;
        }
        self.children
            .iter()
            .rev()
            .find_map(|child| child.hit_test(x, y))
            .or(Some(self))
    }

    fn find(&self, identifier: &str) -> Option<&AxNode> {
        if self.identifier == identifier {
            return Some(self);
        }
        self.children
            .iter()
            .find_map(|child| child.find(identifier))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AxTree {
    pub viewport: AxRect,
    pub children: Vec<AxNode>,
}

impl AxTree {
    pub fn new(viewport_width: f32, viewport_height: f32) -> Self {
        Self {
            viewport: AxRect::new(0.0, 0.0, viewport_width, viewport_height),
            children: Vec::new(),
        }
    }

    pub fn child(mut self, child: AxNode) -> Self {
        self.children.push(child);
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.viewport.is_valid() || self.viewport.width <= 0.0 || self.viewport.height <= 0.0 {
            return Err("AX viewport must be finite and non-empty".into());
        }
        let mut seen = BTreeSet::new();
        for child in &self.children {
            child.validate_into(&mut seen)?;
        }
        Ok(())
    }

    pub fn focused(&self) -> Option<&AxNode> {
        fn find(nodes: &[AxNode]) -> Option<&AxNode> {
            nodes.iter().find_map(|node| {
                if node.focused {
                    Some(node)
                } else {
                    find(&node.children)
                }
            })
        }
        find(&self.children)
    }

    /// 原生 AX element 可能被系统客户端短暂保留；action 到达时必须按最新树
    /// 重新核对 identifier、enabled 与 action，不能信任旧 element 的快照状态。
    fn permits(&self, request: &AxRequest) -> bool {
        self.find(&request.identifier)
            .is_some_and(|node| node.enabled && node.actions.contains(&request.action))
    }

    #[cfg(test)]
    fn hit_test(&self, x: f32, y: f32) -> Option<&AxNode> {
        self.children
            .iter()
            .rev()
            .find_map(|child| child.hit_test(x, y))
    }

    fn find(&self, identifier: &str) -> Option<&AxNode> {
        self.children
            .iter()
            .find_map(|child| child.find(identifier))
    }
}

#[cfg(not(target_os = "macos"))]
pub struct AxBridge;

#[cfg(not(target_os = "macos"))]
impl AxBridge {
    pub fn install(
        _window: &Window,
        _handler: impl Fn(AxRequest) + 'static,
    ) -> Result<Self, String> {
        Ok(Self)
    }

    pub fn update(&mut self, tree: AxTree) -> Result<bool, String> {
        tree.validate()?;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tree() -> AxTree {
        AxTree::new(800.0, 600.0).child(
            AxNode::new(
                "group",
                AxRole::Group,
                "Workspace",
                AxRect::new(0.0, 0.0, 800.0, 600.0),
            )
            .child(
                AxNode::new(
                    "send",
                    AxRole::Button,
                    "Send",
                    AxRect::new(700.0, 540.0, 80.0, 40.0),
                )
                .focused(true)
                .action(AxAction::Press),
            ),
        )
    }

    #[test]
    fn validates_unique_identifiers_and_finite_bounds() {
        assert!(sample_tree().validate().is_ok());
        let duplicate = sample_tree().child(AxNode::new(
            "send",
            AxRole::Button,
            "Duplicate",
            AxRect::new(0.0, 0.0, 1.0, 1.0),
        ));
        assert_eq!(
            duplicate.validate().unwrap_err(),
            "duplicate AX identifier: send"
        );
        let invalid = AxTree::new(1.0, 1.0).child(AxNode::new(
            "bad",
            AxRole::Group,
            "Bad",
            AxRect::new(0.0, 0.0, f32::NAN, 1.0),
        ));
        assert_eq!(invalid.validate().unwrap_err(), "invalid AX bounds: bad");
    }

    #[test]
    fn hit_test_prefers_deepest_last_child_and_focus_is_discoverable() {
        let tree = sample_tree();
        assert_eq!(tree.hit_test(720.0, 560.0).unwrap().identifier, "send");
        assert_eq!(tree.hit_test(20.0, 20.0).unwrap().identifier, "group");
        assert_eq!(tree.focused().unwrap().identifier, "send");
        assert!(tree.hit_test(900.0, 700.0).is_none());
    }

    #[test]
    fn builder_deduplicates_actions_and_find_walks_hierarchy() {
        let node = AxNode::new(
            "composer-input",
            AxRole::TextArea,
            "Message",
            AxRect::new(0.0, 0.0, 100.0, 40.0),
        )
        .action(AxAction::Focus)
        .action(AxAction::Focus)
        .action(AxAction::SetValue)
        .value("hello");
        let tree = AxTree::new(100.0, 100.0).child(node);
        let input = tree.find("composer-input").unwrap();
        assert_eq!(input.actions, vec![AxAction::Focus, AxAction::SetValue]);
        assert_eq!(input.value.as_deref(), Some("hello"));

        let set_value = AxRequest {
            identifier: "composer-input".into(),
            action: AxAction::SetValue,
            value: Some("next".into()),
        };
        assert!(tree.permits(&set_value));
        assert!(!tree.permits(&AxRequest {
            identifier: "composer-input".into(),
            action: AxAction::Press,
            value: None,
        }));
        assert!(!tree.permits(&AxRequest {
            identifier: "missing".into(),
            action: AxAction::SetValue,
            value: Some("next".into()),
        }));

        let disabled = AxTree::new(100.0, 100.0).child(
            AxNode::new(
                "send",
                AxRole::Button,
                "Send",
                AxRect::new(0.0, 0.0, 20.0, 20.0),
            )
            .enabled(false)
            .action(AxAction::Press),
        );
        assert!(!disabled.permits(&AxRequest {
            identifier: "send".into(),
            action: AxAction::Press,
            value: None,
        }));
    }
}
