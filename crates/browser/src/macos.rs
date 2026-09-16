#![allow(unexpected_cfgs)] // objc 0.2 macros use the historical cargo-clippy cfg.

use std::{
    cell::RefCell,
    ffi::{CStr, c_void},
    marker::PhantomData,
    rc::Rc,
    sync::OnceLock,
};

use block::ConcreteBlock;
use cocoa::{
    base::{id, nil},
    foundation::{NSPoint, NSRect, NSSize, NSString},
};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{BOOL, Class, NO, Object, Sel, YES},
    sel, sel_impl,
};
use raw_window_handle::{RawWindowHandle, WindowHandle};

use super::{BrowserState, dom, navigation_allowed, normalize_url};

#[link(name = "WebKit", kind = "framework")]
extern "C" {}

const ERROR_IVAR: &str = "paworkBrowserError";
type ErrorState = RefCell<Option<String>>;

unsafe fn string(value: id) -> String {
    if value == nil {
        return String::new();
    }
    let ptr: *const std::ffi::c_char = msg_send![value, UTF8String];
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

unsafe fn address(request: id) -> String {
    let url: id = msg_send![request, URL];
    let absolute: id = msg_send![url, absoluteString];
    string(absolute)
}

unsafe fn set_error(this: &Object, value: Option<String>) {
    let ptr = *this.get_ivar::<*mut c_void>(ERROR_IVAR) as *const ErrorState;
    if let Some(state) = ptr.as_ref() {
        if let Ok(mut error) = state.try_borrow_mut() {
            *error = value;
        }
    }
}

extern "C" fn decide_navigation(this: &Object, _: Sel, webview: id, action: id, handler: id) {
    unsafe {
        let request: id = msg_send![action, request];
        let allowed = navigation_allowed(&address(request));
        let target: id = msg_send![action, targetFrame];
        let download: BOOL = msg_send![action, shouldPerformDownload];
        let handler = &*(handler as *const block::Block<(isize,), ()>);
        if !allowed || download == YES {
            set_error(
                this,
                Some("不支持此导航或下载 / This navigation or download is not supported".into()),
            );
            handler.call((0,));
        } else if target == nil {
            // Keep new-window links inside the current task's page.
            let _: id = msg_send![request, retain];
            handler.call((0,));
            let _: id = msg_send![webview, loadRequest: request];
            let _: () = msg_send![request, release];
        } else {
            handler.call((1,));
        }
    }
}

extern "C" fn decide_response(this: &Object, _: Sel, _: id, response: id, handler: id) {
    unsafe {
        let can_show: BOOL = msg_send![response, canShowMIMEType];
        let url_response: id = msg_send![response, response];
        let allowed = navigation_allowed(&address(url_response));
        if can_show == NO || !allowed {
            set_error(
                this,
                Some("暂不支持下载 / Downloads are not supported yet".into()),
            );
        }
        (&*(handler as *const block::Block<(isize,), ()>)).call((if can_show == YES && allowed {
            1
        } else {
            0
        },));
    }
}

extern "C" fn started(this: &Object, _: Sel, _: id, _: id) {
    unsafe {
        set_error(this, None);
    }
}

extern "C" fn failed(this: &Object, _: Sel, _: id, _: id, error: id) {
    unsafe {
        let code: isize = msg_send![error, code];
        if code != -999 {
            let description: id = msg_send![error, localizedDescription];
            set_error(this, Some(string(description)));
        }
    }
}

extern "C" fn terminated(this: &Object, _: Sel, _: id) {
    unsafe {
        set_error(
            this,
            Some("网页进程已退出，请刷新 / Web content process exited; reload to retry".into()),
        );
    }
}

extern "C" fn create_webview(this: &Object, _: Sel, webview: id, _: id, action: id, _: id) -> id {
    unsafe {
        let request: id = msg_send![action, request];
        if navigation_allowed(&address(request)) {
            let _: id = msg_send![webview, loadRequest: request];
        } else {
            set_error(
                this,
                Some("不支持此链接 / This link is not supported".into()),
            );
        }
    }
    nil
}

fn delegate_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| unsafe {
        let mut decl = ClassDecl::new("PaworkBrowserDelegate", class!(NSObject))
            .expect("browser delegate class");
        decl.add_ivar::<*mut c_void>(ERROR_IVAR);
        decl.add_method(
            sel!(webView:decidePolicyForNavigationAction:decisionHandler:),
            decide_navigation as extern "C" fn(&Object, Sel, id, id, id),
        );
        decl.add_method(
            sel!(webView:decidePolicyForNavigationResponse:decisionHandler:),
            decide_response as extern "C" fn(&Object, Sel, id, id, id),
        );
        decl.add_method(
            sel!(webView:didStartProvisionalNavigation:),
            started as extern "C" fn(&Object, Sel, id, id),
        );
        decl.add_method(
            sel!(webView:didFailProvisionalNavigation:withError:),
            failed as extern "C" fn(&Object, Sel, id, id, id),
        );
        decl.add_method(
            sel!(webView:didFailNavigation:withError:),
            failed as extern "C" fn(&Object, Sel, id, id, id),
        );
        decl.add_method(
            sel!(webViewWebContentProcessDidTerminate:),
            terminated as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:),
            create_webview as extern "C" fn(&Object, Sel, id, id, id, id) -> id,
        );
        decl.register()
    })
}

/// Main-thread-only owner of a system WebKit view. The borrowed parent is retained.
pub struct BrowserView {
    view: id,
    parent: id,
    delegate: id,
    error: Box<ErrorState>,
    _main_thread: PhantomData<Rc<()>>,
}

impl BrowserView {
    pub fn new(parent: WindowHandle<'_>) -> Result<Self, String> {
        let RawWindowHandle::AppKit(handle) = parent.as_raw() else {
            return Err("浏览器需要 AppKit 窗口 / Browser requires an AppKit window".into());
        };
        unsafe {
            let main: BOOL = msg_send![class!(NSThread), isMainThread];
            if main != YES {
                return Err("Browser must be created on the AppKit main thread".into());
            }
            let parent = handle.ns_view.as_ptr() as id;
            let config: id = msg_send![class!(WKWebViewConfiguration), new];
            let store: id = msg_send![class!(WKWebsiteDataStore), nonPersistentDataStore];
            let _: () = msg_send![config, setWebsiteDataStore: store];
            let preferences: id = msg_send![config, preferences];
            let _: () = msg_send![preferences, setJavaScriptCanOpenWindowsAutomatically: NO];
            let _: () = msg_send![preferences, setTabFocusesLinks: YES];
            let view: id = msg_send![class!(WKWebView), alloc];
            let view: id = msg_send![view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)) configuration: config];
            let _: () = msg_send![config, release];
            if view == nil {
                return Err("Unable to create the system WebKit view".into());
            }
            let error = Box::new(RefCell::new(None));
            let delegate: id = msg_send![delegate_class(), new];
            (*delegate).set_ivar(ERROR_IVAR, (&*error as *const ErrorState) as *mut c_void);
            let _: () = msg_send![view, setNavigationDelegate: delegate];
            let _: () = msg_send![view, setUIDelegate: delegate];
            let _: () = msg_send![view, setHidden: YES];
            let _: id = msg_send![parent, retain];
            let _: () = msg_send![parent, addSubview: view];
            Ok(Self {
                view,
                parent,
                delegate,
                error,
                _main_thread: PhantomData,
            })
        }
    }

    pub fn navigate(&self, input: &str) -> Result<(), String> {
        let url = normalize_url(input)?;
        unsafe {
            let text = NSString::alloc(nil).init_str(&url);
            let address: id = msg_send![class!(NSURL), URLWithString: text];
            let _: () = msg_send![text, release];
            if address == nil {
                return Err("Invalid website address".into());
            }
            *self.error.borrow_mut() = None;
            let request: id = msg_send![class!(NSURLRequest), requestWithURL: address];
            let _: id = msg_send![self.view, loadRequest: request];
        }
        Ok(())
    }

    pub fn state(&self) -> BrowserState {
        unsafe {
            let url: id = msg_send![self.view, URL];
            let address: id = msg_send![url, absoluteString];
            let title: id = msg_send![self.view, title];
            let loading: BOOL = msg_send![self.view, isLoading];
            let back: BOOL = msg_send![self.view, canGoBack];
            let forward: BOOL = msg_send![self.view, canGoForward];
            BrowserState {
                url: string(address),
                title: string(title),
                loading: loading == YES,
                can_go_back: back == YES,
                can_go_forward: forward == YES,
                error: self.error.borrow().clone(),
            }
        }
    }

    pub fn back(&self) {
        unsafe {
            let _: id = msg_send![self.view, goBack];
        }
    }
    pub fn forward(&self) {
        unsafe {
            let _: id = msg_send![self.view, goForward];
        }
    }
    pub fn reload(&self) {
        unsafe {
            *self.error.borrow_mut() = None;
            let _: id = msg_send![self.view, reload];
        }
    }
    pub fn stop(&self) {
        unsafe {
            let _: () = msg_send![self.view, stopLoading];
        }
    }

    pub fn set_bounds(&self, x: f64, y: f64, width: f64, height: f64) {
        if ![x, y, width, height].into_iter().all(f64::is_finite) {
            return;
        }
        unsafe {
            let bounds: NSRect = msg_send![self.parent, bounds];
            let flipped: BOOL = msg_send![self.parent, isFlipped];
            let y = if flipped == YES {
                y
            } else {
                bounds.size.height - y - height
            };
            let _: () = msg_send![self.view, setFrame: NSRect::new(NSPoint::new(x, y), NSSize::new(width.max(0.0), height.max(0.0)))];
        }
    }

    pub fn is_focused(&self) -> bool {
        unsafe {
            let window: id = msg_send![self.parent, window];
            let responder: id = msg_send![window, firstResponder];
            if responder == nil {
                return false;
            }
            let is_view: BOOL = msg_send![responder, isKindOfClass: class!(NSView)];
            is_view == YES && {
                let descendant: BOOL = msg_send![responder, isDescendantOf: self.view];
                descendant == YES
            }
        }
    }

    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let hidden: BOOL = msg_send![self.view, isHidden];
            if (hidden == NO) == visible {
                return;
            }
            if !visible && self.is_focused() {
                self.focus_parent();
            }
            let _: () = msg_send![self.view, setHidden: if visible { NO } else { YES }];
        }
    }

    /// Return keyboard input to the host UI without hiding or recreating the page.
    pub fn focus_parent(&self) {
        unsafe {
            let window: id = msg_send![self.parent, window];
            let _: BOOL = msg_send![window, makeFirstResponder: self.parent];
        }
    }

    pub fn read_page(&self, callback: impl FnOnce(Result<String, String>) + 'static) {
        let state = self.state();
        self.evaluate(
            dom::read_page_script(),
            move |raw| {
                let data = dom::decode_envelope(&raw)?;
                dom::finalize_page_json(data, &state.url, &state.title)
            },
            callback,
        );
    }

    pub fn click(&self, selector: &str, callback: impl FnOnce(Result<String, String>) + 'static) {
        if let Err(error) = dom::require_selector(selector) {
            callback(Err(error));
            return;
        }
        self.evaluate(
            dom::click_script(selector),
            |raw| dom::encode_success(dom::decode_envelope(&raw)?),
            callback,
        );
    }

    pub fn type_text(
        &self,
        selector: &str,
        text: &str,
        callback: impl FnOnce(Result<String, String>) + 'static,
    ) {
        if let Err(error) = dom::require_selector(selector) {
            callback(Err(error));
            return;
        }
        self.evaluate(
            dom::type_text_script(selector, text),
            |raw| dom::encode_success(dom::decode_envelope(&raw)?),
            callback,
        );
    }

    fn evaluate(
        &self,
        script: String,
        map: impl FnOnce(String) -> Result<String, String> + 'static,
        callback: impl FnOnce(Result<String, String>) + 'static,
    ) {
        let done = RefCell::new(Some((map, callback)));
        unsafe {
            let text = NSString::alloc(nil).init_str(&script);
            let block = ConcreteBlock::new(move |result: id, error: id| {
                let raw = {
                    if error != nil {
                        let description: id = msg_send![error, localizedDescription];
                        Err(string(description))
                    } else {
                        Ok(string(result))
                    }
                };
                if let Some((map, callback)) = done.borrow_mut().take() {
                    callback(raw.and_then(map));
                }
            });
            let block = block.copy();
            let _: () = msg_send![self.view, evaluateJavaScript: text completionHandler: &*block];
            let _: () = msg_send![text, release];
        }
    }
}

impl Drop for BrowserView {
    fn drop(&mut self) {
        self.set_visible(false);
        unsafe {
            let _: () = msg_send![self.view, setNavigationDelegate: nil];
            let _: () = msg_send![self.view, setUIDelegate: nil];
            self.stop();
            let _: () = msg_send![self.view, removeFromSuperview];
            let _: () = msg_send![self.view, release];
            (*self.delegate).set_ivar(ERROR_IVAR, std::ptr::null_mut::<c_void>());
            let _: () = msg_send![self.delegate, release];
            let _: () = msg_send![self.parent, release];
        }
    }
}
