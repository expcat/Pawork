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
//!   capture <bundle_id> <jpeg> [title 子串]
//!                                CU-04 窗口截图全流程：未授权拒绝证据 → 预检 →
//!                                校验 → SCK 捕获 → 注册观测 → 落盘 JPEG（前后台前台/鼠标快照）
//!   obsflow <bundle_id> [title 子串]
//!                                观测生命周期：缩放后旧观测 StaleHandle、
//!                                仅移动仍可派发、图像坐标→窗口坐标映射（结束后还原窗口帧）
//!   capturegone <bundle_id> [title 子串]
//!                                捕获成功后语义关窗，同一句柄再捕获按 WindowReplaced 拒绝
//!   axtree <bundle_id> [title 子串]  CU-05 有界读树：未授权拒绝证据 → 授权读取
//!                                （版本 / 截断标志 / 角色分布 / 节点样本）→ 登记 AxTree 观测
//!   axfind <bundle_id> <role|-> [name 子串|-] [title 子串]
//!                                元素定位：唯一匹配签发元素句柄并验证消费；歧义 /
//!                                无匹配 / 截断不可证唯一显式返回；跨 run 与伪造句柄拒绝
//!   axstale <bundle_id> [title 子串] 元素句柄失效证据：缩放改树 → TreeChanged，
//!                                关窗 → WindowReplaced（验收后窗口帧还原）
//!   axact <bundle_id> <press|set_value|insert_text> <role|-> [name|-] [text|-] [title 子串]
//!                                CU-06 语义动作全流程：矩阵预检 → 读树定位唯一元素 →
//!                                签发并消费句柄 → 后端派发（能力闸 / 版本复核 / 路径重解析）→
//!                                读回 AXValue/选区与窗口标题复核效果（探针专用读回，产品路径
//!                                永不读 AXValue）→ 前台/焦点/鼠标/剪贴板无干扰采样
//!   axinput <bundle_id> <insert_text|key_press> <role|-> [name|-] <text|key> [mods|-] [title 子串]
//!                                CU-07 定向输入全流程：矩阵预检 → 读树定位唯一元素 →
//!                                签发并消费句柄 → 后端派发（能力闸 / 最小化 / AXMain /
//!                                焦点 / 选区复核）→ 读回 AXValue/选区复核效果 →
//!                                前台/焦点/鼠标/剪贴板无干扰采样

#[cfg(target_os = "macos")]
mod imp {
    use objc::rc::autoreleasepool;
    use objc::runtime::{Object, BOOL};
    use objc::{class, msg_send, sel, sel_impl};
    use core_foundation::base::TCFType;
    use pawork_computer_use::approval::{GrantKind, TargetIdentity};
    use pawork_computer_use::macos::{DiscoveredApp, DiscoveredWindow, MacosNative, WindowCapture};
    use pawork_computer_use::target::{
        image_to_window, lookup_element, AxTreeRead, ElementHandle, ElementLookup, ElementQuery,
        Environment, ImagePoint, ObservationGeometry, ObservationKind, Scope, TargetObservation,
        TargetRegistry, ValidatedWindow, WindowHandle,
    };
    use pawork_computer_use::target::{
        require_background, KeyModifiers, PointerAction, SemanticAction, SemanticOutcome,
        TargetedInput, TargetedKey, TargetedKeyPress, TargetedOutcome, WindowMove, WindowPoint,
    };
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
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(p: *const c_void);
        fn CFArrayGetCount(a: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
        fn CFGetTypeID(p: *const c_void) -> u64;
        fn CFStringGetTypeID() -> u64;
        fn CFBooleanGetTypeID() -> u64;
        fn CFBooleanGetValue(b: *const c_void) -> u8;
    }

    type CFTypeRef = *const c_void;
    type AXEl = *mut c_void;

    /// HIServices（AX）运行时解析：验收所需符号。
    struct AxFns {
        create_application: unsafe extern "C" fn(i32) -> AXEl,
        create_system_wide: unsafe extern "C" fn() -> AXEl,
        copy_attribute: unsafe extern "C" fn(AXEl, *const c_void, *mut CFTypeRef) -> i32,
        perform_action: unsafe extern "C" fn(AXEl, *const c_void) -> i32,
        set_attribute: unsafe extern "C" fn(AXEl, *const c_void, CFTypeRef) -> i32,
        value_create: unsafe extern "C" fn(i32, *const c_void) -> CFTypeRef,
        value_get_value: unsafe extern "C" fn(CFTypeRef, i32, *mut c_void) -> bool,
        get_window: unsafe extern "C" fn(AXEl, *mut u32) -> i32,
        get_pid: unsafe extern "C" fn(AXEl, *mut i32) -> i32,
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
                create_system_wide: load(h, "AXUIElementCreateSystemWide"),
                copy_attribute: load(h, "AXUIElementCopyAttributeValue"),
                perform_action: load(h, "AXUIElementPerformAction"),
                set_attribute: load(h, "AXUIElementSetAttributeValue"),
                value_create: load(h, "AXValueCreate"),
                value_get_value: load(h, "AXValueGetValue"),
                get_window: load(h, "_AXUIElementGetWindow"),
                get_pid: load(h, "AXUIElementGetPid"),
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

    /// 前台应用、焦点应用与鼠标位置快照：无干扰判定的独立事实。
    /// 焦点经系统级 AXFocusedApplication 观测（frontmost 只说明活跃，
    /// 不说明键盘焦点）。
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
            // WindowServer 连接未建立时系统级焦点查询稳定返回
            // CannotComplete（本机实测：launch / capture 流程里
            // list_applications 之后首次成功，open 流程没有窗口枚举
            // 就持续失败）：先枚举一次窗口列表建立连接，再做焦点
            // 查询；仍空则如实报 focus_observed=false，不把观测缺失
            // 当成焦点变化。
            let window_list = CGWindowListCopyWindowInfo(0, 0);
            if !window_list.is_null() {
                CFRelease(window_list);
            }
            let event = CGEventCreate(ptr::null());
            let point = CGEventGetLocation(event);
            if !event.is_null() {
                CFRelease(event);
            }
            let mut focus_pid = 0i32;
            let mut focus_err = -1i32;
            for _ in 0..25 {
                let sys = (axf().create_system_wide)();
                if !sys.is_null() {
                    let mut focused: CFTypeRef = ptr::null();
                    let err = (axf().copy_attribute)(
                        sys,
                        nsstr("AXFocusedApplication") as *const c_void,
                        &mut focused,
                    );
                    focus_err = err;
                    if err == 0 && !focused.is_null() {
                        (axf().get_pid)(focused as AXEl, &mut focus_pid);
                        CFRelease(focused);
                    }
                    CFRelease(sys as *const c_void);
                }
                if focus_pid != 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            let focus_bundle = if focus_pid == 0 {
                String::new()
            } else {
                let app: *mut Object = msg_send![
                    class!(NSRunningApplication),
                    runningApplicationWithProcessIdentifier: focus_pid
                ];
                if app.is_null() {
                    String::new()
                } else {
                    rs_str(msg_send![app, bundleIdentifier])
                }
            };
            json!({
                "front_pid": front_pid,
                "front_bundle": front_bundle,
                "focus_pid": focus_pid,
                "focus_bundle": focus_bundle,
                "focus_observed": focus_pid != 0,
                "focus_err": focus_err,
                "mouse": [point.x, point.y],
            })
        })
    }

    /// 采样直到系统级焦点查询可用（进程冷启动 CannotComplete 窗口约
    /// 数秒）或如实返回未观测样本。
    fn sample_user_settled() -> Value {
        for attempt in 0..6 {
            let sample = sample_user();
            if sample["focus_observed"] == true || attempt == 5 {
                return sample;
            }
            std::thread::sleep(Duration::from_millis(700));
        }
        unreachable!()
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
    fn cmd_axminimize2(pid: i32, needle: Option<&str>, minimized: bool) -> Result<Value, String> {
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
            let value = if minimized {
                core_foundation::boolean::CFBoolean::true_value()
            } else {
                core_foundation::boolean::CFBoolean::false_value()
            };
            let err = (axf().set_attribute)(
                window,
                nsstr("AXMinimized") as *const c_void,
                value.as_concrete_TypeRef() as CFTypeRef,
            );
            CFRelease(windows);
            CFRelease(app as *const c_void);
            Ok(json!({
                "minimized": minimized && err == 0,
                "set_err": err,
                "window_title": title,
            }))
        })
    }

    /// 语义最小化匹配标题的窗口（缺省第一扇）。
    fn cmd_axminimize(pid: i32, needle: Option<&str>) -> Result<Value, String> {
        cmd_axminimize2(pid, needle, true)
    }

    /// 还原最小化（验收后清理用户侧状态）。
    fn cmd_axunminimize(pid: i32, needle: Option<&str>) -> Result<Value, String> {
        cmd_axminimize2(pid, needle, false)
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
                let minimized = !raw_mini.is_null()
                    && raw_mini
                        == core_foundation::boolean::CFBoolean::true_value().as_concrete_TypeRef()
                            as CFTypeRef;
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

    // ---------- CU-04 窗口截图与观测坐标 ----------

    struct BoundTarget {
        native: MacosNative,
        registry: TargetRegistry<MacosNative>,
        app: DiscoveredApp,
        window: DiscoveredWindow,
        handle: WindowHandle,
    }

    /// 授权并绑定一扇窗口（缺省第一扇，或按标题子串匹配）。
    fn bind_window(bundle_id: &str, needle: Option<&str>) -> Result<BoundTarget, String> {
        let native = MacosNative::new();
        let mut registry = TargetRegistry::new(native.clone());
        // 同一 bundle id 可同时跑多个进程实例（独立 --user-data-dir 的
        // Chrome 与用户会话并存）：按 needle 落到真正持有匹配窗口的实例，
        // 缺省保持第一个有窗实例。
        let candidates: Vec<DiscoveredApp> = native
            .list_applications()
            .into_iter()
            .filter(|a| a.identity.bundle_id.eq_ignore_ascii_case(bundle_id))
            .collect();
        if candidates.is_empty() {
            return Err(format!("app not running: {bundle_id}"));
        }
        let mut picked: Option<(DiscoveredApp, DiscoveredWindow)> = None;
        for app in &candidates {
            let granted = registry
                .grant(
                    &scope(),
                    &TargetIdentity::application(app.identity.clone()),
                    GrantKind::ForRun,
                )
                .map_err(|e| e.to_string());
            if granted.is_err() {
                continue;
            }
            let Ok(windows) = native.list_windows(
                registry.authorizer(),
                &scope(),
                &app.identity,
                &app.instance,
                true,
            ) else {
                continue;
            };
            let found = windows.into_iter().find(|w| match needle {
                Some(n) => w
                    .title
                    .clone()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&n.to_lowercase()),
                None => true,
            });
            if let Some(window) = found {
                picked = Some((app.clone(), window));
                break;
            }
        }
        let (app, window) = picked.ok_or_else(|| format!("no window matching {:?}", needle))?;
        registry
            .grant(
                &scope(),
                &TargetIdentity::application(app.identity.clone()),
                GrantKind::ForRun,
            )
            .map_err(|e| e.to_string())?;
        let handle = registry
            .bind_window(
                &scope(),
                TargetIdentity::application(app.identity.clone()),
                app.instance.clone(),
                window.window,
            )
            .map_err(|e| e.to_string())?;
        Ok(BoundTarget {
            native,
            registry,
            app,
            window,
            handle,
        })
    }

    /// 契约观测流程：校验 → SCK 捕获 → 用捕获几何注册一次性观测。
    fn observe(bound: &mut BoundTarget) -> Result<(WindowCapture, TargetObservation), String> {
        let validated = bound
            .registry
            .validate_window(&bound.handle, &scope())
            .map_err(|e| e.to_string())?;
        let capture = bound
            .native
            .capture_window(bound.registry.authorizer(), &validated)
            .map_err(|e| e.to_string())?;
        let observation = bound
            .registry
            .begin_observation(
                &bound.handle,
                &scope(),
                Environment::NativeBackground,
                ObservationKind::Capture,
                ObservationGeometry {
                    image_width: capture.image_width,
                    image_height: capture.image_height,
                    window_width: capture.window_width,
                    window_height: capture.window_height,
                },
            )
            .map_err(|e| e.to_string())?;
        Ok((capture, observation))
    }

    /// 边缘覆盖检查：内容未铺满输出时边缘是黑条（平均亮度 ≈0）。
    /// 四条边缘带（厚度为短边 2%，至少 2px）的平均亮度与全局最大亮度。
    fn coverage(jpeg: &[u8]) -> Result<Value, String> {
        let rgb = image::load_from_memory(jpeg)
            .map_err(|e| format!("decode jpeg: {e}"))?
            .to_rgb8();
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);
        let t = (w.min(h) / 50).max(2);
        let strip = |xs: std::ops::Range<usize>, ys: std::ops::Range<usize>| {
            let mut sum = 0u64;
            let mut n = 0u64;
            for y in ys {
                for x in xs.clone() {
                    let p = rgb.get_pixel(x as u32, y as u32);
                    sum += (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3;
                    n += 1;
                }
            }
            (sum / n.max(1)) as u32
        };
        let borders = json!({
            "top": strip(0..w, 0..t),
            "bottom": strip(0..w, h - t..h),
            "left": strip(0..t, 0..h),
            "right": strip(w - t..w, 0..h),
        });
        let fills = borders
            .as_object()
            .unwrap()
            .values()
            .all(|v| v.as_u64().unwrap_or(0) > 16);
        Ok(json!({ "border_luminance": borders, "content_fills_frame": fills }))
    }

    /// 两帧解码后的同位置像素一致率（任一通道差 ≤8 视为一致）：
    /// 窗口仅移动时，真实内容应停留在同一图像相对位置。
    fn pixel_match_ratio(a: &[u8], b: &[u8]) -> Result<f64, String> {
        let ia = image::load_from_memory(a)
            .map_err(|e| format!("decode jpeg: {e}"))?
            .to_rgb8();
        let ib = image::load_from_memory(b)
            .map_err(|e| format!("decode jpeg: {e}"))?
            .to_rgb8();
        if ia.dimensions() != ib.dimensions() {
            return Err("dimension mismatch".to_string());
        }
        let mut same = 0u64;
        for (pa, pb) in ia.pixels().zip(ib.pixels()) {
            if (0..3).all(|c| (pa[c] as i32 - pb[c] as i32).abs() <= 8) {
                same += 1;
            }
        }
        Ok(same as f64 / (ia.width() as u64 * ia.height() as u64).max(1) as f64)
    }

    fn cmd_capture(bundle_id: &str, out: &str, needle: Option<&str>) -> Result<Value, String> {
        let before = sample_user_settled();
        let mut bound = bind_window(bundle_id, needle)?;
        // 未授权捕获必须先于任何 OS 访问拒绝。
        let denied = {
            let authorizer = pawork_computer_use::approval::TargetAuthorizer::default();
            let validated = ValidatedWindow {
                target: TargetIdentity::application(bound.app.identity.clone()),
                instance: bound.app.instance.clone(),
                window: bound.window.window,
                scope: scope(),
            };
            bound
                .native
                .capture_window(&authorizer, &validated)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unexpectedly captured".to_string())
        };
        bound
            .registry
            .require_authorized(&bound.handle, &scope())
            .map_err(|e| e.to_string())?;
        let (capture, observation) = observe(&mut bound)?;
        std::fs::write(out, &capture.jpeg).map_err(|e| format!("write {out}: {e}"))?;
        let after = sample_user_settled();
        Ok(json!({
            "capture_without_grant": denied,
            "window_id": capture.window.window_id,
            "generation": capture.window.generation,
            "window_title": bound.window.title,
            "observation_id": observation.observation_id.as_str(),
            "image": [capture.image_width, capture.image_height],
            "window": [capture.window_width, capture.window_height],
            "jpeg_bytes": capture.jpeg.len(),
            "coverage": coverage(&capture.jpeg)?,
            "path": out,
            "front_unchanged": before["front_pid"] == after["front_pid"],
            "focus_unchanged": before["focus_pid"] == after["focus_pid"]
                && before["focus_pid"] != 0,
            "focus_observed": before["focus_observed"] == true && after["focus_observed"] == true,
            "mouse_delta": [
                after["mouse"][0].as_f64().unwrap_or(0.0) - before["mouse"][0].as_f64().unwrap_or(0.0),
                after["mouse"][1].as_f64().unwrap_or(0.0) - before["mouse"][1].as_f64().unwrap_or(0.0),
            ],
            "user_before": before,
            "user_after": after,
        }))
    }

    fn cmd_obsflow(bundle_id: &str, needle: Option<&str>) -> Result<Value, String> {
        let mut bound = bind_window(bundle_id, needle)?;
        let pid = bound.app.instance.pid as i32;
        let original = bound.window.bounds;
        let mut steps = Vec::new();
        let (capture, observation) = observe(&mut bound)?;
        steps.push(json!({
            "step": "captured",
            "image": [capture.image_width, capture.image_height],
            "window": [capture.window_width, capture.window_height],
        }));
        // 缩放窗口：旧观测必须按 StaleHandle 拒绝。
        let resized = cmd_axsetframe(
            pid,
            original.x,
            original.y,
            original.width + 160.0,
            original.height + 90.0,
            needle,
        )?;
        std::thread::sleep(Duration::from_millis(400));
        let after_resize = bound
            .registry
            .consume_observation(&observation.observation_id, &scope(), &|| false)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unexpectedly dispatched".to_string());
        steps.push(json!({
            "step": "consume_after_resize",
            "setframe": resized,
            "result": after_resize,
        }));
        // 新尺寸重新观测；仅移动窗口（尺寸不变）仍可派发。
        let (capture2, observation2) = observe(&mut bound)?;
        let moved = cmd_axsetframe(
            pid,
            original.x + 48.0,
            original.y + 48.0,
            original.width + 160.0,
            original.height + 90.0,
            needle,
        )?;
        std::thread::sleep(Duration::from_millis(400));
        let validated = bound
            .registry
            .consume_observation(&observation2.observation_id, &scope(), &|| false)
            .map_err(|e| e.to_string())?;
        let center = image_to_window(
            ImagePoint {
                x: capture2.image_width as f64 / 2.0,
                y: capture2.image_height as f64 / 2.0,
            },
            &validated.observation,
        )
        .map_err(|e| e.to_string())?;
        steps.push(json!({
            "step": "consume_after_move",
            "setframe": moved,
            "dispatched": true,
            "center_image_point": [capture2.image_width as f64 / 2.0, capture2.image_height as f64 / 2.0],
            "center_window_point": [center.x, center.y],
            "window": [capture2.window_width, capture2.window_height],
        }));
        // 真实内容复验：仅移动窗口后同位置内容必须留在同一图像相对
        // 位置（像素一致率 ≈1），且内容铺满整帧（无黑边），坐标映射
        // 才不是公式自洽。
        let (capture3, _observation3) = observe(&mut bound)?;
        steps.push(json!({
            "step": "recapture_after_move",
            "image": [capture3.image_width, capture3.image_height],
            "moved_content_match_ratio": pixel_match_ratio(&capture2.jpeg, &capture3.jpeg)?,
            "coverage": coverage(&capture3.jpeg)?,
        }));
        // 还原窗口帧，不留下用户侧状态。
        let restored = cmd_axsetframe(
            pid,
            original.x,
            original.y,
            original.width,
            original.height,
            needle,
        )?;
        Ok(json!({
            "bundle_id": bound.app.identity.bundle_id,
            "window_id": bound.window.window.window_id,
            "window_title": bound.window.title,
            "steps": steps,
            "restored": restored,
        }))
    }

    fn cmd_capturegone(bundle_id: &str, needle: Option<&str>) -> Result<Value, String> {
        let bound = bind_window(bundle_id, needle)?;
        let validated = bound
            .registry
            .validate_window(&bound.handle, &scope())
            .map_err(|e| e.to_string())?;
        let first = bound
            .native
            .capture_window(bound.registry.authorizer(), &validated)
            .map_err(|e| e.to_string())?;
        let closed = cmd_axclose(bound.app.instance.pid as i32, needle)?;
        std::thread::sleep(Duration::from_millis(600));
        let capture_after_close = bound
            .native
            .capture_window(bound.registry.authorizer(), &validated)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unexpectedly captured".to_string());
        let validate_after_close = bound
            .registry
            .validate_window(&bound.handle, &scope())
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unexpectedly valid".to_string());
        Ok(json!({
            "first_capture": [first.image_width, first.image_height],
            "closed": closed,
            "capture_after_close": capture_after_close,
            "validate_after_close": validate_after_close,
        }))
    }

    // ---------- CU-05 有界无障碍树与元素定位 ----------

    /// 一个节点的验收摘要（名称已经过读取侧 200 字符截断）。
    fn node_summary(read: &AxTreeRead, index: usize) -> Value {
        let node = &read.nodes[index];
        json!({
            "index": index,
            "path": node.path,
            "role": node.role,
            "name": node.name,
            "name_truncated": node.name_truncated,
            "enabled": node.enabled,
            "focused": node.focused,
            "frame": node.frame,
            "actions": node.actions,
            "children": node.children,
            "depth_limited": node.depth_limited,
        })
    }

    /// 登记 AxTree 树版本观测（图像与窗口点 1:1：元素帧本身是窗口点）。
    fn register_ax_observation(
        bound: &mut BoundTarget,
        read: &AxTreeRead,
    ) -> Result<TargetObservation, String> {
        bound
            .registry
            .begin_observation(
                &bound.handle,
                &scope(),
                Environment::NativeBackground,
                ObservationKind::AxTree {
                    tree_revision: read.tree_revision,
                },
                ObservationGeometry {
                    image_width: bound.window.bounds.width.round() as u32,
                    image_height: bound.window.bounds.height.round() as u32,
                    window_width: bound.window.bounds.width,
                    window_height: bound.window.bounds.height,
                },
            )
            .map_err(|e| e.to_string())
    }

    /// 登记观测并签发元素句柄（token = 节点在本次读取中的序号；节点自带
    /// path 供后续派发重解析，不落原生指针）。
    fn issue_ax_element(
        bound: &mut BoundTarget,
        read: &AxTreeRead,
        index: usize,
    ) -> Result<(TargetObservation, ElementHandle), String> {
        let observation = register_ax_observation(bound, read)?;
        let element = bound
            .registry
            .issue_element(&observation.observation_id, &scope(), index as u64)
            .map_err(|e| e.to_string())?;
        Ok((observation, element))
    }

    /// 授权读取一扇绑定窗口的有界树。
    fn read_tree(bound: &BoundTarget) -> Result<AxTreeRead, String> {
        bound
            .registry
            .require_authorized(&bound.handle, &scope())
            .map_err(|e| e.to_string())?;
        let validated = bound
            .registry
            .validate_window(&bound.handle, &scope())
            .map_err(|e| e.to_string())?;
        bound
            .native
            .read_ax_tree(bound.registry.authorizer(), &validated)
            .map_err(|e| e.to_string())
    }

    fn cmd_axtree(bundle_id: &str, needle: Option<&str>) -> Result<Value, String> {
        let mut bound = bind_window(bundle_id, needle)?;
        // 未授权读树必须先于任何 AX 访问拒绝。
        let denied = {
            let authorizer = pawork_computer_use::approval::TargetAuthorizer::default();
            let validated = ValidatedWindow {
                target: TargetIdentity::application(bound.app.identity.clone()),
                instance: bound.app.instance.clone(),
                window: bound.window.window,
                scope: scope(),
            };
            bound
                .native
                .read_ax_tree(&authorizer, &validated)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unexpectedly read".to_string())
        };
        let started = Instant::now();
        let read = read_tree(&bound)?;
        let elapsed_ms = started.elapsed().as_millis();
        let observation = register_ax_observation(&mut bound, &read)?;
        let mut roles: HashMap<String, usize> = HashMap::new();
        for node in &read.nodes {
            *roles.entry(node.role.clone()).or_default() += 1;
        }
        let mut roles: Vec<(String, usize)> = roles.into_iter().collect();
        roles.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(json!({
            "read_without_grant": denied,
            "window_id": bound.window.window.window_id,
            "window_title": bound.window.title,
            "tree_revision": read.tree_revision,
            "observation_id": observation.observation_id.as_str(),
            "nodes": read.nodes.len(),
            "truncation": read.truncation,
            "elapsed_ms": elapsed_ms,
            "root_frame_local": read.nodes[0].frame,
            "window_bounds": [bound.window.bounds.x, bound.window.bounds.y, bound.window.bounds.width, bound.window.bounds.height],
            "roles_top": roles.iter().take(10).map(|(role, count)| json!({ "role": role, "count": count })).collect::<Vec<_>>(),
            "sample": (0..read.nodes.len().min(15)).map(|i| node_summary(&read, i)).collect::<Vec<_>>(),
        }))
    }

    fn cmd_axfind(
        bundle_id: &str,
        role: Option<&str>,
        name: Option<&str>,
        needle: Option<&str>,
    ) -> Result<Value, String> {
        let mut bound = bind_window(bundle_id, needle)?;
        let read = read_tree(&bound)?;
        let query = ElementQuery::new(role, name).map_err(|e| e.to_string())?;
        match lookup_element(&read, &query) {
            ElementLookup::Unique { index } => {
                let (_observation, element) = issue_ax_element(&mut bound, &read, index)?;
                // 越权拒绝：跨 run 与伪造句柄（拒绝不消耗句柄）。
                let cross_run = bound
                    .registry
                    .consume_element(&element, &Scope::new("cu03-probe", "run-2"), &|| false)
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "unexpectedly consumed".to_string());
                let forged: ElementHandle =
                    serde_json::from_str("\"e-1-1\"").map_err(|e| e.to_string())?;
                let forged_result = bound
                    .registry
                    .consume_element(&forged, &scope(), &|| false)
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "unexpectedly consumed".to_string());
                let consumed = bound
                    .registry
                    .consume_element(&element, &scope(), &|| false)
                    .map_err(|e| e.to_string())?;
                Ok(json!({
                    "outcome": "unique",
                    "nodes": read.nodes.len(),
                    "truncation": read.truncation,
                    "matched": node_summary(&read, index),
                    "element_handle": element.as_str(),
                    "consume_cross_run": cross_run,
                    "consume_forged": forged_result,
                    "consumed_element_token": consumed.element_token,
                    "consumed_tree_revision": consumed.tree_revision,
                    "matches_read_revision": consumed.tree_revision == read.tree_revision,
                }))
            }
            ElementLookup::NoMatch { tree_truncated } => Ok(json!({
                "outcome": "no_match",
                "tree_truncated": tree_truncated,
                "nodes": read.nodes.len(),
                "truncation": read.truncation,
            })),
            ElementLookup::Ambiguous {
                matches,
                tree_truncated,
            } => Ok(json!({
                "outcome": "ambiguous",
                "matches": matches,
                "tree_truncated": tree_truncated,
                "nodes": read.nodes.len(),
                "truncation": read.truncation,
            })),
            ElementLookup::UnprovenUnique { index } => Ok(json!({
                "outcome": "unproven_unique",
                "candidate": node_summary(&read, index),
                "nodes": read.nodes.len(),
                "truncation": read.truncation,
            })),
        }
    }

    fn cmd_axstale(bundle_id: &str, needle: Option<&str>) -> Result<Value, String> {
        let mut bound = bind_window(bundle_id, needle)?;
        let pid = bound.app.instance.pid as i32;
        let original = bound.window.bounds;
        let query = ElementQuery::new(Some("AXTextArea"), None).map_err(|e| e.to_string())?;
        let mut steps = Vec::new();
        let read = read_tree(&bound)?;
        // 句柄目标：唯一 AXTextArea，找不到退回根节点（窗口自身）。
        let index = match lookup_element(&read, &query) {
            ElementLookup::Unique { index } => index,
            _ => 0,
        };
        let (_observation, element) = issue_ax_element(&mut bound, &read, index)?;
        steps.push(json!({
            "step": "issued",
            "index": index,
            "role": read.nodes[index].role,
            "tree_revision": read.tree_revision,
            "nodes": read.nodes.len(),
        }));
        // 树变化：缩放窗口 → 元素帧随版面前进 → 版本指纹变化 → TreeChanged。
        let resized = cmd_axsetframe(
            pid,
            original.x,
            original.y,
            original.width + 160.0,
            original.height + 90.0,
            needle,
        )?;
        std::thread::sleep(Duration::from_millis(400));
        let after_resize = bound
            .registry
            .consume_element(&element, &scope(), &|| false)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unexpectedly consumed".to_string());
        steps.push(json!({
            "step": "consume_after_resize",
            "setframe": resized,
            "result": after_resize,
        }));
        // 还原窗口帧后重读重签（拒绝未消耗旧句柄之外的任何状态）。
        let restored = cmd_axsetframe(
            pid,
            original.x,
            original.y,
            original.width,
            original.height,
            needle,
        )?;
        std::thread::sleep(Duration::from_millis(400));
        let read2 = read_tree(&bound)?;
        let index2 = match lookup_element(&read2, &query) {
            ElementLookup::Unique { index } => index,
            _ => 0,
        };
        let (_observation2, element2) = issue_ax_element(&mut bound, &read2, index2)?;
        steps.push(json!({
            "step": "reissued_after_restore",
            "restored": restored,
            "tree_revision": read2.tree_revision,
        }));
        // 销毁：语义关窗 → 句柄按 WindowReplaced 拒绝。
        let closed = cmd_axclose(pid, needle)?;
        std::thread::sleep(Duration::from_millis(600));
        let after_close = bound
            .registry
            .consume_element(&element2, &scope(), &|| false)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unexpectedly consumed".to_string());
        steps.push(json!({
            "step": "consume_after_close",
            "closed": closed,
            "result": after_close,
        }));
        Ok(json!({
            "bundle_id": bound.app.identity.bundle_id,
            "window_id": bound.window.window.window_id,
            "window_title": bound.window.title,
            "steps": steps,
        }))
    }

    // ---------- CU-06 后台 AX 语义动作 ----------

    /// 剪贴板 changeCount（NSPasteboard）：语义动作不碰剪贴板的独立事实。
    fn clipboard_change_count() -> i64 {
        autoreleasepool(|| unsafe {
            let pb: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            if pb.is_null() {
                return -1;
            }
            msg_send![pb, changeCount]
        })
    }

    /// 探针专用布尔属性读回（AXMain / AXFocused 等焦点事实）。
    unsafe fn ax_bool_read(el: AXEl, attr: &str) -> Option<bool> {
        let mut raw: CFTypeRef = ptr::null();
        let err = (axf().copy_attribute)(el, nsstr(attr) as *const c_void, &mut raw);
        let value = if err == 0 && !raw.is_null() && CFGetTypeID(raw) == CFBooleanGetTypeID() {
            Some(CFBooleanGetValue(raw) != 0)
        } else {
            None
        };
        if !raw.is_null() {
            CFRelease(raw);
        }
        value
    }

    /// 验收探针专用的元素读回：按窗口 id 与 path 重解析活元素，读 AXValue
    /// （仅字符串类型）与 AXSelectedTextRange。产品路径（树读取与派发）永不
    /// 读 AXValue；这里是 CU-01 同款的目标侧独立事实取证。
    fn read_element_text(pid: i32, window_id: u32, path: &[u32]) -> Value {
        autoreleasepool(|| unsafe {
            let app = (axf().create_application)(pid);
            if app.is_null() {
                return json!({ "ok": false, "error": "no AX application" });
            }
            let mut windows: CFTypeRef = ptr::null();
            let err =
                (axf().copy_attribute)(app, nsstr("AXWindows") as *const c_void, &mut windows);
            if err != 0 || windows.is_null() {
                CFRelease(app as *const c_void);
                return json!({ "ok": false, "error": format!("AXWindows read: {err}") });
            }
            let count = CFArrayGetCount(windows);
            let mut window: AXEl = ptr::null_mut();
            for i in 0..count {
                let candidate = CFArrayGetValueAtIndex(windows, i) as AXEl;
                if candidate.is_null() {
                    continue;
                }
                let mut id: u32 = 0;
                if (axf().get_window)(candidate, &mut id) == 0 && id == window_id {
                    window = candidate;
                    break;
                }
            }
            if window.is_null() {
                CFRelease(windows);
                CFRelease(app as *const c_void);
                return json!({ "ok": false, "error": "window element not found" });
            }
            // 逐级读 AXChildren 下钻；每层数组保活到读回完成。
            let mut level_arrays: Vec<CFTypeRef> = vec![windows];
            let mut current = window;
            let mut failed: Option<String> = None;
            for &index in path {
                let mut children: CFTypeRef = ptr::null();
                let err = (axf().copy_attribute)(
                    current,
                    nsstr("AXChildren") as *const c_void,
                    &mut children,
                );
                if err != 0 || children.is_null() {
                    if !children.is_null() {
                        CFRelease(children);
                    }
                    failed = Some(format!("children read: {err}"));
                    break;
                }
                let n = CFArrayGetCount(children);
                let child = if (index as isize) < n {
                    CFArrayGetValueAtIndex(children, index as isize) as AXEl
                } else {
                    ptr::null_mut()
                };
                if child.is_null() {
                    CFRelease(children);
                    failed = Some(format!("path index {index} out of {n} children"));
                    break;
                }
                level_arrays.push(children);
                current = child;
            }
            let result = if let Some(error) = failed {
                json!({ "ok": false, "error": error })
            } else {
                let mut raw_value: CFTypeRef = ptr::null();
                let value_err = (axf().copy_attribute)(
                    current,
                    nsstr("AXValue") as *const c_void,
                    &mut raw_value,
                );
                let value = if value_err == 0
                    && !raw_value.is_null()
                    && CFGetTypeID(raw_value) == CFStringGetTypeID()
                {
                    Some(rs_str(raw_value as *mut Object))
                } else {
                    None
                };
                if !raw_value.is_null() {
                    CFRelease(raw_value);
                }
                let mut raw_range: CFTypeRef = ptr::null();
                let range_err = (axf().copy_attribute)(
                    current,
                    nsstr("AXSelectedTextRange") as *const c_void,
                    &mut raw_range,
                );
                // kAXValueTypeCFRange = 4：{location, length} 两个 CFIndex。
                let mut pair = [0i64; 2];
                let range = if range_err == 0
                    && !raw_range.is_null()
                    && (axf().value_get_value)(raw_range, 4, pair.as_mut_ptr() as *mut c_void)
                {
                    Some(pair)
                } else {
                    None
                };
                if !raw_range.is_null() {
                    CFRelease(raw_range);
                }
                let value_summary = value.as_ref().map(|text| {
                    json!({
                        "chars": text.chars().count(),
                        // 全文 ≤2048 字符直接带，超出带头部样本。
                        "text": if text.chars().count() <= 2048 { text.clone() } else { text.chars().take(120).collect() },
                    })
                });
                json!({
                    "ok": true,
                    "value": value_summary,
                    "value_err": value_err,
                    "selected_range": range,
                    "range_err": range_err,
                    // CU-07 焦点事实：窗口是否 app 内 key window（AXMain）
                    // 与元素是否聚焦（AXFocused）——定向输入的路由依据。
                    "window_main": ax_bool_read(window, "AXMain"),
                    "element_focused": ax_bool_read(current, "AXFocused"),
                })
            };
            for array in level_arrays {
                CFRelease(array);
            }
            CFRelease(app as *const c_void);
            result
        })
    }

    /// CU-06 全流程：绑定 → 读树 → 唯一定位 → 矩阵预检 → 签发/消费句柄 →
    /// 后端派发 → 读回复核 → 无干扰采样。能力矩阵的预检结果与后端权威闸
    /// 都入报告（预检拒绝时仍继续走完消费与后端调用，收集两类拒绝证据）。
    fn cmd_axact(
        bundle_id: &str,
        action_name: &str,
        role: Option<&str>,
        name: Option<&str>,
        text: Option<&str>,
        needle: Option<&str>,
    ) -> Result<Value, String> {
        let action = match action_name {
            "press" => SemanticAction::Press,
            "set_value" => SemanticAction::SetValue(
                text.filter(|t| *t != "-")
                    .ok_or("set_value 需要 text 参数")?
                    .to_string(),
            ),
            "insert_text" => SemanticAction::InsertText(
                text.filter(|t| *t != "-")
                    .ok_or("insert_text 需要 text 参数")?
                    .to_string(),
            ),
            other => return Err(format!("未知语义动作: {other}")),
        };
        let before = sample_user_settled();
        let clipboard_before = clipboard_change_count();
        let mut bound = bind_window(bundle_id, needle)?;
        let title_before = bound.window.title.clone();
        let read = read_tree(&bound)?;
        let query = ElementQuery::new(role, name).map_err(|e| e.to_string())?;
        let index = match lookup_element(&read, &query) {
            ElementLookup::Unique { index } => index,
            ElementLookup::NoMatch { tree_truncated } => {
                return Ok(json!({ "outcome": "no_match", "tree_truncated": tree_truncated }))
            }
            ElementLookup::Ambiguous {
                matches,
                tree_truncated,
            } => {
                return Ok(
                    json!({ "outcome": "ambiguous", "matches": matches, "tree_truncated": tree_truncated }),
                )
            }
            ElementLookup::UnprovenUnique { .. } => {
                return Ok(json!({ "outcome": "unproven_unique" }))
            }
        };
        let node = &read.nodes[index];
        let pid = bound.app.instance.pid as i32;
        let window_id = bound.window.window.window_id;
        // 能力矩阵预检（静态族 × 动作；后端派发前另有权威闸）。
        let pre_check = match require_background(bound.app.identity.family, action.kind(), false) {
            Ok(()) => "supported".to_string(),
            Err(e) => e.to_string(),
        };
        let value_before = read_element_text(pid, window_id as u32, &node.path);
        let (_observation, element) = issue_ax_element(&mut bound, &read, index)?;
        let consumed = match bound
            .registry
            .consume_element(&element, &scope(), &|| false)
        {
            Ok(consumed) => consumed,
            Err(e) => {
                return Ok(json!({
                    "outcome": "consume_rejected",
                    "error": e.to_string(),
                    "pre_check": pre_check,
                }))
            }
        };
        let dispatch = bound.native.semantic_action(
            bound.registry.authorizer_mut(),
            &consumed,
            &node.path,
            &action,
            &|| false,
        );
        let (outcome, dispatch_error) = match &dispatch {
            Ok(pawork_computer_use::target::SemanticOutcome::Dispatched) => {
                ("dispatched".to_string(), None)
            }
            Ok(pawork_computer_use::target::SemanticOutcome::UnknownEffect) => {
                ("unknown_effect".to_string(), None)
            }
            Err(e) => ("rejected".to_string(), Some(e.to_string())),
        };
        // 给目标应用一点生效时间后复核：AXValue / 选区读回与窗口标题。
        std::thread::sleep(Duration::from_millis(500));
        let value_after = read_element_text(pid, window_id as u32, &node.path);
        let title_after = bound
            .native
            .list_windows(
                bound.registry.authorizer(),
                &scope(),
                &bound.app.identity,
                &bound.app.instance,
                true,
            )
            .ok()
            .and_then(|windows| {
                windows
                    .into_iter()
                    .find(|w| w.window.window_id == window_id)
                    .and_then(|w| w.title)
            });
        let revision_after = read_tree(&bound).ok().map(|r| r.tree_revision);
        let after = sample_user_settled();
        let clipboard_after = clipboard_change_count();
        Ok(json!({
            "bundle_id": bound.app.identity.bundle_id,
            "family": bound.app.identity.family,
            "window_id": window_id,
            "action": action_name,
            "matched": node_summary(&read, index),
            "pre_check": pre_check,
            "outcome": outcome,
            "dispatch_error": dispatch_error,
            "value_before": value_before,
            "value_after": value_after,
            "title_before": title_before,
            "title_after": title_after,
            "tree_revision_before": read.tree_revision,
            "tree_revision_after": revision_after,
            "front_unchanged": before["front_pid"] == after["front_pid"],
            "focus_unchanged": before["focus_pid"] == after["focus_pid"]
                && before["focus_pid"] != 0,
            "focus_observed": before["focus_observed"] == true && after["focus_observed"] == true,
            "mouse_delta": [
                after["mouse"][0].as_f64().unwrap_or(0.0) - before["mouse"][0].as_f64().unwrap_or(0.0),
                after["mouse"][1].as_f64().unwrap_or(0.0) - before["mouse"][1].as_f64().unwrap_or(0.0),
            ],
            "clipboard_before": clipboard_before,
            "clipboard_after": clipboard_after,
            "clipboard_unchanged": clipboard_before == clipboard_after,
            "user_before": before,
            "user_after": after,
        }))
    }

    // ---------- CU-07 后台定向文本与按键输入 ----------

    fn parse_targeted_key(name: &str) -> Result<TargetedKey, String> {
        match name {
            "return" => Ok(TargetedKey::Return),
            "tab" => Ok(TargetedKey::Tab),
            "space" => Ok(TargetedKey::Space),
            "backspace" => Ok(TargetedKey::Backspace),
            "escape" => Ok(TargetedKey::Escape),
            "delete" => Ok(TargetedKey::Delete),
            "home" => Ok(TargetedKey::Home),
            "end" => Ok(TargetedKey::End),
            "page_up" => Ok(TargetedKey::PageUp),
            "page_down" => Ok(TargetedKey::PageDown),
            "left" => Ok(TargetedKey::Left),
            "up" => Ok(TargetedKey::Up),
            "right" => Ok(TargetedKey::Right),
            "down" => Ok(TargetedKey::Down),
            other => Err(format!("未知按键: {other}")),
        }
    }

    fn parse_modifiers(raw: Option<&str>) -> Result<KeyModifiers, String> {
        let mut modifiers = KeyModifiers::none();
        let Some(raw) = raw.filter(|value| !value.is_empty() && *value != "-") else {
            return Ok(modifiers);
        };
        for modifier in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match modifier {
                "shift" => modifiers.shift = true,
                "control" | "ctrl" => modifiers.control = true,
                "option" | "alt" => modifiers.option = true,
                other => return Err(format!("未知修饰键: {other}")),
            }
        }
        Ok(modifiers)
    }

    /// CU-07 全流程：绑定 → 读树 → 唯一定位 → 矩阵预检 → 签发/消费句柄 →
    /// 后端定向派发（能力闸 / 最小化 / AXMain / 焦点 / 选区复核）→ 读回
    /// 复核 → 无干扰采样。预检结果与后端权威闸都入报告（预检拒绝时仍
    /// 走完消费与后端调用，收集两类拒绝证据）。
    #[allow(clippy::too_many_arguments)]
    fn cmd_axinput(
        bundle_id: &str,
        action_name: &str,
        role: Option<&str>,
        name: Option<&str>,
        payload: &str,
        mods: Option<&str>,
        needle: Option<&str>,
    ) -> Result<Value, String> {
        let input = match action_name {
            "insert_text" => TargetedInput::InsertText(payload.to_string()),
            "key_press" => TargetedInput::KeyPress(TargetedKeyPress {
                key: parse_targeted_key(payload)?,
                modifiers: parse_modifiers(mods)?,
            }),
            other => return Err(format!("未知定向输入动作: {other}")),
        };
        let before = sample_user_settled();
        let clipboard_before = clipboard_change_count();
        let mut bound = bind_window(bundle_id, needle)?;
        let title_before = bound.window.title.clone();
        let read = read_tree(&bound)?;
        let query = ElementQuery::new(role, name).map_err(|e| e.to_string())?;
        let index = match lookup_element(&read, &query) {
            ElementLookup::Unique { index } => index,
            ElementLookup::NoMatch { tree_truncated } => {
                return Ok(json!({ "outcome": "no_match", "tree_truncated": tree_truncated }))
            }
            ElementLookup::Ambiguous {
                matches,
                tree_truncated,
            } => {
                return Ok(
                    json!({ "outcome": "ambiguous", "matches": matches, "tree_truncated": tree_truncated }),
                )
            }
            ElementLookup::UnprovenUnique { .. } => {
                return Ok(json!({ "outcome": "unproven_unique" }))
            }
        };
        let node = &read.nodes[index];
        let pid = bound.app.instance.pid as i32;
        let window_id = bound.window.window.window_id;
        // 能力矩阵预检（静态族 × 动作；后端派发前另有权威闸，且后端
        // 还复核最小化 / AXMain / 焦点 / 选区）。
        let pre_check = match require_background(
            bound.app.identity.family,
            input.kind(),
            false,
        ) {
            Ok(()) => "supported".to_string(),
            Err(e) => e.to_string(),
        };
        let value_before = read_element_text(pid, window_id as u32, &node.path);
        let (_observation, element) = issue_ax_element(&mut bound, &read, index)?;
        let consumed = match bound
            .registry
            .consume_element(&element, &scope(), &|| false)
        {
            Ok(consumed) => consumed,
            Err(e) => {
                return Ok(json!({
                    "outcome": "consume_rejected",
                    "error": e.to_string(),
                    "pre_check": pre_check,
                }))
            }
        };
        let dispatch = bound.native.targeted_input(
            bound.registry.authorizer_mut(),
            &consumed,
            &node.path,
            &input,
            &|| false,
        );
        let (outcome, events, cause, dispatch_error) = match &dispatch {
            Ok(TargetedOutcome::Dispatched {
                events_posted,
                events_total,
            }) => (
                "dispatched".to_string(),
                json!({ "posted": events_posted, "total": events_total }),
                None,
                None,
            ),
            Ok(TargetedOutcome::Partial {
                events_posted,
                events_total,
                cause,
            }) => (
                "partial".to_string(),
                json!({ "posted": events_posted, "total": events_total }),
                Some(format!("{cause}")),
                None,
            ),
            Err(e) => ("rejected".to_string(), Value::Null, None, Some(e.to_string())),
        };
        // 给目标应用一点生效时间后复核：AXValue / 选区读回（含 AXMain /
        // AXFocused 焦点事实）与窗口标题。
        std::thread::sleep(Duration::from_millis(500));
        let value_after = read_element_text(pid, window_id as u32, &node.path);
        let title_after = bound
            .native
            .list_windows(
                bound.registry.authorizer(),
                &scope(),
                &bound.app.identity,
                &bound.app.instance,
                true,
            )
            .ok()
            .and_then(|windows| {
                windows
                    .into_iter()
                    .find(|w| w.window.window_id == window_id)
                    .and_then(|w| w.title)
            });
        let revision_after = read_tree(&bound).ok().map(|r| r.tree_revision);
        let after = sample_user_settled();
        let clipboard_after = clipboard_change_count();
        Ok(json!({
            "bundle_id": bound.app.identity.bundle_id,
            "family": bound.app.identity.family,
            "window_id": window_id,
            "action": action_name,
            "payload": if action_name == "insert_text" { payload } else { "" },
            "key": if action_name == "key_press" { payload } else { "" },
            "mods": mods.unwrap_or("-"),
            "matched": node_summary(&read, index),
            "pre_check": pre_check,
            "outcome": outcome,
            "events": events,
            "partial_cause": cause,
            "dispatch_error": dispatch_error,
            "value_before": value_before,
            "value_after": value_after,
            "title_before": title_before,
            "title_after": title_after,
            "tree_revision_before": read.tree_revision,
            "tree_revision_after": revision_after,
            "front_unchanged": before["front_pid"] == after["front_pid"],
            "focus_unchanged": before["focus_pid"] == after["focus_pid"]
                && before["focus_pid"] != 0,
            "focus_observed": before["focus_observed"] == true && after["focus_observed"] == true,
            "mouse_delta": [
                after["mouse"][0].as_f64().unwrap_or(0.0) - before["mouse"][0].as_f64().unwrap_or(0.0),
                after["mouse"][1].as_f64().unwrap_or(0.0) - before["mouse"][1].as_f64().unwrap_or(0.0),
            ],
            "clipboard_before": clipboard_before,
            "clipboard_after": clipboard_after,
            "clipboard_unchanged": clipboard_before == clipboard_after,
            "user_before": before,
            "user_after": after,
        }))
    }

    /// CU-08 全流程（指针动作拒绝路径）：绑定 → 截图观测 → 矩阵预检 →
    /// 观测消费 → 后端 pointer_action → 复核零效果与无干扰。能力矩阵
    /// 当前对三族应用全 Unsupported，预期终态即 BackgroundUnsupported、
    /// 零派发；预检与后端权威闸结果都入报告。
    fn cmd_axpointer(
        bundle_id: &str,
        action_name: &str,
        coords: &[f64],
        needle: Option<&str>,
    ) -> Result<Value, String> {
        fn point(coords: &[f64], i: usize) -> Result<WindowPoint, String> {
            Ok(WindowPoint {
                x: *coords.get(i).ok_or("缺少坐标参数")?,
                y: *coords.get(i + 1).ok_or("缺少坐标参数")?,
            })
        }
        let action = match action_name {
            "click" => PointerAction::Click { point: point(coords, 0)? },
            "drag" => PointerAction::Drag {
                from: point(coords, 0)?,
                to: point(coords, 2)?,
            },
            "scroll" => PointerAction::Scroll {
                point: point(coords, 0)?,
                delta_x: *coords.get(2).ok_or("缺少滚动增量参数")?,
                delta_y: *coords.get(3).ok_or("缺少滚动增量参数")?,
            },
            other => return Err(format!("未知指针动作: {other}")),
        };
        let before = sample_user_settled();
        let clipboard_before = clipboard_change_count();
        let mut bound = bind_window(bundle_id, needle)?;
        let window_id = bound.window.window.window_id;
        let (_capture, observation) = observe(&mut bound)?;
        let pre_check = match require_background(
            bound.app.identity.family,
            action.kind(),
            false,
        ) {
            Ok(()) => "supported".to_string(),
            Err(e) => e.to_string(),
        };
        let consumed = match bound
            .registry
            .consume_observation(&observation.observation_id, &scope(), &|| false)
        {
            Ok(consumed) => consumed,
            Err(e) => {
                return Ok(json!({
                    "outcome": "consume_rejected",
                    "error": e.to_string(),
                    "pre_check": pre_check,
                }))
            }
        };
        let dispatch = bound.native.pointer_action(
            bound.registry.authorizer_mut(),
            &consumed,
            &action,
            &|| false,
        );
        let (outcome, dispatch_error) = match &dispatch {
            Ok(()) => ("dispatched".to_string(), None),
            Err(e) => ("rejected".to_string(), Some(e.to_string())),
        };
        // 复核：零效果（标题不变）与无干扰（前台 / 焦点 / 鼠标 /
        // 剪贴板）。
        std::thread::sleep(Duration::from_millis(300));
        let title_after = bound
            .native
            .list_windows(
                bound.registry.authorizer(),
                &scope(),
                &bound.app.identity,
                &bound.app.instance,
                true,
            )
            .ok()
            .and_then(|windows| {
                windows
                    .into_iter()
                    .find(|w| w.window.window_id == window_id)
                    .and_then(|w| w.title)
            });
        let after = sample_user_settled();
        let clipboard_after = clipboard_change_count();
        Ok(json!({
            "bundle_id": bound.app.identity.bundle_id,
            "family": bound.app.identity.family,
            "window_id": window_id,
            "action": action_name,
            "coords": coords,
            "pre_check": pre_check,
            "outcome": outcome,
            "dispatch_error": dispatch_error,
            "title_before": bound.window.title,
            "title_after": title_after,
            "front_unchanged": before["front_pid"] == after["front_pid"],
            "focus_unchanged": before["focus_pid"] == after["focus_pid"]
                && before["focus_pid"] != 0,
            "focus_observed": before["focus_observed"] == true && after["focus_observed"] == true,
            "mouse_delta": [
                after["mouse"][0].as_f64().unwrap_or(0.0) - before["mouse"][0].as_f64().unwrap_or(0.0),
                after["mouse"][1].as_f64().unwrap_or(0.0) - before["mouse"][1].as_f64().unwrap_or(0.0),
            ],
            "clipboard_unchanged": clipboard_before == clipboard_after,
            "user_before": before,
            "user_after": after,
        }))
    }

    /// CU-08 全流程（窗口移动）：绑定 → 读原帧 → 矩阵预检 → 窗口派发
    /// 消费 → 后端 move_window → 读回复核新帧 → 还原原位 → 无干扰采样。
    fn cmd_axmove(
        bundle_id: &str,
        x: f64,
        y: f64,
        needle: Option<&str>,
    ) -> Result<Value, String> {
        let before = sample_user_settled();
        let clipboard_before = clipboard_change_count();
        let mut bound = bind_window(bundle_id, needle)?;
        let window_id = bound.window.window.window_id;
        let frame_before = [
            bound.window.bounds.x,
            bound.window.bounds.y,
            bound.window.bounds.width,
            bound.window.bounds.height,
        ];
        let move_to = WindowMove { x, y };
        let pre_check = match require_background(
            bound.app.identity.family,
            move_to.kind(),
            false,
        ) {
            Ok(()) => "supported".to_string(),
            Err(e) => e.to_string(),
        };
        let consumed = match bound
            .registry
            .consume_window_dispatch(&bound.handle, &scope(), &|| false)
        {
            Ok(consumed) => consumed,
            Err(e) => {
                return Ok(json!({
                    "outcome": "consume_rejected",
                    "error": e.to_string(),
                    "pre_check": pre_check,
                }))
            }
        };
        let dispatch = bound.native.move_window(
            bound.registry.authorizer_mut(),
            &consumed,
            &move_to,
            &|| false,
        );
        let (outcome, dispatch_error) = match &dispatch {
            Ok(SemanticOutcome::Dispatched) => ("dispatched".to_string(), None),
            Ok(SemanticOutcome::UnknownEffect) => ("unknown_effect".to_string(), None),
            Err(e) => ("rejected".to_string(), Some(e.to_string())),
        };
        // 读回复核：窗口服务器帧应到达目标原点（尺寸不变）。
        std::thread::sleep(Duration::from_millis(500));
        let frame_after = bound
            .native
            .list_windows(
                bound.registry.authorizer(),
                &scope(),
                &bound.app.identity,
                &bound.app.instance,
                true,
            )
            .ok()
            .and_then(|windows| {
                windows
                    .into_iter()
                    .find(|w| w.window.window_id == window_id)
                    .map(|w| [w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height])
            });
        // 还原原位（独立一次消费与派发），失败如实上报。是否还原由读回
        // 帧驱动：UnknownEffect 时窗口可能已移动；rejected 但读回显示已
        // 移动属异常，同样还原并如实上报；读回帧缺失时恢复状态记未知。
        let restore = (|| -> Result<Value, String> {
            let Some(frame_after_value) = frame_after else {
                return Ok(json!({ "needed": false, "state": "unknown" }));
            };
            let moved = (frame_after_value[0] - frame_before[0]).abs() > 0.5
                || (frame_after_value[1] - frame_before[1]).abs() > 0.5;
            if !moved {
                return Ok(json!({ "needed": false, "state": "unmoved" }));
            }
            let consumed = bound
                .registry
                .consume_window_dispatch(&bound.handle, &scope(), &|| false)
                .map_err(|e| e.to_string())?;
            let back = WindowMove {
                x: frame_before[0],
                y: frame_before[1],
            };
            let result = bound.native.move_window(
                bound.registry.authorizer_mut(),
                &consumed,
                &back,
                &|| false,
            );
            std::thread::sleep(Duration::from_millis(300));
            let frame_restored = bound
                .native
                .list_windows(
                    bound.registry.authorizer(),
                    &scope(),
                    &bound.app.identity,
                    &bound.app.instance,
                    true,
                )
                .ok()
                .and_then(|windows| {
                    windows
                        .into_iter()
                        .find(|w| w.window.window_id == window_id)
                        .map(|w| [w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height])
                });
            Ok(json!({
                "needed": true,
                "outcome": match &result {
                    Ok(SemanticOutcome::Dispatched) => "dispatched",
                    Ok(SemanticOutcome::UnknownEffect) => "unknown_effect",
                    Err(_) => "rejected",
                },
                "error": result.err().map(|e| e.to_string()),
                "frame": frame_restored,
                "moved_while_rejected": dispatch.is_err(),
            }))
        })()?;
        let after = sample_user_settled();
        let clipboard_after = clipboard_change_count();
        Ok(json!({
            "bundle_id": bound.app.identity.bundle_id,
            "family": bound.app.identity.family,
            "window_id": window_id,
            "target_origin": [x, y],
            "pre_check": pre_check,
            "outcome": outcome,
            "dispatch_error": dispatch_error,
            "frame_before": frame_before,
            "frame_after": frame_after,
            "origin_reached": frame_after
                .map(|f| {
                    (f[0] - x).abs() <= 0.5
                        && (f[1] - y).abs() <= 0.5
                        && f[2] == frame_before[2]
                        && f[3] == frame_before[3]
                })
                .unwrap_or(false),
            "restore": restore,
            "front_unchanged": before["front_pid"] == after["front_pid"],
            "focus_unchanged": before["focus_pid"] == after["focus_pid"]
                && before["focus_pid"] != 0,
            "focus_observed": before["focus_observed"] == true && after["focus_observed"] == true,
            "mouse_delta": [
                after["mouse"][0].as_f64().unwrap_or(0.0) - before["mouse"][0].as_f64().unwrap_or(0.0),
                after["mouse"][1].as_f64().unwrap_or(0.0) - before["mouse"][1].as_f64().unwrap_or(0.0),
            ],
            "clipboard_unchanged": clipboard_before == clipboard_after,
            "user_before": before,
            "user_after": after,
        }))
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
            Some("axunminimize") => cmd_axunminimize(
                args.get(1)
                    .ok_or("axunminimize 需要 pid")?
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
            Some("capture") => cmd_capture(
                args.get(1).ok_or("capture 需要 bundle_id")?,
                args.get(2).ok_or("capture 需要 jpeg 输出路径")?,
                args.get(3).map(String::as_str),
            ),
            Some("obsflow") => cmd_obsflow(
                args.get(1).ok_or("obsflow 需要 bundle_id")?,
                args.get(2).map(String::as_str),
            ),
            Some("capturegone") => cmd_capturegone(
                args.get(1).ok_or("capturegone 需要 bundle_id")?,
                args.get(2).map(String::as_str),
            ),
            Some("axtree") => cmd_axtree(
                args.get(1).ok_or("axtree 需要 bundle_id")?,
                args.get(2).map(String::as_str),
            ),
            Some("axfind") => cmd_axfind(
                args.get(1).ok_or("axfind 需要 bundle_id")?,
                args.get(2).map(String::as_str).filter(|s| s != &"-"),
                args.get(3).map(String::as_str).filter(|s| s != &"-"),
                args.get(4).map(String::as_str),
            ),
            Some("axstale") => cmd_axstale(
                args.get(1).ok_or("axstale 需要 bundle_id")?,
                args.get(2).map(String::as_str),
            ),
            Some("axact") => cmd_axact(
                args.get(1).ok_or("axact 需要 bundle_id")?,
                args.get(2)
                    .ok_or("axact 需要动作：press|set_value|insert_text")?,
                args.get(3).map(String::as_str).filter(|s| s != &"-"),
                args.get(4).map(String::as_str).filter(|s| s != &"-"),
                args.get(5).map(String::as_str),
                args.get(6).map(String::as_str),
            ),
            Some("axinput") => cmd_axinput(
                args.get(1).ok_or("axinput 需要 bundle_id")?,
                args.get(2)
                    .ok_or("axinput 需要动作：insert_text|key_press")?,
                args.get(3).map(String::as_str).filter(|s| s != &"-"),
                args.get(4).map(String::as_str).filter(|s| s != &"-"),
                args.get(5)
                    .map(String::as_str)
                    .filter(|s| s != &"-")
                    .ok_or("axinput 需要 text 或 key 参数")?,
                args.get(6).map(String::as_str).filter(|s| s != &"-"),
                args.get(7).map(String::as_str),
            ),
            Some("axpointer") => {
                let bundle_id = args.get(1).ok_or("axpointer 需要 bundle_id")?;
                let action_name = args
                    .get(2)
                    .ok_or("axpointer 需要动作：click|drag|scroll")?;
                let mut coords = Vec::new();
                let mut needle = None;
                for arg in &args[3..] {
                    match arg.parse::<f64>() {
                        Ok(value) => coords.push(value),
                        Err(_) => {
                            needle = Some(arg.clone());
                            break;
                        }
                    }
                }
                cmd_axpointer(bundle_id, action_name, &coords, needle.as_deref())
            }
            Some("axmove") => cmd_axmove(
                args.get(1).ok_or("axmove 需要 bundle_id")?,
                args.get(2).ok_or("axmove 需要 x")?.parse().map_err(|_| "x 非法")?,
                args.get(3).ok_or("axmove 需要 y")?.parse().map_err(|_| "y 非法")?,
                args.get(4).map(String::as_str),
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
