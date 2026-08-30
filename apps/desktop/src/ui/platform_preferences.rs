//! macOS Accessibility display preferences used by the Desktop renderer.
//!
//! Pawork currently has no motion effects, so Reduce Motion needs no render
//! branch. Increase Contrast does affect the palette and is refreshed from the
//! workspace notification without introducing a new runtime dependency.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, Window};

static INCREASE_CONTRAST: AtomicBool = AtomicBool::new(false);

pub(crate) fn increase_contrast() -> bool {
    INCREASE_CONTRAST.load(Ordering::Acquire)
}

#[cfg(target_os = "macos")]
pub(crate) fn install(window: &Window, cx: &App) {
    macos::install(window, cx);
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn install(_window: &Window, _cx: &App) {}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::sync::Once;

    use cocoa::base::{id, nil};
    use gpui::AsyncWindowContext;
    use objc::{class, msg_send, sel, sel_impl};

    use super::{App, Ordering, Window, INCREASE_CONTRAST};

    const BLOCK_IS_GLOBAL: i32 = 1 << 28;

    thread_local! {
        static PREFERENCES_WINDOW: RefCell<Option<AsyncWindowContext>> =
            const { RefCell::new(None) };
    }

    #[repr(C)]
    struct BlockDescriptor {
        reserved: usize,
        size: usize,
    }

    #[repr(C)]
    struct DisplayOptionsBlock {
        isa: *const objc::runtime::Class,
        flags: i32,
        reserved: i32,
        invoke: unsafe extern "C" fn(*mut DisplayOptionsBlock, id),
        descriptor: *const BlockDescriptor,
    }

    unsafe extern "C" {
        static _NSConcreteGlobalBlock: objc::runtime::Class;
        static NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification: id;
    }

    unsafe fn system_increase_contrast() -> bool {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let enabled: objc::runtime::BOOL =
            msg_send![workspace, accessibilityDisplayShouldIncreaseContrast];
        enabled
    }

    fn refresh() {
        let enabled = unsafe { system_increase_contrast() };
        INCREASE_CONTRAST.store(enabled, Ordering::Release);
    }

    unsafe extern "C" fn display_options_changed(
        _block: *mut DisplayOptionsBlock,
        _notification: id,
    ) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            refresh();
            PREFERENCES_WINDOW.with_borrow_mut(|slot| {
                if let Some(cx) = slot.as_mut() {
                    let _ = cx.update(|window, _| window.refresh());
                }
            });
        }));
        if let Err(panic) = result {
            eprintln!("[platform-preferences] refresh failed: {panic:?}");
        }
    }

    pub(super) fn install(window: &Window, cx: &App) {
        PREFERENCES_WINDOW.with_borrow_mut(|slot| *slot = Some(window.to_async(cx)));
        refresh();

        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            let descriptor: &'static BlockDescriptor = Box::leak(Box::new(BlockDescriptor {
                reserved: 0,
                size: std::mem::size_of::<DisplayOptionsBlock>(),
            }));
            let block: &'static DisplayOptionsBlock = Box::leak(Box::new(DisplayOptionsBlock {
                isa: std::ptr::addr_of!(_NSConcreteGlobalBlock),
                flags: BLOCK_IS_GLOBAL,
                reserved: 0,
                invoke: display_options_changed,
                descriptor,
            }));
            unsafe {
                let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
                let center: id = msg_send![workspace, notificationCenter];
                let main_queue: id = msg_send![class!(NSOperationQueue), mainQueue];
                let block_ptr = block as *const DisplayOptionsBlock as id;
                let _: id = msg_send![center,
                    addObserverForName: NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification
                    object: nil
                    queue: main_queue
                    usingBlock: block_ptr
                ];
            }
        });
    }
}
