//! CU-03 人工探针：macOS 应用发现、窗口选择与受控启动的真实验收（不属于
//! 库产品路径，不经 tools / Host 接线）。探针扮演宿主角色：自建
//! TargetRegistry、自行签发授权，走 pawork_computer_use::macos 后端。
//! 每条命令输出 JSON / JSONL，供 shell 编排取证；证据边界与 CU-01 一致
//! （截图不入仓库，日志注明日期、环境、输入、结果与位置）。
//!
//! 用法：
//!   perms                          当前进程 TCC 状态（preflight，不发 prompt）
//!   apps                           应用列表（bundle id / 族 / pid / start_token / 窗口数）
//!   wins <bundle_id> <titles 0|1>  窗口列表（先证未授权读取被拒；授权后 titles=1 才含标题）
//!   deny <bundle_id>               未授权启动 / 读取与受保护目标、撤销后绑定的拒绝证据
//!   launch <bundle_id>             授权后不激活启动（报告前后 frontmost 与鼠标坐标）
//!   open <bundle_id> <file>        不激活地用目标应用打开文件（构造多窗口）
//!   axclose <pid> [title 子串]     语义关闭匹配标题的窗口（构造 WindowReplaced）
//!   axsetframe <pid> <x> <y> <w> <h> [title 子串]   语义设置窗口帧（构造同帧多窗口）
//!   axminimize <pid> [title 子串]  语义最小化匹配标题的窗口（构造最小化存活证据）
//!   watch <bundle_id> <seconds>    绑定全部窗口后轮询校验，状态变化即输出 JSONL
//!   quit <pid>                     正常退出应用（构造 ProcessRestarted 证据）

#[cfg(target_os = "macos")]
mod imp {
    use objc::rc::autoreleasepool;
    use objc::runtime::{Object, BOOL};
    use objc::{class, msg_send, sel, sel_impl};
    use core_foundation::base::TCFType;
    use pawork_computer_use::approval::{GrantKind, TargetIdentity};
    use pawork_computer_use::macos::MacosNative;
    use pawork_computer_use::target::{Scope, TargetRegistry, WindowHandle};
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::ffi::{c_void, CStr, CString};
    use std::os::raw::c_char;
    use std::ptr;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *const c_void) -> CgPoint;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(p: *const c_void);
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
    }

    type CFTypeRef = *const c_void;
    type AXEl = *mut c_void;

    /// HIServices（AX）运行时解析：验收所需符号。
    struct AxFns {
        create_application: unsafe extern "C" fn(i32) -> AXEl,
        copy_attribute: unsafe extern "C" fn(AXEl, *const c_void, *mut CFTypeRef) -> i32,
        perform_action: unsafe extern "C" fn(AXEl, *const c_void) -> i32,
        set_attribute: unsafe extern "C" fn(AXEl, *const c_void, CFTypeRef) -> i32,
        value_create: unsafe extern "C" fn(i32, *const c_void) -> CFTypeRef,
        value_get_value: unsafe extern "C" fn(CFTypeRef, i32, *mut c_void) -> bool,
        get_window: unsafe extern "C" fn(AXEl, *mut u32) -> i32,
    }

    fn axf() -> &'static AxFns {
        static AXF: std::sync::OnceLock<AxFns> = std::sync::OnceLock::new();
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
                create_application: load(h, "AXUIElementCreateApplication"),
                copy_attribute: load(h, "AXUIElementCopyAttributeValue"),
                perform_action: load(h, "AXUIElementPerformAction"),
                set_attribute: load(h, "AXUIElementSetAttributeValue"),
                value_create: load(h, "AXValueCreate"),
                value_get_value: load(h, "AXValueGetValue"),
                get_window: load(h, "_AXUIElementGetWindow"),
            }
        })
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CgPoint {
        x: f64,
        y: f64,
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

    /// 前台应用与鼠标位置快照：无干扰判定的独立事实。
    fn sample_user() -> Value {
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
            let event = CGEventCreate(ptr::null());
            let point = CGEventGetLocation(event);
            if !event.is_null() {
                CFRelease(event);
            }
            json!({
                "front_pid": front_pid,
                "front_bundle": front_bundle,
                "mouse": [point.x, point.y],
            })
        })
    }

    fn scope() -> Scope {
        Scope::new("cu03-probe", "run-1")
    }

    fn cmd_perms() -> Value {
        let p = MacosNative::new().permissions();
        json!({
            "screen_recording": p.screen_recording,
            "accessibility": p.accessibility,
        })
    }

    fn cmd_apps() -> Value {
        let native = MacosNative::new();
        let apps: Vec<Value> = native
            .list_applications()
            .iter()
            .map(|a| {
                json!({
                    "bundle_id": a.identity.bundle_id,
                    "family": format!("{:?}", a.identity.family),
                    "name": a.localized_name,
                    "pid": a.instance.pid,
                    "start_token": a.instance.start_token,
                    "window_count": a.window_count,
                })
            })
            .collect();
        json!({ "count": apps.len(), "apps": apps })
    }

    fn cmd_wins(bundle_id: &str, titles: bool) -> Result<Value, String> {
        let native = MacosNative::new();
        let mut registry = TargetRegistry::new(native.clone());
        let app = native
            .list_applications()
            .into_iter()
            .find(|a| a.identity.bundle_id.eq_ignore_ascii_case(bundle_id))
            .ok_or_else(|| format!("app not running: {bundle_id}"))?;
        // 读取授权闸：未授权时必须先于任何 AX / 标题读取拒绝。
        let list_without_grant = native
            .list_windows(
                registry.authorizer(),
                &scope(),
                &app.identity,
                &app.instance,
                titles,
            )
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unexpectedly listed".to_string());
        registry
            .grant(
                &scope(),
                &TargetIdentity::application(app.identity.clone()),
                GrantKind::ForRun,
            )
            .map_err(|e| e.to_string())?;
        let windows = native
            .list_windows(
                registry.authorizer(),
                &scope(),
                &app.identity,
                &app.instance,
                titles,
            )
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "bundle_id": app.identity.bundle_id,
            "pid": app.instance.pid,
            "list_without_grant": list_without_grant,
            "titles_included": titles,
            "screen_recording": native.permissions().screen_recording,
            "count": windows.len(),
            "windows": windows.iter().map(|w| json!({
                "window_id": w.window.window_id,
                "generation": w.window.generation,
                "bounds": [w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height],
                "title": w.title,
            })).collect::<Vec<_>>(),
        }))
    }

    fn cmd_deny(bundle_id: &str) -> Value {
        let native = MacosNative::new();
        let mut registry = TargetRegistry::new(native.clone());
        let app = native
            .list_applications()
            .into_iter()
            .find(|a| a.identity.bundle_id.eq_ignore_ascii_case(bundle_id));
        let identity = app
            .as_ref()
            .map(|a| a.identity.clone())
            .unwrap_or(pawork_computer_use::target::AppIdentity {
                bundle_id: bundle_id.to_string(),
                family: pawork_computer_use::target::AppFamily::Appkit,
            });
        // 未授权启动：必须先于 LaunchServices 拒绝。
        let launch_ungranted = native
            .launch_authorized(registry.authorizer(), &scope(), &identity)
            .err()
            .map(|e| e.to_string());
        // 未授权窗口读取：必须先于任何 AX / 标题读取拒绝。
        let list_ungranted = app.as_ref().map(|a| {
            native
                .list_windows(registry.authorizer(), &scope(), &a.identity, &a.instance, false)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unexpectedly listed".to_string())
        });
        // 身份错配：授权目标身份 + 另一个应用的活进程实例，必须先于读取拒绝。
        let list_identity_mismatch = app.as_ref().and_then(|a| {
            let other = native
                .list_applications()
                .into_iter()
                .find(|x| x.identity.bundle_id != a.identity.bundle_id)?;
            registry
                .grant(
                    &scope(),
                    &TargetIdentity::application(a.identity.clone()),
                    GrantKind::ForRun,
                )
                .ok()?;
            let result = native
                .list_windows(
                    registry.authorizer(),
                    &scope(),
                    &a.identity,
                    &other.instance,
                    false,
                )
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unexpectedly listed".to_string());
            registry.revoke(&scope(), &TargetIdentity::application(a.identity.clone()));
            Some(json!({
                "granted": a.identity.bundle_id,
                "instance_bundle": other.identity.bundle_id,
                "error": result,
            }))
        });
        // 受保护目标（macOS 权限界面）：授权与启动都必须拒绝。
        let protected = pawork_computer_use::target::AppIdentity {
            bundle_id: "com.apple.systempreferences".into(),
            family: pawork_computer_use::target::AppFamily::Appkit,
        };
        let grant_protected = registry
            .grant(
                &scope(),
                &TargetIdentity::application(protected.clone()),
                GrantKind::ForRun,
            )
            .err()
            .map(|e| e.to_string());
        let launch_protected = native
            .launch_authorized(registry.authorizer(), &scope(), &protected)
            .err()
            .map(|e| e.to_string());
        // 撤销后绑定：授权 → 列窗 → 撤销 → 绑定必须拒绝。
        let bind_after_revoke = app.as_ref().and_then(|a| {
            registry
                .grant(
                    &scope(),
                    &TargetIdentity::application(a.identity.clone()),
                    GrantKind::ForRun,
                )
                .ok()?;
            let windows = native
                .list_windows(registry.authorizer(), &scope(), &a.identity, &a.instance, false)
                .ok()?;
            let window = windows.first()?;
            registry.revoke(&scope(), &TargetIdentity::application(a.identity.clone()));
            Some(
                registry
                    .bind_window(
                        &scope(),
                        TargetIdentity::application(a.identity.clone()),
                        a.instance.clone(),
                        window.window,
                    )
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "unexpectedly bound".to_string()),
            )
        });
        json!({
            "launch_without_grant": launch_ungranted,
            "list_without_grant": list_ungranted,
            "list_identity_mismatch": list_identity_mismatch,
            "grant_protected_settings": grant_protected,
            "launch_protected_settings": launch_protected,
            "bind_after_revoke": bind_after_revoke,
        })
    }

    fn cmd_launch(bundle_id: &str) -> Result<Value, String> {
        let native = MacosNative::new();
        let mut registry = TargetRegistry::new(native.clone());
        let before = sample_user();
        let already = native
            .list_applications()
            .into_iter()
            .find(|a| a.identity.bundle_id.eq_ignore_ascii_case(bundle_id));
        let identity = already
            .as_ref()
            .map(|a| a.identity.clone())
            .or_else(|| native.resolve_identity(bundle_id))
            .ok_or_else(|| format!("no installed application for: {bundle_id}"))?;
        registry
            .grant(
                &scope(),
                &TargetIdentity::application(identity.clone()),
                GrantKind::ForRun,
            )
            .map_err(|e| e.to_string())?;
        let instance = native
            .launch_authorized(registry.authorizer(), &scope(), &identity)
            .map_err(|e| e.to_string())?;
        let after = sample_user();
        Ok(json!({
            "launched": true,
            "bundle_id": identity.bundle_id,
            "pid": instance.pid,
            "start_token": instance.start_token,
            "front_before": before,
            "front_after": after,
            // 前台应用不变即无焦点抢夺；鼠标是用户自己的，移动属用户活动。
            "front_unchanged": before["front_pid"] == after["front_pid"]
                && before["front_bundle"] == after["front_bundle"],
        }))
    }

    fn cmd_open(bundle_id: &str, file: &str) -> Result<Value, String> {
        let before = sample_user();
        autoreleasepool(|| unsafe {
            let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: nsstr(file)];
            let urls: *mut Object = msg_send![class!(NSMutableArray), array];
            let _: () = msg_send![urls, addObject: url];
            let ok: BOOL = msg_send![
                ws,
                openURLs: urls
                withAppBundleIdentifier: nsstr(bundle_id)
                options: 0x0000_0200usize // NSWorkspaceLaunchWithoutActivation
                additionalEventParamDescriptor: ptr::null_mut::<Object>()
                launchIdentifiers: ptr::null_mut::<Object>()
            ];
            if !ok {
                return Err("openURLs failed".to_string());
            }
            Ok(json!({
                "opened": true,
                "front_before": before,
                "front_after": sample_user(),
            }))
        })
    }

    fn cmd_axclose(pid: i32, needle: Option<&str>) -> Result<Value, String> {
        autoreleasepool(|| unsafe {
            let app = (axf().create_application)(pid);
            if app.is_null() {
                return Err(format!("no AX application for pid {pid}"));
            }
            let mut windows: CFTypeRef = ptr::null();
            let err = (axf().copy_attribute)(app, nsstr("AXWindows") as *const c_void, &mut windows);
            if err != 0 || windows.is_null() {
                CFRelease(app as *const c_void);
                return Err(format!("AXWindows copy failed: {err}"));
            }
            let count = CFArrayGetCount(windows);
            let mut matched: Option<(AXEl, String)> = None;
            for i in 0..count {
                let window = CFArrayGetValueAtIndex(windows, i) as AXEl;
                let mut raw_title: CFTypeRef = ptr::null();
                (axf().copy_attribute)(
                    window,
                    nsstr("AXTitle") as *const c_void,
                    &mut raw_title,
                );
                let title = rs_str(raw_title as *mut Object);
                if !raw_title.is_null() {
                    CFRelease(raw_title);
                }
                let hit = match needle {
                    Some(n) => title.to_lowercase().contains(&n.to_lowercase()),
                    None => true,
                };
                if hit {
                    matched = Some((window, title));
                    break;
                }
            }
            let Some((window, title)) = matched else {
                CFRelease(windows);
                CFRelease(app as *const c_void);
                return Err(format!("no window matching {:?}", needle));
            };
            let mut close: CFTypeRef = ptr::null();
            let err =
                (axf().copy_attribute)(window, nsstr("AXCloseButton") as *const c_void, &mut close);
            if err != 0 || close.is_null() {
                CFRelease(windows);
                CFRelease(app as *const c_void);
                return Err(format!("AXCloseButton copy failed: {err}"));
            }
            let press = (axf().perform_action)(close as AXEl, nsstr("AXPress") as *const c_void);
            let result = json!({
                "pressed": press == 0,
                "press_err": press,
                "window_title": title,
            });
            CFRelease(close);
            CFRelease(windows);
            CFRelease(app as *const c_void);
            Ok(result)
        })
    }

    /// 语义设置匹配窗口的帧（构造同帧多窗口场景）。
    fn cmd_axsetframe(
        pid: i32,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        needle: Option<&str>,
    ) -> Result<Value, String> {
        autoreleasepool(|| unsafe {
            let app = (axf().create_application)(pid);
            if app.is_null() {
                return Err(format!("no AX application for pid {pid}"));
            }
            let mut windows: CFTypeRef = ptr::null();
            let err = (axf().copy_attribute)(app, nsstr("AXWindows") as *const c_void, &mut windows);
            if err != 0 || windows.is_null() {
                CFRelease(app as *const c_void);
                return Err(format!("AXWindows copy failed: {err}"));
            }
            let count = CFArrayGetCount(windows);
            let mut results = Vec::new();
            for i in 0..count {
                let window = CFArrayGetValueAtIndex(windows, i) as AXEl;
                if window.is_null() {
                    continue;
                }
                let mut raw_title: CFTypeRef = ptr::null();
                (axf().copy_attribute)(window, nsstr("AXTitle") as *const c_void, &mut raw_title);
                let title = rs_str(raw_title as *mut Object);
                if !raw_title.is_null() {
                    CFRelease(raw_title);
                }
                let hit = match needle {
                    Some(n) => title.to_lowercase().contains(&n.to_lowercase()),
                    None => true,
                };
                if !hit {
                    continue;
                }
                let position = CgPoint { x, y };
                let pos_value =
                    (axf().value_create)(1, &position as *const CgPoint as *const c_void);
                let pos_err = (axf().set_attribute)(
                    window,
                    nsstr("AXPosition") as *const c_void,
                    pos_value,
                );
                if !pos_value.is_null() {
                    CFRelease(pos_value);
                }
                let size = CgPoint { x: w, y: h };
                let size_value = (axf().value_create)(2, &size as *const CgPoint as *const c_void);
                let size_err =
                    (axf().set_attribute)(window, nsstr("AXSize") as *const c_void, size_value);
                if !size_value.is_null() {
                    CFRelease(size_value);
                }
                results.push(json!({
                    "title": title,
                    "position_err": pos_err,
                    "size_err": size_err,
                }));
            }
            CFRelease(windows);
            CFRelease(app as *const c_void);
            if results.is_empty() {
                return Err(format!("no window matching {:?}", needle));
            }
            Ok(json!({ "set": results }))
        })
    }

    /// 语义最小化匹配标题的窗口（缺省第一扇）。
    fn cmd_axminimize(pid: i32, needle: Option<&str>) -> Result<Value, String> {
        autoreleasepool(|| unsafe {
            let app = (axf().create_application)(pid);
            if app.is_null() {
                return Err(format!("no AX application for pid {pid}"));
            }
            let mut windows: CFTypeRef = ptr::null();
            let err = (axf().copy_attribute)(app, nsstr("AXWindows") as *const c_void, &mut windows);
            if err != 0 || windows.is_null() {
                CFRelease(app as *const c_void);
                return Err(format!("AXWindows copy failed: {err}"));
            }
            let count = CFArrayGetCount(windows);
            let mut matched: Option<(AXEl, String)> = None;
            for i in 0..count {
                let window = CFArrayGetValueAtIndex(windows, i) as AXEl;
                if window.is_null() {
                    continue;
                }
                let mut raw_title: CFTypeRef = ptr::null();
                (axf().copy_attribute)(window, nsstr("AXTitle") as *const c_void, &mut raw_title);
                let title = rs_str(raw_title as *mut Object);
                if !raw_title.is_null() {
                    CFRelease(raw_title);
                }
                let hit = match needle {
                    Some(n) => title.to_lowercase().contains(&n.to_lowercase()),
                    None => true,
                };
                if hit {
                    matched = Some((window, title));
                    break;
                }
            }
            let Some((window, title)) = matched else {
                CFRelease(windows);
                CFRelease(app as *const c_void);
                return Err(format!("no window matching {:?}", needle));
            };
            let truth = core_foundation::boolean::CFBoolean::true_value();
            let err = (axf().set_attribute)(
                window,
                nsstr("AXMinimized") as *const c_void,
                truth.as_concrete_TypeRef() as CFTypeRef,
            );
            CFRelease(windows);
            CFRelease(app as *const c_void);
            Ok(json!({
                "minimized": err == 0,
                "set_err": err,
                "window_title": title,
            }))
        })
    }

    /// 诊断：从探针进程（有 Accessibility 授权）读取目标 AX 窗口真值。
    fn cmd_axdump(pid: i32) -> Result<Value, String> {
        autoreleasepool(|| unsafe {
            let app = (axf().create_application)(pid);
            if app.is_null() {
                return Err(format!("no AX application for pid {pid}"));
            }
            let mut windows: CFTypeRef = ptr::null();
            let err = (axf().copy_attribute)(app, nsstr("AXWindows") as *const c_void, &mut windows);
            if err != 0 || windows.is_null() {
                CFRelease(app as *const c_void);
                return Err(format!("AXWindows copy failed: {err}"));
            }
            let count = CFArrayGetCount(windows);
            let mut out = Vec::new();
            for i in 0..count {
                let window = CFArrayGetValueAtIndex(windows, i) as AXEl;
                if window.is_null() {
                    continue;
                }
                let mut id: u32 = 0;
                let id_err = (axf().get_window)(window, &mut id);
                let mut raw_title: CFTypeRef = ptr::null();
                let title_err =
                    (axf().copy_attribute)(window, nsstr("AXTitle") as *const c_void, &mut raw_title);
                let title = rs_str(raw_title as *mut Object);
                if !raw_title.is_null() {
                    CFRelease(raw_title);
                }
                let mut raw_pos: CFTypeRef = ptr::null();
                let pos_err = (axf().copy_attribute)(
                    window,
                    nsstr("AXPosition") as *const c_void,
                    &mut raw_pos,
                );
                let mut point = CgPoint { x: 0.0, y: 0.0 };
                let pos_ok = !raw_pos.is_null()
                    && (axf().value_get_value)(
                        raw_pos,
                        1,
                        &mut point as *mut CgPoint as *mut c_void,
                    );
                if !raw_pos.is_null() {
                    CFRelease(raw_pos);
                }
                let mut raw_size: CFTypeRef = ptr::null();
                let size_err =
                    (axf().copy_attribute)(window, nsstr("AXSize") as *const c_void, &mut raw_size);
                let mut dim = CgPoint { x: 0.0, y: 0.0 };
                let size_ok = !raw_size.is_null()
                    && (axf().value_get_value)(
                        raw_size,
                        2,
                        &mut dim as *mut CgPoint as *mut c_void,
                    );
                if !raw_size.is_null() {
                    CFRelease(raw_size);
                }
                let mut raw_mini: CFTypeRef = ptr::null();
                let mini_err = (axf().copy_attribute)(
                    window,
                    nsstr("AXMinimized") as *const c_void,
                    &mut raw_mini,
                );
                let minimized = !raw_mini.is_null() && raw_mini == unsafe {
                    core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef()
                        as CFTypeRef
                };
                if !raw_mini.is_null() {
                    CFRelease(raw_mini);
                }
                out.push(json!({
                    "index": i,
                    "id_err": id_err,
                    "cg_window_id": id,
                    "title_err": title_err,
                    "title": title,
                    "pos_err": pos_err,
                    "pos_ok": pos_ok,
                    "pos": [point.x, point.y],
                    "size_err": size_err,
                    "size_ok": size_ok,
                    "size": [dim.x, dim.y],
                    "mini_err": mini_err,
                    "minimized": minimized,
                }));
            }
            CFRelease(windows);
            CFRelease(app as *const c_void);
            Ok(json!({ "pid": pid, "ax_window_count": out.len(), "windows": out }))
        })
    }

    fn cmd_quit(pid: i32) -> Value {
        autoreleasepool(|| unsafe {
            let app: *mut Object = msg_send![
                class!(NSRunningApplication),
                runningApplicationWithProcessIdentifier: pid
            ];
            if app.is_null() {
                return json!({ "terminated": false, "reason": "no application for pid" });
            }
            let ok: BOOL = msg_send![app, terminate];
            json!({ "terminated": ok })
        })
    }

    fn cmd_watch(bundle_id: &str, seconds: u64) -> Result<Value, String> {
        let native = MacosNative::new();
        let mut registry = TargetRegistry::new(native.clone());
        let app = native
            .list_applications()
            .into_iter()
            .find(|a| a.identity.bundle_id.eq_ignore_ascii_case(bundle_id))
            .ok_or_else(|| format!("app not running: {bundle_id}"))?;
        registry
            .grant(
                &scope(),
                &TargetIdentity::application(app.identity.clone()),
                GrantKind::ForRun,
            )
            .map_err(|e| e.to_string())?;
        let windows = native
            .list_windows(
                registry.authorizer(),
                &scope(),
                &app.identity,
                &app.instance,
                true,
            )
            .map_err(|e| e.to_string())?;
        let mut handles: HashMap<String, (u64, Option<String>)> = HashMap::new();
        let mut bound: Vec<WindowHandle> = Vec::new();
        for window in &windows {
            let handle = registry
                .bind_window(
                    &scope(),
                    TargetIdentity::application(app.identity.clone()),
                    app.instance.clone(),
                    window.window,
                )
                .map_err(|e| e.to_string())?;
            handles.insert(
                handle.as_str().to_string(),
                (window.window.window_id, window.title.clone()),
            );
            bound.push(handle);
        }
        println!(
            "{}",
            json!({
                "t": now_ms(),
                "event": "bound",
                "bundle_id": app.identity.bundle_id,
                "pid": app.instance.pid,
                "windows": bound.iter().map(|h| {
                    let (id, title) = &handles[h.as_str()];
                    json!({ "handle": h.as_str(), "window_id": id, "title": title })
                }).collect::<Vec<_>>(),
                "user": sample_user(),
            })
        );
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut last_user = String::new();
        while Instant::now() < deadline && !bound.is_empty() {
            std::thread::sleep(Duration::from_millis(500));
            let user = sample_user();
            let user_line = serde_json::to_string(&user).unwrap_or_default();
            if user_line != last_user {
                println!("{}", json!({ "t": now_ms(), "event": "user", "state": user }));
                last_user = user_line;
            }
            let mut alive = Vec::new();
            for handle in bound {
                match registry.validate_window(&handle, &scope()) {
                    Ok(_) => alive.push(handle),
                    Err(error) => {
                        let (id, title) = &handles[handle.as_str()];
                        println!(
                            "{}",
                            json!({
                                "t": now_ms(),
                                "event": "invalidated",
                                "handle": handle.as_str(),
                                "window_id": id,
                                "title": title,
                                "error": error.to_string(),
                            })
                        );
                    }
                }
            }
            bound = alive;
        }
        Ok(json!({ "t": now_ms(), "event": "done", "remaining": bound.len() }))
    }

    pub fn run(args: Vec<String>) -> Result<Value, String> {
        match args.first().map(String::as_str) {
            Some("perms") => Ok(cmd_perms()),
            Some("apps") => Ok(cmd_apps()),
            Some("wins") => cmd_wins(
                args.get(1).ok_or("wins 需要 bundle_id")?,
                args.get(2).map(String::as_str) == Some("1"),
            ),
            Some("deny") => Ok(cmd_deny(args.get(1).ok_or("deny 需要 bundle_id")?)),
            Some("launch") => cmd_launch(args.get(1).ok_or("launch 需要 bundle_id")?),
            Some("open") => cmd_open(
                args.get(1).ok_or("open 需要 bundle_id")?,
                args.get(2).ok_or("open 需要文件路径")?,
            ),
            Some("quit") => Ok(cmd_quit(
                args.get(1)
                    .ok_or("quit 需要 pid")?
                    .parse()
                    .map_err(|_| "pid 非法")?,
            )),
            Some("axclose") => cmd_axclose(
                args.get(1)
                    .ok_or("axclose 需要 pid")?
                    .parse()
                    .map_err(|_| "pid 非法")?,
                args.get(2).map(String::as_str),
            ),
            Some("axsetframe") => cmd_axsetframe(
                args.get(1)
                    .ok_or("axsetframe 需要 pid")?
                    .parse()
                    .map_err(|_| "pid 非法")?,
                args.get(2).ok_or("axsetframe 需要 x")?.parse().map_err(|_| "x 非法")?,
                args.get(3).ok_or("axsetframe 需要 y")?.parse().map_err(|_| "y 非法")?,
                args.get(4).ok_or("axsetframe 需要 w")?.parse().map_err(|_| "w 非法")?,
                args.get(5).ok_or("axsetframe 需要 h")?.parse().map_err(|_| "h 非法")?,
                args.get(6).map(String::as_str),
            ),
            Some("axminimize") => cmd_axminimize(
                args.get(1)
                    .ok_or("axminimize 需要 pid")?
                    .parse()
                    .map_err(|_| "pid 非法")?,
                args.get(2).map(String::as_str),
            ),
            Some("axdump") => cmd_axdump(
                args.get(1)
                    .ok_or("axdump 需要 pid")?
                    .parse()
                    .map_err(|_| "pid 非法")?,
            ),
            Some("watch") => cmd_watch(
                args.get(1).ok_or("watch 需要 bundle_id")?,
                args.get(2)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(60),
            ),
            _ => Err("未知命令".to_string()),
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match imp::run(args) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_target_probe is macOS-only");
    std::process::exit(1);
}
