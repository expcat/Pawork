//! macOS AppKit implementation for ADR-042.
#![allow(unexpected_cfgs)] // objc 0.2 macros probe their historical cargo-clippy cfg.

use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use cocoa::base::{id, nil};
use cocoa::foundation::{NSArray, NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use gpui::Window;
use objc::declare::ClassDecl;
use objc::runtime::{self, Class, Object, Sel, BOOL, NO, YES};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{AxAction, AxNode, AxRect, AxRequest, AxTree};

const ELEMENT_STATE_IVAR: &str = "paworkAxState";

#[derive(Clone, Copy)]
struct ScreenRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl ScreenRect {
    fn contains(self, point: NSPoint) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }
}

struct HitNode {
    element: usize,
    frame: ScreenRect,
    depth: usize,
}

#[derive(Default)]
struct ViewState {
    nodes: Vec<HitNode>,
    focused: Option<usize>,
    by_identifier: HashMap<String, usize>,
}

fn view_states() -> &'static Mutex<HashMap<usize, ViewState>> {
    static STATES: OnceLock<Mutex<HashMap<usize, ViewState>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

struct ElementState {
    identifier: String,
    actions: Vec<AxAction>,
    enabled: bool,
    handler: Rc<dyn Fn(AxRequest)>,
}

impl ElementState {
    fn send(&self, action: AxAction, value: Option<String>) -> bool {
        if !self.enabled || !self.actions.contains(&action) {
            return false;
        }
        (self.handler)(AxRequest {
            identifier: self.identifier.clone(),
            action,
            value,
        });
        true
    }
}

unsafe fn element_state(this: &Object) -> Option<&ElementState> {
    let raw = unsafe { *this.get_ivar::<*mut c_void>(ELEMENT_STATE_IVAR) };
    unsafe { (raw as *const ElementState).as_ref() }
}

extern "C" fn element_dealloc(this: &mut Object, _cmd: Sel) {
    unsafe {
        let raw = *this.get_ivar::<*mut c_void>(ELEMENT_STATE_IVAR);
        if !raw.is_null() {
            drop(Box::from_raw(raw as *mut ElementState));
            this.set_ivar(ELEMENT_STATE_IVAR, std::ptr::null_mut::<c_void>());
        }
        let _: () = msg_send![super(this, class!(NSAccessibilityElement)), dealloc];
    }
}

/// AppKit 保证在主线程服务外部 AX 请求（与 GPUI UI 线程一致）；
/// ElementState.handler 的 Rc 依赖该不变量，debug 构建下显式断言。
fn debug_assert_main_thread() {
    // cfg(test)：单测在各自线程直接调用 IMP，非主线程属预期，跳过断言。
    #[cfg(not(test))]
    {
        let is_main: BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
        debug_assert!(is_main == YES, "AX action dispatched off main thread");
    }
}

extern "C" fn element_press(this: &Object, _cmd: Sel) -> BOOL {
    debug_assert_main_thread();
    unsafe {
        if element_state(this).is_some_and(|state| state.send(AxAction::Press, None)) {
            YES
        } else {
            NO
        }
    }
}

extern "C" fn element_attribute_settable(this: &Object, _cmd: Sel, attribute: id) -> BOOL {
    unsafe {
        let Some(state) = element_state(this).filter(|state| state.enabled) else {
            return NO;
        };
        if attribute == nil {
            return NO;
        }
        let utf8 = NSString::UTF8String(attribute);
        if utf8.is_null() {
            return NO;
        }
        match std::ffi::CStr::from_ptr(utf8).to_bytes() {
            b"AXValue" if state.actions.contains(&AxAction::SetValue) => YES,
            b"AXFocused" if state.actions.contains(&AxAction::Focus) => YES,
            _ => NO,
        }
    }
}

extern "C" fn element_selector_allowed(this: &Object, _cmd: Sel, selector: Sel) -> BOOL {
    unsafe {
        let Some(state) = element_state(this) else {
            return msg_send![super(this, class!(NSAccessibilityElement)), isAccessibilitySelectorAllowed: selector];
        };
        if selector == sel!(setAccessibilityValue:) {
            return if state.enabled && state.actions.contains(&AxAction::SetValue) {
                YES
            } else {
                NO
            };
        }
        if selector == sel!(setAccessibilityFocused:) {
            return if state.enabled && state.actions.contains(&AxAction::Focus) {
                YES
            } else {
                NO
            };
        }
        if selector == sel!(accessibilityPerformPress) {
            return if state.enabled && state.actions.contains(&AxAction::Press) {
                YES
            } else {
                NO
            };
        }
        msg_send![super(this, class!(NSAccessibilityElement)), isAccessibilitySelectorAllowed: selector]
    }
}

extern "C" fn element_set_focused(this: &mut Object, _cmd: Sel, focused: BOOL) {
    debug_assert_main_thread();
    unsafe {
        let Some(state) = element_state(this) else {
            return;
        };
        // 与 set_value 同一双门：未声明 Focus 动作时连 AppKit 侧缓存也不更新；
        // 内部树同步走 build/refresh 的 super 直调，不经此 override。
        if !state.enabled || !state.actions.contains(&AxAction::Focus) {
            return;
        }
        let _: () = msg_send![super(this, class!(NSAccessibilityElement)), setAccessibilityFocused: focused];
        if focused != NO {
            let _ = state.send(AxAction::Focus, None);
        }
    }
}

extern "C" fn element_set_value(this: &mut Object, _cmd: Sel, value: id) {
    debug_assert_main_thread();
    unsafe {
        let Some(state) = element_state(this) else {
            return;
        };
        let action_gate = state.enabled && state.actions.contains(&AxAction::SetValue);
        if !action_gate {
            return;
        }
        let _: () =
            msg_send![super(this, class!(NSAccessibilityElement)), setAccessibilityValue: value];
        if value == nil {
            let _ = state.send(AxAction::SetValue, Some(String::new()));
            return;
        }
        let utf8 = NSString::UTF8String(value);
        if utf8.is_null() {
            return;
        }
        let value = std::ffi::CStr::from_ptr(utf8)
            .to_string_lossy()
            .into_owned();
        let _ = state.send(AxAction::SetValue, Some(value));
    }
}

fn element_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| unsafe {
        if let Some(class) = Class::get("PaworkAXElement") {
            return class;
        }
        let mut decl = ClassDecl::new("PaworkAXElement", class!(NSAccessibilityElement))
            .expect("allocate PaworkAXElement");
        decl.add_ivar::<*mut c_void>(ELEMENT_STATE_IVAR);
        decl.add_method(
            sel!(dealloc),
            element_dealloc as extern "C" fn(&mut Object, Sel),
        );
        decl.add_method(
            sel!(accessibilityIsAttributeSettable:),
            element_attribute_settable as extern "C" fn(&Object, Sel, id) -> BOOL,
        );
        decl.add_method(
            sel!(isAccessibilitySelectorAllowed:),
            element_selector_allowed as extern "C" fn(&Object, Sel, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(setAccessibilityFocused:),
            element_set_focused as extern "C" fn(&mut Object, Sel, BOOL),
        );
        decl.add_method(
            sel!(setAccessibilityValue:),
            element_set_value as extern "C" fn(&mut Object, Sel, id),
        );
        decl.register()
    })
}

fn press_element_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| unsafe {
        if let Some(class) = Class::get("PaworkAXPressElement") {
            return class;
        }
        let mut decl = ClassDecl::new("PaworkAXPressElement", element_class())
            .expect("allocate PaworkAXPressElement");
        decl.add_method(
            sel!(accessibilityPerformPress),
            element_press as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.register()
    })
}

extern "C" fn view_is_accessibility_element(_this: &Object, _cmd: Sel) -> BOOL {
    YES
}

extern "C" fn view_hit_test(this: &Object, _cmd: Sel, point: NSPoint) -> id {
    let Ok(states) = view_states().lock() else {
        return this as *const Object as id;
    };
    states
        .get(&(this as *const Object as usize))
        .and_then(|state| {
            state
                .nodes
                .iter()
                .filter(|node| node.frame.contains(point))
                .max_by_key(|node| node.depth)
                .map(|node| node.element as id)
        })
        .unwrap_or(this as *const Object as id)
}

extern "C" fn view_focused_element(this: &Object, _cmd: Sel) -> id {
    view_states()
        .lock()
        .ok()
        .and_then(|states| {
            states
                .get(&(this as *const Object as usize))
                .and_then(|state| state.focused)
        })
        .map(|element| element as id)
        .unwrap_or(this as *const Object as id)
}

fn accessible_view_class(superclass: &Class) -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| unsafe {
        if let Some(class) = Class::get("PaworkAccessibleGPUIView") {
            return class;
        }
        let mut decl = ClassDecl::new("PaworkAccessibleGPUIView", superclass)
            .expect("allocate PaworkAccessibleGPUIView");
        decl.add_method(
            sel!(isAccessibilityElement),
            view_is_accessibility_element as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(accessibilityHitTest:),
            view_hit_test as extern "C" fn(&Object, Sel, NSPoint) -> id,
        );
        decl.add_method(
            sel!(accessibilityFocusedUIElement),
            view_focused_element as extern "C" fn(&Object, Sel) -> id,
        );
        decl.register()
    })
}

unsafe extern "C" {
    fn object_setClass(object: *mut Object, class: *const Class) -> *const Class;
    fn NSAccessibilityPostNotification(element: id, notification: id);
    static NSAccessibilityLayoutChangedNotification: id;
    static NSAccessibilityFocusedUIElementChangedNotification: id;
    static NSAccessibilityValueChangedNotification: id;
}

pub struct AxBridge {
    view: id,
    original_class: *const Class,
    objects: Vec<id>,
    handler: Rc<dyn Fn(AxRequest)>,
    tree: Option<AxTree>,
}

impl AxBridge {
    pub fn install(window: &Window, handler: impl Fn(AxRequest) + 'static) -> Result<Self, String> {
        let raw = HasWindowHandle::window_handle(window)
            .map_err(|error| format!("get AppKit window handle: {error}"))?;
        let RawWindowHandle::AppKit(handle) = raw.as_raw() else {
            return Err("gpui window did not expose an AppKit NSView".into());
        };
        let view = handle.ns_view.as_ptr() as id;
        if view == nil {
            return Err("gpui AppKit NSView was null".into());
        }
        unsafe {
            let original_class = runtime::object_getClass(view);
            if original_class.is_null() {
                return Err("gpui AppKit NSView had no Objective-C class".into());
            }
            if (&*original_class).name() == "PaworkAccessibleGPUIView" {
                return Err("accessibility bridge is already installed on this NSView".into());
            }
            let subclass = accessible_view_class(&*original_class);
            view_states()
                .lock()
                .map_err(|_| "AX view state lock poisoned".to_string())?
                .insert(view as usize, ViewState::default());
            // raw-window-handle only lends this pointer. Retain it while the bridge stores
            // the NSView across renders so Drop can restore the original Objective-C class.
            let _: id = msg_send![view, retain];
            object_setClass(view, subclass);
            let role = ns_string("AXGroup");
            let label = ns_string("Pawork");
            let identifier = ns_string("pawork-root");
            let _: () = msg_send![view, setAccessibilityElement: YES];
            let _: () = msg_send![view, setAccessibilityRole: role];
            let _: () = msg_send![view, setAccessibilityLabel: label];
            let _: () = msg_send![view, setAccessibilityIdentifier: identifier];
            Ok(Self {
                view,
                original_class,
                objects: Vec::new(),
                handler: Rc::new(handler),
                tree: None,
            })
        }
    }

    pub fn update(&mut self, tree: AxTree) -> Result<bool, String> {
        tree.validate()?;
        if self.tree.as_ref() == Some(&tree) {
            return Ok(false);
        }
        let previous = self.tree.as_ref();
        let previous_focus = previous
            .and_then(AxTree::focused)
            .map(|node| node.identifier.as_str());
        let next_focus = tree.focused().map(|node| node.identifier.as_str());
        let changed_values = previous
            .map(|old| changed_value_identifiers(old, &tree))
            .unwrap_or_default();
        // 结构（identifier/role/press 能力/子树形状）不变时原位刷新既有原生
        // element，避免流式文本每帧整树重建、使外部 AX 客户端持有的 element
        // 全部失效（D4：AX 通道不引入渲染面没有的状态）。
        let structural = previous.is_none_or(|old| !same_skeleton(&old.children, &tree.children));

        unsafe {
            let mut refreshed = false;
            if !structural {
                if let Some(state) = self.refresh_tree(&tree)? {
                    view_states()
                        .lock()
                        .map_err(|_| "AX view state lock poisoned".to_string())?
                        .insert(self.view as usize, state);
                    refreshed = true;
                }
            }
            if !refreshed {
                let mut objects = Vec::new();
                let mut state = ViewState::default();
                let mut top_level = Vec::new();
                for node in &tree.children {
                    top_level.push(self.build_node(
                        node,
                        self.view,
                        tree.viewport,
                        tree.viewport,
                        0,
                        &mut objects,
                        &mut state,
                    ));
                }
                let array = NSArray::arrayWithObjects(nil, &top_level);
                let _: () = msg_send![self.view, setAccessibilityChildren: array];

                view_states()
                    .lock()
                    .map_err(|_| "AX view state lock poisoned".to_string())?
                    .insert(self.view as usize, state);

                let old_objects = std::mem::replace(&mut self.objects, objects);
                for object in old_objects {
                    let _: () = msg_send![object, release];
                }

                NSAccessibilityPostNotification(
                    self.view,
                    NSAccessibilityLayoutChangedNotification,
                );
            }
            if previous_focus != next_focus {
                let focused = next_focus
                    .and_then(|identifier| self.native_element(identifier))
                    .unwrap_or(self.view);
                NSAccessibilityPostNotification(
                    focused,
                    NSAccessibilityFocusedUIElementChangedNotification,
                );
            }
            for identifier in changed_values {
                if let Some(element) = self.native_element(&identifier) {
                    NSAccessibilityPostNotification(
                        element,
                        NSAccessibilityValueChangedNotification,
                    );
                }
            }
        }
        self.tree = Some(tree);
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn build_node(
        &self,
        node: &AxNode,
        parent: id,
        parent_bounds: AxRect,
        viewport: AxRect,
        depth: usize,
        objects: &mut Vec<id>,
        view_state: &mut ViewState,
    ) -> id {
        unsafe {
            let class = if node.enabled && node.actions.contains(&AxAction::Press) {
                press_element_class()
            } else {
                element_class()
            };
            let object: id = msg_send![class, alloc];
            let object: id = msg_send![object, init];
            let role = ns_string(node.role.macos_name());
            let label = ns_string(&node.label);
            let identifier = ns_string(&node.identifier);
            let _: () = msg_send![object, setAccessibilityElement: YES];
            let _: () = msg_send![object, setAccessibilityRole: role];
            let _: () = msg_send![object, setAccessibilityLabel: label];
            let _: () = msg_send![object, setAccessibilityIdentifier: identifier];
            let _: () = msg_send![object, setAccessibilityParent: parent];
            let _: () =
                msg_send![object, setAccessibilityEnabled: if node.enabled { YES } else { NO }];
            let _: () =
                msg_send![object, setAccessibilitySelected: if node.selected { YES } else { NO }];
            // 初始属性同步直达 super：我们的 setAccessibilityValue: override 在
            // ElementState 安装前会把写入整体吞掉（gate 先决），且树同步本就不该
            // 触发对外 AxAction::SetValue 上报。
            if let Some(value) = node.value.as_deref() {
                let value = ns_string(value);
                let _: () = msg_send![super(object, class!(NSAccessibilityElement)), setAccessibilityValue: value];
            }
            if let Some(description) = node.description.as_deref() {
                let help = ns_string(description);
                let _: () = msg_send![object, setAccessibilityHelp: help];
            }

            let parent_frame = parent_space_frame(node.bounds, parent_bounds);
            let screen_frame = self.screen_frame(node.bounds, viewport);
            let _: () = msg_send![object, setAccessibilityFrame: screen_frame];
            let _: () = msg_send![object, setAccessibilityFrameInParentSpace: parent_frame];
            // 同 value：直达 super，避免 override 把树同步误报为 AxAction::Focus。
            let _: () = msg_send![super(object, class!(NSAccessibilityElement)), setAccessibilityFocused: if node.focused { YES } else { NO }];

            let state = Box::new(ElementState {
                identifier: node.identifier.clone(),
                actions: node.actions.clone(),
                enabled: node.enabled,
                handler: Rc::clone(&self.handler),
            });
            (&mut *object).set_ivar(ELEMENT_STATE_IVAR, Box::into_raw(state) as *mut c_void);

            let mut children = Vec::new();
            for child in &node.children {
                children.push(self.build_node(
                    child,
                    object,
                    node.bounds,
                    viewport,
                    depth + 1,
                    objects,
                    view_state,
                ));
            }
            if !children.is_empty() {
                let array = NSArray::arrayWithObjects(nil, &children);
                let _: () = msg_send![object, setAccessibilityChildren: array];
            }

            let screen = ScreenRect {
                x: screen_frame.origin.x,
                y: screen_frame.origin.y,
                width: screen_frame.size.width,
                height: screen_frame.size.height,
            };
            view_state.nodes.push(HitNode {
                element: object as usize,
                frame: screen,
                depth,
            });
            view_state
                .by_identifier
                .insert(node.identifier.clone(), object as usize);
            if node.focused {
                view_state.focused = Some(object as usize);
            }
            objects.push(object);
            object
        }
    }

    /// 原位刷新：复用既有原生 element，仅同步变化的属性并重建命中测试快照。
    /// 返回 None 表示新旧 element 对不上（防御性；same_skeleton 已保证可对上），
    /// 调用方回退到整树重建。
    unsafe fn refresh_tree(&self, tree: &AxTree) -> Result<Option<ViewState>, String> {
        let old_map = view_states()
            .lock()
            .map_err(|_| "AX view state lock poisoned".to_string())?
            .get(&(self.view as usize))
            .map(|state| state.by_identifier.clone())
            .unwrap_or_default();
        let previous = self
            .tree
            .as_ref()
            .expect("refresh requires an installed tree");
        let mut state = ViewState::default();
        for (old, node) in previous.children.iter().zip(&tree.children) {
            if self
                .refresh_node(
                    old,
                    node,
                    tree.viewport,
                    tree.viewport,
                    0,
                    &old_map,
                    &mut state,
                )
                .is_none()
            {
                return Ok(None);
            }
        }
        Ok(Some(state))
    }

    /// 单节点原位刷新。value/focused 直达 super，绕开 gated override，避免把
    /// 树同步误报为外部 AX 动作；ElementState 同步最新 enabled/actions/handler。
    #[allow(clippy::too_many_arguments)]
    unsafe fn refresh_node(
        &self,
        old: &AxNode,
        node: &AxNode,
        parent_bounds: AxRect,
        viewport: AxRect,
        depth: usize,
        old_map: &HashMap<String, usize>,
        view_state: &mut ViewState,
    ) -> Option<id> {
        let object = *old_map.get(&node.identifier)? as id;
        unsafe {
            if old.label != node.label {
                let label = ns_string(&node.label);
                let _: () = msg_send![object, setAccessibilityLabel: label];
            }
            if old.value != node.value {
                let value = node
                    .value
                    .as_deref()
                    .map(|value| ns_string(value))
                    .unwrap_or(nil);
                let _: () = msg_send![super(object, class!(NSAccessibilityElement)), setAccessibilityValue: value];
            }
            if old.description != node.description {
                let help = node
                    .description
                    .as_deref()
                    .map(|value| ns_string(value))
                    .unwrap_or(nil);
                let _: () = msg_send![object, setAccessibilityHelp: help];
            }
            if old.enabled != node.enabled {
                let _: () =
                    msg_send![object, setAccessibilityEnabled: if node.enabled { YES } else { NO }];
            }
            if old.selected != node.selected {
                let _: () = msg_send![object, setAccessibilitySelected: if node.selected { YES } else { NO }];
            }
            if old.focused != node.focused {
                let _: () = msg_send![super(object, class!(NSAccessibilityElement)), setAccessibilityFocused: if node.focused { YES } else { NO }];
            }
            // frame 每轮重算：bounds 相同但 viewport 变化（窗口缩放）时屏幕坐标仍会变。
            let parent_frame = parent_space_frame(node.bounds, parent_bounds);
            let screen_frame = self.screen_frame(node.bounds, viewport);
            let _: () = msg_send![object, setAccessibilityFrame: screen_frame];
            let _: () = msg_send![object, setAccessibilityFrameInParentSpace: parent_frame];

            // ElementState 跟随最新门状态；identifier 不变（same_skeleton 保证）。
            let raw = *(&*object).get_ivar::<*mut c_void>(ELEMENT_STATE_IVAR);
            if !raw.is_null() {
                let element_state = &mut *(raw as *mut ElementState);
                element_state.actions = node.actions.clone();
                element_state.enabled = node.enabled;
                element_state.handler = Rc::clone(&self.handler);
            }

            view_state.nodes.push(HitNode {
                element: object as usize,
                frame: ScreenRect {
                    x: screen_frame.origin.x,
                    y: screen_frame.origin.y,
                    width: screen_frame.size.width,
                    height: screen_frame.size.height,
                },
                depth,
            });
            view_state
                .by_identifier
                .insert(node.identifier.clone(), object as usize);
            if node.focused {
                view_state.focused = Some(object as usize);
            }
            for (old_child, child) in old.children.iter().zip(&node.children) {
                self.refresh_node(
                    old_child,
                    child,
                    node.bounds,
                    viewport,
                    depth + 1,
                    old_map,
                    view_state,
                )?;
            }
        }
        Some(object)
    }

    unsafe fn screen_frame(&self, bounds: AxRect, viewport: AxRect) -> NSRect {
        unsafe {
            let view_rect = NSRect::new(
                NSPoint::new(
                    bounds.x as f64,
                    (viewport.height - bounds.y - bounds.height) as f64,
                ),
                NSSize::new(bounds.width as f64, bounds.height as f64),
            );
            let window_rect: NSRect = msg_send![self.view, convertRect: view_rect toView: nil];
            let window: id = msg_send![self.view, window];
            if window == nil {
                return view_rect;
            }
            msg_send![window, convertRectToScreen: window_rect]
        }
    }

    fn native_element(&self, identifier: &str) -> Option<id> {
        view_states()
            .lock()
            .ok()
            .and_then(|states| {
                states
                    .get(&(self.view as usize))
                    .and_then(|state| state.by_identifier.get(identifier).copied())
            })
            .map(|element| element as id)
    }
}

impl Drop for AxBridge {
    fn drop(&mut self) {
        unsafe {
            let empty = NSArray::array(nil);
            let _: () = msg_send![self.view, setAccessibilityChildren: empty];
            let _: () = msg_send![self.view, setAccessibilityElement: NO];
            view_states()
                .lock()
                .ok()
                .map(|mut states| states.remove(&(self.view as usize)));
            for object in self.objects.drain(..) {
                let _: () = msg_send![object, release];
            }
            if !self.original_class.is_null() {
                object_setClass(self.view, self.original_class);
            }
            let _: () = msg_send![self.view, release];
        }
    }
}

fn parent_space_frame(bounds: AxRect, parent: AxRect) -> NSRect {
    NSRect::new(
        NSPoint::new(
            (bounds.x - parent.x) as f64,
            (parent.height - (bounds.y - parent.y) - bounds.height) as f64,
        ),
        NSSize::new(bounds.width as f64, bounds.height as f64),
    )
}

unsafe fn ns_string(value: &str) -> id {
    unsafe { NSString::alloc(nil).init_str(value).autorelease() }
}

/// 原生 element 的 class 在构建期按 press 能力选择（PaworkAXPressElement），
/// 能力变化视为结构变化，必须整树重建。
fn press_capable(node: &AxNode) -> bool {
    node.enabled && node.actions.contains(&AxAction::Press)
}

/// 结构 = identifier + role + press 能力 + 子树形状。结构不变时 update 走
/// 原位刷新，不重建原生对象。
fn same_skeleton(old: &[AxNode], new: &[AxNode]) -> bool {
    old.len() == new.len()
        && old.iter().zip(new.iter()).all(|(old, new)| {
            old.identifier == new.identifier
                && old.role == new.role
                && press_capable(old) == press_capable(new)
                && same_skeleton(&old.children, &new.children)
        })
}

fn changed_value_identifiers(previous: &AxTree, next: &AxTree) -> Vec<String> {
    fn collect<'a>(nodes: &'a [AxNode], values: &mut HashMap<&'a str, Option<&'a str>>) {
        for node in nodes {
            values.insert(&node.identifier, node.value.as_deref());
            collect(&node.children, values);
        }
    }
    let mut old = HashMap::new();
    let mut new = HashMap::new();
    collect(&previous.children, &mut old);
    collect(&next.children, &mut new);
    new.into_iter()
        .filter_map(|(identifier, value)| {
            (old.get(identifier).copied().flatten() != value).then(|| identifier.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn native_dispatch_honors_attribute_settable_gate() {
        let calls = Rc::new(Cell::new(0));
        let witness = Rc::clone(&calls);
        let state = Box::new(ElementState {
            identifier: "composer-input".into(),
            actions: vec![AxAction::Focus],
            enabled: true,
            handler: Rc::new(move |_| witness.set(witness.get() + 1)),
        });
        let state = Box::into_raw(state) as *mut c_void;
        unsafe {
            let object: id = msg_send![element_class(), alloc];
            let object: id = msg_send![object, init];
            (*object).set_ivar(ELEMENT_STATE_IVAR, state);
            let attribute = ns_string("AXValue");
            assert_eq!(
                element_attribute_settable(
                    &*object,
                    sel!(accessibilityIsAttributeSettable:),
                    attribute,
                ),
                NO
            );
            let value = ns_string("forbidden");
            element_set_value(&mut *object, sel!(setAccessibilityValue:), value);
            assert_eq!(calls.get(), 0);
            let _: () = msg_send![object, release];
        }
    }

    #[test]
    fn top_left_bounds_convert_to_parent_bottom_left_space() {
        let frame = parent_space_frame(
            AxRect::new(15.0, 25.0, 100.0, 40.0),
            AxRect::new(5.0, 10.0, 300.0, 200.0),
        );
        assert_eq!(frame.origin.x, 10.0);
        assert_eq!(frame.origin.y, 145.0);
        assert_eq!(frame.size.width, 100.0);
        assert_eq!(frame.size.height, 40.0);
    }

    #[test]
    fn value_diff_only_returns_changed_or_new_nodes() {
        let old = AxTree::new(10.0, 10.0).child(
            AxNode::new(
                "a",
                super::super::AxRole::StaticText,
                "A",
                AxRect::default(),
            )
            .value("one"),
        );
        let new = AxTree::new(10.0, 10.0)
            .child(
                AxNode::new(
                    "a",
                    super::super::AxRole::StaticText,
                    "A",
                    AxRect::default(),
                )
                .value("two"),
            )
            .child(
                AxNode::new(
                    "b",
                    super::super::AxRole::StaticText,
                    "B",
                    AxRect::default(),
                )
                .value("new"),
            );
        let mut changed = changed_value_identifiers(&old, &new);
        changed.sort();
        assert_eq!(changed, vec!["a", "b"]);
    }

    #[test]
    fn disabled_element_rejects_native_action_dispatch() {
        let calls = Rc::new(Cell::new(0));
        let witness = Rc::clone(&calls);
        let state = ElementState {
            identifier: "send".into(),
            actions: vec![AxAction::Press],
            enabled: false,
            handler: Rc::new(move |_| witness.set(witness.get() + 1)),
        };
        assert!(!state.send(AxAction::Press, None));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn same_skeleton_tracks_structure_not_property_changes() {
        let base = || {
            AxNode::new(
                "send",
                super::super::AxRole::Button,
                "Send",
                AxRect::new(0.0, 0.0, 10.0, 10.0),
            )
            .action(AxAction::Press)
        };
        // label/value/focused/bounds 等属性变化不算结构变化。
        let changed_props = base().value("v").focused(true);
        assert!(same_skeleton(&[base()], &[changed_props]));
        // role、press 能力、子树形状变化都算结构变化。
        let text = AxNode::new(
            "send",
            super::super::AxRole::StaticText,
            "Send",
            AxRect::default(),
        );
        assert!(!same_skeleton(&[base()], &[text]));
        assert!(!same_skeleton(&[base()], &[base().enabled(false)]));
        let with_child = base().child(AxNode::new(
            "child",
            super::super::AxRole::StaticText,
            "C",
            AxRect::default(),
        ));
        assert!(!same_skeleton(&[base()], &[with_child]));
    }

    #[test]
    fn native_dispatch_honors_focus_gate() {
        let calls = Rc::new(Cell::new(0));
        let witness = Rc::clone(&calls);
        let state = Box::new(ElementState {
            identifier: "composer-input".into(),
            // 只允许 SetValue，不允许 Focus。
            actions: vec![AxAction::SetValue],
            enabled: true,
            handler: Rc::new(move |_| witness.set(witness.get() + 1)),
        });
        let state = Box::into_raw(state) as *mut c_void;
        unsafe {
            let object: id = msg_send![element_class(), alloc];
            let object: id = msg_send![object, init];
            (*object).set_ivar(ELEMENT_STATE_IVAR, state);
            element_set_focused(&mut *object, sel!(setAccessibilityFocused:), YES);
            assert_eq!(calls.get(), 0);
            let _: () = msg_send![object, release];
        }
    }
}
