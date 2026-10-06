//! CU-01 人工探针：macOS 本机后台操作可行性（不属于库本体，不参与产品路径）。
//!
//! 只用公开 API：ScreenCaptureKit（SCScreenshotManager 单帧窗口截图）、
//! HIServices AX（读取 / AXPress / AXSetValue）、CoreGraphics
//! CGEventPostToPid（定向按键 / 指针）。每条命令输出一行 JSON，供 shell
//! 编排「应用 x 动作 x 窗口状态」矩阵；watch 命令在用户侧独立采样前台应用、
//! 键盘焦点、鼠标位置与剪贴板，作为无干扰判定的独立事实。
//!
//! AX 符号经 dlopen/dlsym 解析：HIServices 是 ApplicationServices 的子框架，
//! 不在 ld 默认搜索路径，避免为探针改动构建配置。AX 属性名按字符串值传递
//! （AX API 按字符串比较属性名，不依赖导出常量指针）。
//!
//! 运行前提：Screen Recording 与 Accessibility 授权（按责任进程归属）。
//! 全部命令默认在主线程执行；加 --spawn 改到新建线程执行，用于核对线程要求。
//!
//! 用法：
//!   perms                        当前进程 TCC 状态（不发 prompt）
//!   wins <pid>                   SCShareableContent 窗口列表（pid=0 列全部）
//!   capture <window_id> <png>    SCScreenshotManager 单帧窗口截图
//!   axdump <pid>                 AX 概览：窗口/焦点/节点统计/Web 区
//!   axread <pid> [full]          读首个文本区 AXValue（优先焦点元素）；full 输出全文
//!   axset <pid> <text>           语义写：AXSetValue 替换文本区内容
//!   axinsert <pid> <text>        语义写：AXSelectedText 在光标处插入
//!   axpress <pid> <needle>       语义点：AXPress 匹配标题/描述的控件
//!   axfind <pid> <needle>        只定位可压控件（返回 role/title/frame，不按压）
//!   axmanual <pid> <0|1>         设置 AXManualAccessibility（Chromium 系）
//!   axmin <pid> <0|1> [title]    设置窗口 AXMinimized（可按标题子串选窗口）
//!   keys <pid> <text>            CGEventPostToPid 定向 Unicode 输入
//!   key <pid> <code> [mods]      定向单键（mods: cmd,shift,ctrl,alt）
//!   click <pid> <x> <y>          CGEventPostToPid 定向指针点击（全局坐标）
//!   activate <pid>               激活应用（仅用于模拟「用户侧」切换）
//!   usertype <pid> <text>        模拟用户：激活 + 会话级真实键盘事件
//!   sessiontype <text>           会话级输入（不切换前台，目标需已有焦点）
//!   watch <ms>                   用户侧采样，状态变化即输出 JSONL

#[cfg(target_os = "macos")]
mod imp {
    use block::ConcreteBlock;
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::CFURL;
    use core_graphics::access::ScreenCaptureAccess;
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton,
    };
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use objc::rc::autoreleasepool;
    use objc::runtime::{Class, Object, BOOL, NO, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::ffi::{c_void, CStr, CString};
    use std::os::raw::c_char;
    use std::path::Path;
    use std::ptr;
    use std::sync::{Arc, Condvar, Mutex, OnceLock};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    type AXEl = *mut c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFRetain(p: *const c_void) -> *const c_void;
        fn CFRelease(p: *const c_void);
        fn CFGetTypeID(p: *const c_void) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(b: *const c_void) -> u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *const c_void) -> CGPoint;
        fn CGMainDisplayID() -> u32;
        fn CGImageGetWidth(image: *const c_void) -> usize;
        fn CGImageGetHeight(image: *const c_void) -> usize;
    }

    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageDestinationCreateWithURL(
            url: *const c_void,
            ty: CFStringRef,
            count: usize,
            options: *const c_void,
        ) -> *mut c_void;
        fn CGImageDestinationAddImage(dest: *mut c_void, image: *const c_void, props: *const c_void);
        fn CGImageDestinationFinalize(dest: *mut c_void) -> bool;
    }

    // ---------- HIServices（AX）运行时解析 ----------

    struct AxFns {
        is_trusted: unsafe extern "C" fn(*const c_void) -> bool,
        create_application: unsafe extern "C" fn(libc::pid_t) -> AXEl,
        create_system_wide: unsafe extern "C" fn() -> AXEl,
        copy_attribute: unsafe extern "C" fn(AXEl, CFStringRef, *mut CFTypeRef) -> i32,
        set_attribute: unsafe extern "C" fn(AXEl, CFStringRef, CFTypeRef) -> i32,
        perform_action: unsafe extern "C" fn(AXEl, CFStringRef) -> i32,
        get_pid: unsafe extern "C" fn(AXEl, *mut libc::pid_t) -> i32,
        value_get_value: unsafe extern "C" fn(CFTypeRef, i32, *mut c_void) -> bool,
        value_type_id: unsafe extern "C" fn() -> usize,
        value_create: unsafe extern "C" fn(i32, *const c_void) -> CFTypeRef,
    }

    fn axf() -> &'static AxFns {
        static AXF: OnceLock<AxFns> = OnceLock::new();
        AXF.get_or_init(|| unsafe {
            let path = b"/System/Library/Frameworks/ApplicationServices.framework/Frameworks/HIServices.framework/HIServices\0";
            let h = libc::dlopen(path.as_ptr() as *const c_char, libc::RTLD_LAZY);
            assert!(!h.is_null(), "dlopen HIServices failed");
            unsafe fn load<T>(h: *mut c_void, name: &str) -> T {
                let c = CString::new(name).unwrap();
                let p = libc::dlsym(h, c.as_ptr());
                assert!(!p.is_null(), "dlsym {} failed", name);
                std::mem::transmute_copy(&p)
            }
            AxFns {
                is_trusted: load(h, "AXIsProcessTrustedWithOptions"),
                create_application: load(h, "AXUIElementCreateApplication"),
                create_system_wide: load(h, "AXUIElementCreateSystemWide"),
                copy_attribute: load(h, "AXUIElementCopyAttributeValue"),
                set_attribute: load(h, "AXUIElementSetAttributeValue"),
                perform_action: load(h, "AXUIElementPerformAction"),
                get_pid: load(h, "AXUIElementGetPid"),
                value_get_value: load(h, "AXValueGetValue"),
                value_type_id: load(h, "AXValueGetTypeID"),
                value_create: load(h, "AXValueCreate"),
            }
        })
    }

    fn ax_err(code: i32) -> String {
        let name = match code {
            0 => "success",
            -25200 => "failure",
            -25201 => "illegal_argument",
            -25202 => "invalid_ui_element",
            -25204 => "cannot_complete",
            -25205 => "attribute_unsupported",
            -25206 => "action_unsupported",
            -25208 => "not_implemented",
            -25211 => "api_disabled",
            -25212 => "no_value",
            -25213 => "parameterized_attribute_unsupported",
            _ => "unknown",
        };
        format!("{}({})", name, code)
    }

    fn now_ms() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    fn nsstr(s: &str) -> *mut Object {
        let c = CString::new(s).unwrap();
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    fn rs_str(ns: *mut Object) -> String {
        if ns.is_null() {
            return String::new();
        }
        unsafe {
            let p: *const c_char = msg_send![ns, UTF8String];
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    fn err_desc(err: *mut Object) -> String {
        if err.is_null() {
            return "unknown nil-context error".into();
        }
        unsafe { rs_str(msg_send![err, localizedDescription]) }
    }

    // ---------- ScreenCaptureKit ----------

    /// 探针不静态链接 ScreenCaptureKit；类查找前先 dlopen 加载框架。
    fn ensure_sck() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            // SCScreenshotManager 依赖 WindowServer 连接（CGS），未初始化会在
            // capture 时断言 did_initialize；先触发一次 CG 调用完成初始化。
            CGMainDisplayID();
            let path = b"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit ";
            let h = libc::dlopen(path.as_ptr() as *const c_char, libc::RTLD_LAZY);
            assert!(!h.is_null(), "dlopen ScreenCaptureKit failed");
        });
    }

    /// 等待一次 ObjC completion handler；返回 (object_ptr, err_string)。
    /// 成功对象由回调内 retain 保证生命周期；NSError 在回调内转成 String，
    /// 不向等待方外传裸指针（回调 autoreleasepool 排空后会悬空）。
    fn wait_block(
        pair: &Arc<(Mutex<(usize, Option<String>, bool)>, Condvar)>,
    ) -> Result<(usize, Option<String>), String> {
        let (lock, cv) = &**pair;
        let guard = lock.lock().map_err(|_| "poisoned")?;
        // wait_timeout_while 检查完成标志，早到通知与虚假唤醒都不会误报超时。
        let (guard, timeout) = cv
            .wait_timeout_while(guard, Duration::from_secs(20), |g| !g.2)
            .map_err(|_| "poisoned")?;
        if timeout.timed_out() {
            return Err("completion handler timeout (20s)".into());
        }
        Ok((guard.0, guard.1.clone()))
    }

    fn share_content() -> Result<*mut Object, String> {
        ensure_sck();
        unsafe {
            let cls = Class::get("SCShareableContent").ok_or("SCShareableContent class missing")?;
            let pair = Arc::new((Mutex::new((0usize, None, false)), Condvar::new()));
            let pair2 = pair.clone();
            let block = ConcreteBlock::new(move |content: *mut Object, err: *mut Object| {
                // 回调线程的 autoreleasepool 在 handler 返回后即排空：
                // 成功对象回调内 retain，NSError 回调内转成 String。
                if !content.is_null() {
                    let _: *mut Object = msg_send![content, retain];
                }
                let err_s = if err.is_null() { None } else { Some(err_desc(err)) };
                let (lock, cv) = &*pair2;
                if let Ok(mut g) = lock.lock() {
                    *g = (content as usize, err_s, true);
                    cv.notify_one();
                }
            });
            let block = block.copy();
            let modern: BOOL = msg_send![
                cls,
                respondsToSelector: sel!(getShareableContentExcludingDesktopWindows:onScreenWindowsOnly:completionHandler:)
            ];
            if modern == YES {
                let _: () = msg_send![cls,
                    getShareableContentExcludingDesktopWindows: NO
                    onScreenWindowsOnly: NO
                    completionHandler: &*block];
            } else {
                let _: () = msg_send![cls, getShareableContentWithCompletionHandler: &*block];
            }
            let (content, err) = wait_block(&pair)?;
            if let Some(e) = err {
                return Err(format!("SCShareableContent: {}", e));
            }
            if content == 0 {
                return Err("SCShareableContent returned nil".into());
            }
            Ok(content as *mut Object)
        }
    }

    fn win_json(w: *mut Object) -> Value {
        unsafe {
            let wid: u32 = msg_send![w, windowID];
            let title: *mut Object = msg_send![w, title];
            let frame: CGRect = msg_send![w, frame];
            let on_screen: BOOL = msg_send![w, isOnScreen];
            let layer: i64 = msg_send![w, windowLayer];
            let app: *mut Object = msg_send![w, owningApplication];
            let (pid, app_name, bundle) = if app.is_null() {
                (0, String::new(), String::new())
            } else {
                let pid: i32 = msg_send![app, processID];
                let name: *mut Object = msg_send![app, applicationName];
                let bid: *mut Object = msg_send![app, bundleIdentifier];
                (pid, rs_str(name), rs_str(bid))
            };
            json!({
                "window_id": wid,
                "pid": pid,
                "app": app_name,
                "bundle": bundle,
                "title": rs_str(title),
                "on_screen": on_screen == YES,
                "layer": layer,
                "frame": {
                    "x": frame.origin.x, "y": frame.origin.y,
                    "w": frame.size.width, "h": frame.size.height
                }
            })
        }
    }

    fn find_sc_window(content: *mut Object, wid: u32) -> Option<*mut Object> {
        unsafe {
            let windows: *mut Object = msg_send![content, windows];
            if windows.is_null() {
                return None;
            }
            let n: usize = msg_send![windows, count];
            for i in 0..n {
                let w: *mut Object = msg_send![windows, objectAtIndex: i];
                let id32: u32 = msg_send![w, windowID];
                if id32 == wid {
                    let _: *mut Object = msg_send![w, retain];
                    return Some(w);
                }
            }
            None
        }
    }

    fn save_png(image: *const c_void, path: &str) -> Result<(), String> {
        let url = CFURL::from_path(Path::new(path), false).ok_or("CFURL create failed")?;
        let ty = CFString::new("public.png");
        unsafe {
            let dest = CGImageDestinationCreateWithURL(
                url.as_concrete_TypeRef() as *const c_void,
                ty.as_concrete_TypeRef(),
                1,
                ptr::null(),
            );
            if dest.is_null() {
                return Err("CGImageDestinationCreateWithURL failed".into());
            }
            CGImageDestinationAddImage(dest, image, ptr::null());
            let ok = CGImageDestinationFinalize(dest);
            CFRelease(dest);
            if ok {
                Ok(())
            } else {
                Err("CGImageDestinationFinalize failed".into())
            }
        }
    }

    fn cmd_capture(wid: u32, out: &str) -> Result<Value, String> {
        ensure_sck();
        unsafe {
            let content = share_content()?;
            let w = find_sc_window(content, wid).ok_or(format!("window {} not found", wid))?;
            let frame: CGRect = msg_send![w, frame];
            let on_screen: BOOL = msg_send![w, isOnScreen];
            let filter: *mut Object = msg_send![class!(SCContentFilter), alloc];
            let filter: *mut Object = msg_send![filter, initWithDesktopIndependentWindow: w];
            if filter.is_null() {
                return Err("SCContentFilter init returned nil".into());
            }
            let cfg: *mut Object = msg_send![class!(SCStreamConfiguration), alloc];
            let cfg: *mut Object = msg_send![cfg, init];
            let wpx = (frame.size.width * 2.0).clamp(1.0, 8192.0) as usize;
            let hpx = (frame.size.height * 2.0).clamp(1.0, 8192.0) as usize;
            let _: () = msg_send![cfg, setWidth: wpx];
            let _: () = msg_send![cfg, setHeight: hpx];
            let _: () = msg_send![cfg, setShowsCursor: NO];
            let _: () = msg_send![cfg, setPixelFormat: 0x42475241u32];
            let has_res: BOOL = msg_send![cfg, respondsToSelector: sel!(setCaptureResolution:)];
            if has_res == YES {
                let _: () = msg_send![cfg, setCaptureResolution: 1i64];
            }
            let mgr = Class::get("SCScreenshotManager")
                .ok_or("SCScreenshotManager missing (needs macOS 14+)")?;
            let pair = Arc::new((Mutex::new((0usize, None, false)), Condvar::new()));
            let pair2 = pair.clone();
            let block = ConcreteBlock::new(move |image: *mut c_void, err: *mut Object| {
                let err_s = if err.is_null() { None } else { Some(err_desc(err)) };
                let (lock, cv) = &*pair2;
                if let Ok(mut g) = lock.lock() {
                    if !image.is_null() {
                        CFRetain(image);
                    }
                    *g = (image as usize, err_s, true);
                    cv.notify_one();
                }
            });
            let block = block.copy();
            let _: () = msg_send![mgr,
                captureImageWithFilter: filter
                configuration: cfg
                completionHandler: &*block];
            let (image, err) = wait_block(&pair)?;
            if let Some(e) = err {
                return Err(format!("capture: {}", e));
            }
            if image == 0 {
                return Err("capture returned nil image".into());
            }
            let image = image as *const c_void;
            let width = CGImageGetWidth(image);
            let height = CGImageGetHeight(image);
            save_png(image, out)?;
            let bytes = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
            CFRelease(image);
            Ok(json!({
                "width": width, "height": height, "bytes": bytes, "path": out,
                "window_on_screen": on_screen == YES,
                "requested_px": [wpx, hpx]
            }))
        }
    }

    // ---------- Accessibility ----------

    fn ax_app(pid: i32) -> AXEl {
        unsafe { (axf().create_application)(pid) }
    }

    unsafe fn ax_copy(el: AXEl, attr: &str) -> Result<CFTypeRef, i32> {
        let name = CFString::new(attr);
        let mut out: CFTypeRef = ptr::null();
        let e = (axf().copy_attribute)(el, name.as_concrete_TypeRef(), &mut out);
        if e == 0 && !out.is_null() {
            Ok(out)
        } else {
            Err(e)
        }
    }

    unsafe fn ax_set(el: AXEl, attr: &str, value: CFTypeRef) -> i32 {
        let name = CFString::new(attr);
        (axf().set_attribute)(el, name.as_concrete_TypeRef(), value)
    }

    unsafe fn ax_perform(el: AXEl, action: &str) -> i32 {
        let name = CFString::new(action);
        (axf().perform_action)(el, name.as_concrete_TypeRef())
    }

    /// 消费 +1 引用并尽力转字符串：CFString 直转；NSAttributedString 走 objc string 选择器。
    unsafe fn cf_to_string(v: CFTypeRef) -> Option<String> {
        if v.is_null() {
            return None;
        }
        let tid = CFGetTypeID(v);
        if tid == CFStringGetTypeID() {
            let s = CFString::wrap_under_create_rule(v as CFStringRef);
            return Some(s.to_string());
        }
        if tid == (axf().value_type_id)() {
            CFRelease(v);
            return None;
        }
        let obj = v as *mut Object;
        let has: BOOL = msg_send![obj, respondsToSelector: sel!(string)];
        let out = if has == YES {
            let s: *mut Object = msg_send![obj, string];
            Some(rs_str(s))
        } else {
            let has_desc: BOOL = msg_send![obj, respondsToSelector: sel!(description)];
            if has_desc == YES {
                let s: *mut Object = msg_send![obj, description];
                Some(rs_str(s))
            } else {
                None
            }
        };
        CFRelease(v);
        out
    }

    unsafe fn ax_str(el: AXEl, attr: &str) -> Option<String> {
        match ax_copy(el, attr) {
            Ok(v) => cf_to_string(v),
            Err(_) => None,
        }
    }

    unsafe fn ax_role(el: AXEl) -> String {
        ax_str(el, "AXRole").unwrap_or_default()
    }

    unsafe fn ax_bool_of(v: CFTypeRef) -> Option<bool> {
        if v.is_null() {
            return None;
        }
        let out = if CFGetTypeID(v) == CFBooleanGetTypeID() {
            Some(CFBooleanGetValue(v) != 0)
        } else {
            None
        };
        CFRelease(v);
        out
    }

    unsafe fn ax_bool_attr(el: AXEl, attr: &str) -> Result<Option<bool>, i32> {
        match ax_copy(el, attr) {
            Ok(v) => Ok(ax_bool_of(v)),
            Err(e) => Err(e),
        }
    }

    unsafe fn ax_list(el: AXEl, attr: &str) -> Vec<AXEl> {
        match ax_copy(el, attr) {
            Ok(arr) => {
                let n = CFArrayGetCount(arr as *const c_void);
                let mut out = Vec::new();
                for i in 0..n {
                    let c = CFArrayGetValueAtIndex(arr as *const c_void, i);
                    if !c.is_null() {
                        CFRetain(c);
                        out.push(c as AXEl);
                    }
                }
                CFRelease(arr as *const c_void);
                out
            }
            Err(_) => Vec::new(),
        }
    }

    unsafe fn ax_frame(el: AXEl) -> Value {
        // AXValueType：kAXValueTypeCGPoint=1, CGSize=2, CGRect=3
        let pos = match ax_copy(el, "AXPosition") {
            Ok(v) => {
                let mut p = CGPoint::new(0.0, 0.0);
                let ok = (axf().value_get_value)(v, 1, &mut p as *mut CGPoint as *mut c_void);
                CFRelease(v);
                if ok { Some(p) } else { None }
            }
            Err(_) => None,
        };
        let size = match ax_copy(el, "AXSize") {
            Ok(v) => {
                let mut s = CGSize::new(0.0, 0.0);
                let ok = (axf().value_get_value)(v, 2, &mut s as *mut CGSize as *mut c_void);
                CFRelease(v);
                if ok { Some(s) } else { None }
            }
            Err(_) => None,
        };
        match (pos, size) {
            (Some(p), Some(s)) => json!({"x": p.x, "y": p.y, "w": s.width, "h": s.height}),
            _ => Value::Null,
        }
    }

    const TEXT_ROLES: [&str; 2] = ["AXTextArea", "AXTextField"];
    const PRESS_ROLES: [&str; 7] = [
        "AXButton",
        "AXLink",
        "AXCheckBox",
        "AXRadioButton",
        "AXPopUpButton",
        "AXMenuItem",
        "AXTab",
    ];

    /// BFS（仅沿 AXChildren 下探），命中即返回已 retain 的元素与路径深度。
    unsafe fn bfs_find(
        roots: &[AXEl],
        max_nodes: usize,
        pred: &dyn Fn(AXEl, &str) -> bool,
    ) -> Option<(AXEl, usize)> {
        let mut queue: VecDeque<(AXEl, usize)> = VecDeque::new();
        for r in roots {
            queue.push_back((*r, 0));
        }
        let mut seen = 0usize;
        while let Some((el, depth)) = queue.pop_front() {
            seen += 1;
            if seen > max_nodes || depth > 30 {
                return None;
            }
            let role = ax_role(el);
            if pred(el, &role) {
                CFRetain(el as *const c_void);
                return Some((el, depth));
            }
            for c in ax_list(el, "AXChildren") {
                queue.push_back((c, depth + 1));
            }
        }
        None
    }

    /// 多窗口应用按标题子串选窗口；缺省取首窗口（保持原行为）。
    unsafe fn pick_window(app: AXEl, needle: Option<&str>) -> Result<AXEl, String> {
        let windows = ax_list(app, "AXWindows");
        match needle {
            None => windows.first().copied().ok_or_else(|| "no windows".to_string()),
            Some(n) => {
                let low = n.to_lowercase();
                for w in &windows {
                    let t = ax_str(*w, "AXTitle").unwrap_or_default().to_lowercase();
                    if t.contains(&low) {
                        return Ok(*w);
                    }
                }
                Err(format!("no window title contains {:?}", n))
            }
        }
    }

    /// 目标文本区：优先应用焦点元素，否则在首窗口子树 BFS。
    unsafe fn find_text_area(app: AXEl, windows: &[AXEl]) -> (Option<AXEl>, String) {
        if let Ok(focused) = ax_copy(app, "AXFocusedUIElement") {
            let role = ax_role(focused as AXEl);
            if TEXT_ROLES.contains(&role.as_str()) {
                return (Some(focused as AXEl), "focused".into());
            }
            CFRelease(focused);
        }
        let roots: Vec<AXEl> = if windows.is_empty() { vec![app] } else { windows.to_vec() };
        let hit = bfs_find(&roots, 3000, &|el, role| TEXT_ROLES.contains(&role) && !el.is_null());
        (hit.map(|(el, _)| el), "bfs".into())
    }

    fn bool_attr_json(r: Result<Option<bool>, i32>) -> Value {
        match r {
            Ok(v) => json!({"value": v}),
            Err(e) => json!({"error": ax_err(e)}),
        }
    }

    fn cmd_axdump(pid: i32) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let trusted = (axf().is_trusted)(ptr::null());
            let windows = ax_list(app, "AXWindows");
            let mut win_list = Vec::new();
            for w in &windows {
                let title = ax_str(*w, "AXTitle").unwrap_or_default();
                let role = ax_role(*w);
                let minimized = ax_bool_attr(*w, "AXMinimized").unwrap_or(None);
                let kids = ax_list(*w, "AXChildren").len();
                win_list.push(json!({
                    "title": title, "role": role, "minimized": minimized,
                    "children": kids, "frame": ax_frame(*w)
                }));
            }
            let mut focused = Value::Null;
            if let Ok(f) = ax_copy(app, "AXFocusedUIElement") {
                let mut f_pid: i32 = 0;
                (axf().get_pid)(f as AXEl, &mut f_pid);
                focused = json!({
                    "role": ax_role(f as AXEl),
                    "pid": f_pid,
                    "title": ax_str(f as AXEl, "AXTitle").unwrap_or_default()
                });
                CFRelease(f);
            }
            let mut total = 0usize;
            let mut web_area = 0usize;
            let mut text_areas = 0usize;
            let mut buttons = 0usize;
            let mut links = 0usize;
            if let Some(first) = windows.first() {
                let mut queue: VecDeque<(AXEl, usize)> = VecDeque::new();
                queue.push_back((*first, 0));
                while let Some((el, depth)) = queue.pop_front() {
                    total += 1;
                    if total > 5000 || depth > 30 {
                        break;
                    }
                    match ax_role(el).as_str() {
                        "AXWebArea" => web_area += 1,
                        "AXTextArea" | "AXTextField" => text_areas += 1,
                        "AXButton" => buttons += 1,
                        "AXLink" => links += 1,
                        _ => {}
                    }
                    for c in ax_list(el, "AXChildren") {
                        queue.push_back((c, depth + 1));
                    }
                }
            }
            let manual_val = ax_bool_attr(app, "AXManualAccessibility");
            let enhanced_val = ax_bool_attr(app, "AXEnhancedUserInterface");
            Ok(json!({
                "ax_trusted": trusted,
                "windows": win_list,
                "focused": focused,
                "subtree": {
                    "nodes": total, "web_areas": web_area,
                    "text_areas": text_areas, "buttons": buttons, "links": links,
                    "capped": total > 5000
                },
                "manual_accessibility": bool_attr_json(manual_val),
                "enhanced_user_interface": bool_attr_json(enhanced_val)
            }))
        }
    }

    fn cmd_axread(pid: i32, full: bool) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let windows = ax_list(app, "AXWindows");
            let (el, source) = find_text_area(app, &windows);
            let el = el.ok_or("no AXTextArea/AXTextField found")?;
            let role = ax_role(el);
            let value = match ax_copy(el, "AXValue") {
                Ok(v) => cf_to_string(v).unwrap_or_default(),
                Err(e) => return Err(format!("read AXValue: {}", ax_err(e))),
            };
            // AXSelectedTextRange（CFRange，AXValueType=4）：指针落点的独立事实
            let range = match ax_copy(el, "AXSelectedTextRange") {
                Ok(v) => {
                    let mut loc: i64 = -1;
                    let mut len: i64 = -1;
                    let mut buf = [0i64; 2];
                    let ok = (axf().value_get_value)(v, 4, buf.as_mut_ptr() as *mut c_void);
                    CFRelease(v);
                    if ok {
                        loc = buf[0];
                        len = buf[1];
                    }
                    json!({"ok": ok, "loc": loc, "len": len})
                }
                Err(e) => json!({"ok": false, "err": ax_err(e)}),
            };
            Ok(json!({
                "source": source, "role": role, "len": value.chars().count(),
                "frame": ax_frame(el),
                "selected_range": range,
                "head": value.chars().take(80).collect::<String>(),
                "tail": value.chars().rev().take(80).collect::<String>().chars().rev().collect::<String>(),
                "value": if full { json!(value) } else { Value::Null }
            }))
        }
    }

    fn cmd_axset(pid: i32, text: &str) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let windows = ax_list(app, "AXWindows");
            let (el, source) = find_text_area(app, &windows);
            let el = el.ok_or("no AXTextArea/AXTextField found")?;
            let before = match ax_copy(el, "AXValue") {
                Ok(v) => cf_to_string(v).unwrap_or_default(),
                Err(_) => String::new(),
            };
            let value = CFString::new(text);
            let e = ax_set(el, "AXValue", value.as_concrete_TypeRef() as CFTypeRef);
            let after = match ax_copy(el, "AXValue") {
                Ok(v) => cf_to_string(v).unwrap_or_default(),
                Err(_) => String::new(),
            };
            Ok(json!({
                "source": source, "err": ax_err(e), "err_code": e,
                "before_len": before.chars().count(), "after_len": after.chars().count(),
                "applied": after == text
            }))
        }
    }

    fn cmd_axinsert(pid: i32, text: &str) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let windows = ax_list(app, "AXWindows");
            let (el, source) = find_text_area(app, &windows);
            let el = el.ok_or("no AXTextArea/AXTextField found")?;
            let before = match ax_copy(el, "AXValue") {
                Ok(v) => cf_to_string(v).unwrap_or_default(),
                Err(_) => String::new(),
            };
            let value = CFString::new(text);
            let e = ax_set(el, "AXSelectedText", value.as_concrete_TypeRef() as CFTypeRef);
            std::thread::sleep(Duration::from_millis(80));
            let after = match ax_copy(el, "AXValue") {
                Ok(v) => cf_to_string(v).unwrap_or_default(),
                Err(_) => String::new(),
            };
            Ok(json!({
                "source": source, "err": ax_err(e), "err_code": e,
                "before_len": before.chars().count(), "after_len": after.chars().count(),
                "delta": after.chars().count() as i64 - before.chars().count() as i64,
                "tail": after.chars().rev().take(60).collect::<String>().chars().rev().collect::<String>()
            }))
        }
    }

    /// axpress 与 axfind 共用的定位逻辑（窗口 + 菜单栏 BFS），返回已 retain 的元素。
    unsafe fn find_pressable(pid: i32, needle: &str) -> Result<(AXEl, usize), String> {
        unsafe {
            let app = ax_app(pid);
            let windows = ax_list(app, "AXWindows");
            // 菜单栏也纳入搜索（AXMenuBar 下是各菜单的 AXMenuItem，如 File/Save）。
            let mut roots: Vec<AXEl> = if windows.is_empty() { vec![app] } else { windows };
            if let Ok(mb) = ax_copy(app, "AXMenuBar") {
                roots.push(mb as AXEl);
            }
            let low = needle.to_lowercase();
            let hit = bfs_find(&roots, 5000, &|el, role| {
                if !PRESS_ROLES.contains(&role) {
                    return false;
                }
                let mut hay = ax_str(el, "AXTitle").unwrap_or_default();
                hay.push(' ');
                hay.push_str(&ax_str(el, "AXDescription").unwrap_or_default());
                hay.push(' ');
                if let Ok(v) = ax_copy(el, "AXValue") {
                    hay.push_str(&cf_to_string(v).unwrap_or_default());
                }
                for c in ax_list(el, "AXChildren") {
                    hay.push(' ');
                    hay.push_str(&ax_str(c, "AXTitle").unwrap_or_default());
                    if let Ok(v) = ax_copy(c, "AXValue") {
                        hay.push_str(&cf_to_string(v).unwrap_or_default());
                    }
                    CFRelease(c as *const c_void);
                }
                hay.to_lowercase().contains(&low)
            });
            hit.ok_or(format!("no pressable element matching {:?}", needle))
        }
    }

    fn cmd_axpress(pid: i32, needle: &str) -> Result<Value, String> {
        unsafe {
            let (el, depth) = find_pressable(pid, needle)?;
            let role = ax_role(el);
            let title = ax_str(el, "AXTitle").unwrap_or_default();
            let e = ax_perform(el, "AXPress");
            Ok(json!({
                "matched": {"role": role, "title": title, "depth": depth, "frame": ax_frame(el)},
                "err": ax_err(e), "err_code": e
            }))
        }
    }

    fn cmd_axfind(pid: i32, needle: &str) -> Result<Value, String> {
        unsafe {
            let (el, depth) = find_pressable(pid, needle)?;
            let role = ax_role(el);
            let title = ax_str(el, "AXTitle").unwrap_or_default();
            Ok(json!({
                "matched": {"role": role, "title": title, "depth": depth, "frame": ax_frame(el)}
            }))
        }
    }

    fn cmd_axmanual(pid: i32, on: bool) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let v = if on {
                core_foundation::boolean::CFBoolean::true_value()
            } else {
                core_foundation::boolean::CFBoolean::false_value()
            };
            // Chromium 系开启完整 AX 树的两个入口都试：AXManualAccessibility
            // 与 AXEnhancedUserInterface（VoiceOver 路径），分别回报结果。
            let e1 = ax_set(app, "AXManualAccessibility", v.as_concrete_TypeRef() as CFTypeRef);
            let e2 = ax_set(app, "AXEnhancedUserInterface", v.as_concrete_TypeRef() as CFTypeRef);
            Ok(json!({
                "manual_err": ax_err(e1), "manual_code": e1,
                "enhanced_err": ax_err(e2), "enhanced_code": e2,
                "set": on
            }))
        }
    }

    /// 恢复目标窗口 key 状态：AXRaise（应用内前排）与 AXMain 写入。
    fn cmd_axraise(pid: i32, needle: Option<&str>) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let w = pick_window(app, needle)?;
            let e = ax_perform(w, "AXRaise");
            let main_after = ax_bool_attr(w, "AXMain").unwrap_or(None);
            Ok(json!({"err": ax_err(e), "err_code": e, "main_now": main_after}))
        }
    }

    fn cmd_axmain(pid: i32, on: bool, needle: Option<&str>) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let w = pick_window(app, needle)?;
            let v = if on {
                core_foundation::boolean::CFBoolean::true_value()
            } else {
                core_foundation::boolean::CFBoolean::false_value()
            };
            let e = ax_set(w, "AXMain", v.as_concrete_TypeRef() as CFTypeRef);
            std::thread::sleep(Duration::from_millis(150));
            let now = ax_bool_attr(w, "AXMain").unwrap_or(None);
            Ok(json!({"err": ax_err(e), "err_code": e, "main_now": now}))
        }
    }

    /// 场景搭建用：设置首窗口 AXPosition/AXSize（用于制造完全遮挡）。
    fn cmd_axmove(pid: i32, x: f64, y: f64, w: f64, h: f64, needle: Option<&str>) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let win = pick_window(app, needle)?;
            let p = CGPoint::new(x, y);
            let s = CGSize::new(w, h);
            let pv = (axf().value_create)(1, &p as *const CGPoint as *const c_void);
            let sv = (axf().value_create)(2, &s as *const CGSize as *const c_void);
            if pv.is_null() || sv.is_null() {
                return Err("AXValueCreate failed".into());
            }
            let e1 = ax_set(win, "AXPosition", pv as CFTypeRef);
            let e2 = ax_set(win, "AXSize", sv as CFTypeRef);
            CFRelease(pv);
            CFRelease(sv);
            std::thread::sleep(Duration::from_millis(200));
            Ok(json!({"err_pos": ax_err(e1), "err_size": ax_err(e2), "frame_now": ax_frame(win)}))
        }
    }

    fn cmd_axmin(pid: i32, on: bool, needle: Option<&str>) -> Result<Value, String> {
        unsafe {
            let app = ax_app(pid);
            let w = pick_window(app, needle)?;
            let v = if on {
                core_foundation::boolean::CFBoolean::true_value()
            } else {
                core_foundation::boolean::CFBoolean::false_value()
            };
            let e = ax_set(w, "AXMinimized", v.as_concrete_TypeRef() as CFTypeRef);
            std::thread::sleep(Duration::from_millis(500));
            let now = ax_bool_attr(w, "AXMinimized").unwrap_or(None);
            Ok(json!({"err": ax_err(e), "err_code": e, "minimized_now": now}))
        }
    }

    // ---------- CGEvent 定向输入 ----------

    fn ev_source() -> Result<CGEventSource, String> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| "CGEventSource create failed".to_string())
    }

    /// Unicode 文本 → 键盘事件流（chunk ≤10 UTF-16 units）。to_session=true 走会话级投递。
    fn post_text(pid: i32, text: &str, to_session: bool) -> Result<usize, String> {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut sent = 0usize;
        for chunk in units.chunks(10) {
            for down in [true, false] {
                let ev = CGEvent::new_keyboard_event(ev_source()?, 0, down)
                    .map_err(|_| "CGEventCreateKeyboardEvent failed".to_string())?;
                if down {
                    ev.set_string_from_utf16_unchecked(chunk);
                }
                if to_session {
                    ev.post(CGEventTapLocation::Session);
                } else {
                    ev.post_to_pid(pid);
                }
            }
            sent += chunk.len();
            std::thread::sleep(Duration::from_millis(6));
        }
        Ok(sent)
    }

    fn cmd_key(pid: i32, code: u16, mods: &str) -> Result<Value, String> {
        let mut flags = CGEventFlags::empty();
        for m in mods.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            flags |= match m {
                "cmd" => CGEventFlags::CGEventFlagCommand,
                "shift" => CGEventFlags::CGEventFlagShift,
                "ctrl" => CGEventFlags::CGEventFlagControl,
                "alt" | "option" => CGEventFlags::CGEventFlagAlternate,
                other => return Err(format!("unknown modifier {:?}", other)),
            };
        }
        let down = CGEvent::new_keyboard_event(ev_source()?, code as CGKeyCode, true)
            .map_err(|_| "key event create failed".to_string())?;
        down.set_flags(flags);
        down.post_to_pid(pid);
        std::thread::sleep(Duration::from_millis(30));
        let up = CGEvent::new_keyboard_event(ev_source()?, code as CGKeyCode, false)
            .map_err(|_| "key event create failed".to_string())?;
        up.post_to_pid(pid);
        Ok(json!({"posted": true, "keycode": code, "mods": mods}))
    }

    fn cmd_click(pid: i32, x: f64, y: f64) -> Result<Value, String> {
        let point = CGPoint::new(x, y);
        let mv = CGEvent::new_mouse_event(ev_source()?, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|_| "mouse event create failed".to_string())?;
        mv.post_to_pid(pid);
        std::thread::sleep(Duration::from_millis(25));
        let dn = CGEvent::new_mouse_event(ev_source()?, CGEventType::LeftMouseDown, point, CGMouseButton::Left)
            .map_err(|_| "mouse event create failed".to_string())?;
        dn.post_to_pid(pid);
        std::thread::sleep(Duration::from_millis(35));
        let up = CGEvent::new_mouse_event(ev_source()?, CGEventType::LeftMouseUp, point, CGMouseButton::Left)
            .map_err(|_| "mouse event create failed".to_string())?;
        up.post_to_pid(pid);
        Ok(json!({"posted": true, "point": [x, y]}))
    }

    fn cmd_activate(pid: i32) -> Result<Value, String> {
        unsafe {
            let app: *mut Object =
                msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
            if app.is_null() {
                return Err(format!("no running application for pid {}", pid));
            }
            let ok: BOOL = msg_send![app, activateWithOptions: 0usize];
            Ok(json!({"activated": ok == YES}))
        }
    }

    // ---------- 用户侧 watch ----------

    struct UserState {
        front_pid: i32,
        front_bundle: String,
        focus_pid: i32,
        focus_role: String,
        focus_err: i32,
        mouse: (f64, f64),
        pb_count: i64,
    }

    fn sample_user() -> UserState {
        autoreleasepool(|| unsafe {
            let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            let front: *mut Object = msg_send![ws, frontmostApplication];
            let (front_pid, front_bundle) = if front.is_null() {
                (0, String::new())
            } else {
                let pid: i32 = msg_send![front, processIdentifier];
                let bid: *mut Object = msg_send![front, bundleIdentifier];
                (pid, rs_str(bid))
            };
            // 系统级 AXFocusedUIElement 实测会返回 -25204(cannotComplete)；
            // 改经 frontmost app 的应用级焦点属性兜底，系统级错误码留作诊断。
            let sys = (axf().create_system_wide)();
            let (mut focus_pid, mut focus_role, mut focus_err) = (0, String::new(), 0);
            match ax_copy(sys, "AXFocusedUIElement") {
                Ok(f) => {
                    (axf().get_pid)(f as AXEl, &mut focus_pid);
                    focus_role = ax_role(f as AXEl);
                    CFRelease(f);
                }
                Err(e) => focus_err = e,
            }
            CFRelease(sys as *const c_void);
            if focus_pid == 0 && front_pid > 0 {
                let app = (axf().create_application)(front_pid);
                if let Ok(f) = ax_copy(app, "AXFocusedUIElement") {
                    (axf().get_pid)(f as AXEl, &mut focus_pid);
                    focus_role = ax_role(f as AXEl);
                    CFRelease(f);
                }
                CFRelease(app as *const c_void);
            }
            let ev = CGEventCreate(ptr::null());
            let p = CGEventGetLocation(ev);
            if !ev.is_null() {
                CFRelease(ev);
            }
            use cocoa::appkit::NSPasteboard;
            use cocoa::base::nil;
            let pb = NSPasteboard::generalPasteboard(nil);
            let pb_count = pb.changeCount();
            UserState {
                front_pid,
                front_bundle,
                focus_pid,
                focus_role,
                focus_err,
                mouse: (p.x, p.y),
                pb_count,
            }
        })
    }

    fn state_json(s: &UserState) -> Value {
        json!({
            "front_pid": s.front_pid, "front_bundle": s.front_bundle,
            "focus_pid": s.focus_pid, "focus_role": s.focus_role, "focus_err": s.focus_err,
            "mouse": [s.mouse.0, s.mouse.1], "pb_count": s.pb_count
        })
    }

    fn pb_content_fingerprint() -> (i64, u64, usize) {
        autoreleasepool(|| unsafe {
            use cocoa::appkit::NSPasteboard;
            use cocoa::base::nil;
            let pb = NSPasteboard::generalPasteboard(nil);
            let count = pb.changeCount();
            let s = pb.stringForType(nsstr("public.utf8-plain-text"));
            let text = rs_str(s);
            let mut h: u64 = 0xcbf29ce484222325;
            for b in text.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            (count, h, text.len())
        })
    }

    fn run_watch(ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let pb_start = pb_content_fingerprint();
        let mut last = String::new();
        while Instant::now() < deadline {
            let s = sample_user();
            let v = state_json(&s);
            let line = serde_json::to_string(&v).unwrap_or_default();
            if line != last {
                println!("{}", json!({"t": now_ms(), "state": v}));
                use std::io::Write;
                let _ = std::io::stdout().flush();
                last = line;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let pb_end = pb_content_fingerprint();
        println!(
            "{}",
            json!({
                "summary": true,
                "pb_start": {"count": pb_start.0, "hash": pb_start.1, "len": pb_start.2},
                "pb_end": {"count": pb_end.0, "hash": pb_end.1, "len": pb_end.2},
                "pb_unchanged": pb_start == pb_end
            })
        );
    }

    // ---------- 命令分发 ----------

    fn arg<'a>(args: &'a [String], i: usize) -> Result<&'a str, String> {
        args.get(i)
            .map(|s| s.as_str())
            .ok_or_else(|| format!("missing argument {}", i))
    }

    fn run_cmd(cmd: &str, args: &[String]) -> Result<Value, String> {
        let started = Instant::now();
        let mut out = match cmd {
            "perms" => {
                let ax = unsafe { (axf().is_trusted)(ptr::null()) };
                let scr = ScreenCaptureAccess.preflight();
                json!({
                    "ax_trusted": ax, "screen_capture": scr,
                    "pid": std::process::id(),
                    "exe": std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
                })
            }
            "wins" => {
                let pid: i32 = arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?;
                let content = share_content()?;
                let windows: *mut Object = unsafe { msg_send![content, windows] };
                let n: usize = unsafe { msg_send![windows, count] };
                let mut list = Vec::new();
                for i in 0..n {
                    let w: *mut Object = unsafe { msg_send![windows, objectAtIndex: i] };
                    let v = win_json(w);
                    if pid == 0 || v["pid"].as_i64() == Some(pid as i64) {
                        list.push(v);
                    }
                }
                json!({"windows": list})
            }
            "capture" => {
                let wid: u32 = arg(args, 0)?.parse().map_err(|_| "window id must be int".to_string())?;
                let out = arg(args, 1)?;
                cmd_capture(wid, out)?
            }
            "axdump" => cmd_axdump(arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?)?,
            "axread" => cmd_axread(
                arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?,
                args.get(1).map(|s| s == "full").unwrap_or(false),
            )?,
            "axset" => cmd_axset(arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?, arg(args, 1)?)?,
            "axinsert" => cmd_axinsert(arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?, arg(args, 1)?)?,
            "axpress" => cmd_axpress(arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?, arg(args, 1)?)?,
            "axfind" => cmd_axfind(arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?, arg(args, 1)?)?,
            "axmanual" => cmd_axmanual(
                arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?,
                arg(args, 1)? == "1",
            )?,
            "axraise" => cmd_axraise(
                arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?,
                args.get(1).map(|s| s.as_str()),
            )?,
            "axmain" => cmd_axmain(
                arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?,
                arg(args, 1)? == "1",
                args.get(2).map(|s| s.as_str()),
            )?,
            "axmove" => cmd_axmove(
                arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?,
                arg(args, 1)?.parse().map_err(|_| "x".to_string())?,
                arg(args, 2)?.parse().map_err(|_| "y".to_string())?,
                arg(args, 3)?.parse().map_err(|_| "w".to_string())?,
                arg(args, 4)?.parse().map_err(|_| "h".to_string())?,
                args.get(5).map(|s| s.as_str()),
            )?,
            "axmin" => cmd_axmin(
                arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?,
                arg(args, 1)? == "1",
                args.get(2).map(|s| s.as_str()),
            )?,
            "keys" => {
                let pid: i32 = arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?;
                let sent = post_text(pid, arg(args, 1)?, false)?;
                json!({"posted_utf16_units": sent, "mode": "to_pid"})
            }
            "key" => {
                let pid: i32 = arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?;
                let code: u16 = arg(args, 1)?.parse().map_err(|_| "keycode must be int".to_string())?;
                let mods = args.get(2).map(|s| s.as_str()).unwrap_or("");
                cmd_key(pid, code, mods)?
            }
            "click" => {
                let pid: i32 = arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?;
                let x: f64 = arg(args, 1)?.parse().map_err(|_| "x must be float".to_string())?;
                let y: f64 = arg(args, 2)?.parse().map_err(|_| "y must be float".to_string())?;
                cmd_click(pid, x, y)?
            }
            "activate" => cmd_activate(arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?)?,
            "usertype" => {
                let pid: i32 = arg(args, 0)?.parse().map_err(|_| "pid must be int".to_string())?;
                let act = cmd_activate(pid)?;
                std::thread::sleep(Duration::from_millis(600));
                let sent = post_text(pid, arg(args, 1)?, true)?;
                json!({"activate": act, "posted_utf16_units": sent, "mode": "session"})
            }
            "sessiontype" => {
                let sent = post_text(0, arg(args, 0)?, true)?;
                json!({"posted_utf16_units": sent, "mode": "session"})
            }
            other => return Err(format!("unknown command {:?}", other)),
        };
        out["ok"] = json!(true);
        out["cmd"] = json!(cmd);
        out["ms"] = json!(started.elapsed().as_millis() as u64);
        Ok(out)
    }

    pub fn real_main() {
        let mut args: Vec<String> = std::env::args().collect();
        if !args.is_empty() {
            args.remove(0);
        }
        let spawn = match args.iter().position(|a| a == "--spawn") {
            Some(i) => {
                args.remove(i);
                true
            }
            None => false,
        };
        if args.is_empty() {
            eprintln!("macOS background feasibility probe; see header for commands");
            std::process::exit(2);
        }
        let cmd = args[0].clone();
        if cmd == "watch" {
            let ms: u64 = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(10_000);
            run_watch(ms);
            return;
        }
        let rest: Vec<String> = args[1..].to_vec();
        let job_cmd = cmd.clone();
        let res = if spawn {
            std::thread::spawn(move || run_cmd(&job_cmd, &rest))
                .join()
                .unwrap_or_else(|_| Err("panic in spawned thread".to_string()))
        } else {
            run_cmd(&cmd, &args[1..])
        };
        match res {
            Ok(v) => println!("{}", v),
            Err(e) => {
                println!("{}", json!({"ok": false, "cmd": cmd, "error": e}));
                std::process::exit(1);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    imp::real_main();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_background_probe is macOS-only");
    std::process::exit(2);
}
