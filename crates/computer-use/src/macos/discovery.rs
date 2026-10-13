//! Process instances, activation-free launch, application-family classification, and window enumeration.

use super::*;

// ---------- process instances ----------

/// Start token of a live process: kern.proc.pid start time in microseconds
/// since the epoch. In kinfo_proc the leading extern_proc begins with a
/// union whose first alternative is p_starttime, so the start timeval sits
/// at offset 0 of the whole struct; reading only the head avoids depending
/// on the full (kernel-version-specific) layout. None once the pid is gone.
pub(super) fn process_start_token(pid: i32) -> Option<u64> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    unsafe {
        let mut size: libc::size_t = 0;
        if libc::sysctl(
            mib.as_mut_ptr(),
            4,
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
            0,
        ) != 0
            || size < 16
        {
            return None;
        }
        let mut buffer = vec![0u8; size];
        if libc::sysctl(
            mib.as_mut_ptr(),
            4,
            buffer.as_mut_ptr() as *mut c_void,
            &mut size,
            ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        // struct timeval on 64-bit macOS: time_t tv_sec (i64), suseconds_t
        // tv_usec (i32).
        let seconds = i64::from_ne_bytes(buffer[0..8].try_into().ok()?);
        let micros = i32::from_ne_bytes(buffer[8..12].try_into().ok()?) as i64;
        if seconds <= 0 || !(0..1_000_000).contains(&micros) {
            return None;
        }
        Some(seconds as u64 * 1_000_000 + micros as u64)
    }
}

/// The bundle id a pid actually belongs to, read back from the system:
/// the binding check that keeps a grant for one application from
/// unlocking reads of another process. None when the pid is not a
/// running application.
fn running_bundle_id(pid: i32) -> Option<String> {
    autoreleasepool(|| unsafe {
        let app: *mut Object = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid
        ];
        if app.is_null() {
            return None;
        }
        let bundle: *mut Object = msg_send![app, bundleIdentifier];
        if bundle.is_null() {
            return None;
        }
        Some(rs_str(bundle))
    })
}

/// The granted identity must own this process: the bundle id is read back
/// from the system so a grant for one application never unlocks another
/// process's windows. Listing and capture share this binding.
pub(super) fn verify_instance_owns_bundle(bundle_id: &str, pid: i32) -> Result<(), TargetError> {
    if running_bundle_id(pid).as_deref() != Some(bundle_id) {
        return Err(TargetError::Invalid(
            "process instance does not belong to the authorized identity",
        ));
    }
    Ok(())
}

/// NSRunningApplication termination state, when the pid is an application:
/// Some(true) catches the zombie window between process exit and reaping.
pub(super) fn ns_app_terminated(pid: i32) -> Option<bool> {
    autoreleasepool(|| unsafe {
        let app: *mut Object = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid
        ];
        if app.is_null() {
            return None;
        }
        // The property is named terminated; its getter is isTerminated.
        let terminated: BOOL = msg_send![app, isTerminated];
        Some(terminated == YES)
    })
}

// ---------- controlled launch ----------

/// Test-only proof that denied launches never reach LaunchServices.
#[cfg(test)]
pub(super) static LAUNCH_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Test-only proof that denied captures never reach ScreenCaptureKit.
#[cfg(test)]
pub(super) static CAPTURE_SYSTEM_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub(super) fn launch(identity: &AppIdentity) -> Result<ProcessInstance, LaunchError> {
    #[cfg(test)]
    LAUNCH_SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    autoreleasepool(|| unsafe {
        let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let url: *mut Object =
            msg_send![ws, URLForApplicationWithBundleIdentifier: nsstr(&identity.bundle_id)];
        if url.is_null() {
            return Err(LaunchError::NotFound(identity.bundle_id.clone()));
        }
        let mut error: *mut Object = ptr::null_mut();
        let app: *mut Object = msg_send![
            ws,
            launchApplicationAtURL: url
            options: LAUNCH_WITHOUT_ACTIVATION
            configuration: ptr::null_mut::<Object>()
            error: &mut error
        ];
        if app.is_null() {
            let description: *mut Object = msg_send![error, localizedDescription];
            let reason = rs_str(description);
            return Err(LaunchError::Failed(if reason.is_empty() {
                "LaunchServices returned no application".to_string()
            } else {
                reason
            }));
        }
        let pid: i32 = msg_send![app, processIdentifier];
        if pid <= 0 {
            return Err(LaunchError::Failed(
                "LaunchServices returned no process".to_string(),
            ));
        }
        process_start_token(pid)
            .map(|start_token| ProcessInstance {
                pid: pid as u32,
                start_token,
            })
            .ok_or(LaunchError::ExitedEarly)
    })
}

// ---------- application family classification ----------

/// Capability family from bundle facts. Declaring the http/https URL scheme
/// marks a browser (covers WebKit/Gecko browsers too); a bundled Chromium
/// framework marks Electron/CEF apps; everything else is AppKit. Heuristic
/// feeding only the CU-01 background capability matrix — never a security
/// decision.
pub(super) fn classify(chromium_embedded: bool, declares_web_scheme: bool) -> AppFamily {
    if declares_web_scheme {
        AppFamily::Browser
    } else if chromium_embedded {
        AppFamily::Chromium
    } else {
        AppFamily::Appkit
    }
}

pub(super) fn classify_bundle(bundle_url: *mut Object) -> AppFamily {
    autoreleasepool(|| unsafe {
        if bundle_url.is_null() {
            return AppFamily::Appkit;
        }
        let bundle: *mut Object = msg_send![class!(NSBundle), bundleWithURL: bundle_url];
        if bundle.is_null() {
            return AppFamily::Appkit;
        }
        let types: *mut Object =
            msg_send![bundle, objectForInfoDictionaryKey: nsstr("CFBundleURLTypes")];
        let declares_web = declares_web_scheme(types);
        let path = rs_str(msg_send![bundle, bundlePath]);
        let chromium_embedded = [
            "Electron Framework.framework",
            "Chromium Embedded Framework.framework",
        ]
        .iter()
        .any(|framework| {
            Path::new(&path)
                .join("Contents/Frameworks")
                .join(framework)
                .exists()
        });
        classify(chromium_embedded, declares_web)
    })
}

/// CFBundleURLTypes → any entry whose CFBundleURLSchemes contains http or
/// https (case-insensitive).
fn declares_web_scheme(types: *mut Object) -> bool {
    unsafe {
        if types.is_null() {
            return false;
        }
        let count: usize = msg_send![types, count];
        for i in 0..count {
            let entry: *mut Object = msg_send![types, objectAtIndex: i];
            if entry.is_null() {
                continue;
            }
            let schemes: *mut Object = msg_send![entry, objectForKey: nsstr("CFBundleURLSchemes")];
            if schemes.is_null() {
                continue;
            }
            let scheme_count: usize = msg_send![schemes, count];
            for j in 0..scheme_count {
                let scheme: *mut Object = msg_send![schemes, objectAtIndex: j];
                let value = rs_str(scheme).to_ascii_lowercase();
                if value == "http" || value == "https" {
                    return true;
                }
            }
        }
        false
    }
}

// ---------- window enumeration ----------

/// titles_for_pid gates title copying: kCGWindowName (window content) is
/// copied out of a window's dictionary only when the window belongs to
/// that owner — callers pass the authorized target's pid, and no other
/// application's title is ever read into memory. None copies no titles.
pub(super) fn enumerate_windows(titles_for_pid: Option<i32>) -> Vec<RawWindow> {
    // The kCGWindow* keys are immutable immutable framework constants;
    // reading them needs an unsafe block.
    unsafe fn keys() -> (
        CFStringRef,
        CFStringRef,
        CFStringRef,
        CFStringRef,
        CFStringRef,
    ) {
        (
            cgwindow::kCGWindowNumber,
            cgwindow::kCGWindowOwnerPID,
            cgwindow::kCGWindowLayer,
            cgwindow::kCGWindowName,
            cgwindow::kCGWindowBounds,
        )
    }
    let (k_number, k_owner, k_layer, k_name, k_bounds) = unsafe { keys() };
    let Some(array) = cgwindow::copy_window_info(
        cgwindow::kCGWindowListOptionAll | cgwindow::kCGWindowListExcludeDesktopElements,
        cgwindow::kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for pointer in array.get_all_values() {
        let info: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(pointer as CFDictionaryRef) };
        let Some(id) = number(&info, k_number) else {
            continue;
        };
        let Some(owner_pid) = number(&info, k_owner) else {
            continue;
        };
        let owner_pid = owner_pid as i32;
        out.push(RawWindow {
            id: id as u32,
            owner_pid,
            layer: number(&info, k_layer).unwrap_or(0.0) as i32,
            bounds: window_bounds(&info, k_bounds).unwrap_or(WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            }),
            title: if titles_for_pid == Some(owner_pid) {
                string(&info, k_name)
            } else {
                None
            },
        });
    }
    out
}

fn dict_value(
    dict: &CFDictionary<CFString, CFType>,
    key: CFStringRef,
) -> Option<ItemRef<'_, CFType>> {
    let key = unsafe { CFString::wrap_under_get_rule(key) };
    dict.find(&key)
}

fn number(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<f64> {
    let value = dict_value(dict, key)?;
    if !value.instance_of::<CFNumber>() {
        return None;
    }
    let number =
        unsafe { CFNumber::wrap_under_get_rule(value.as_concrete_TypeRef() as CFNumberRef) };
    number.to_f64()
}

fn string(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<String> {
    let value = dict_value(dict, key)?;
    if !value.instance_of::<CFString>() {
        return None;
    }
    let string =
        unsafe { CFString::wrap_under_get_rule(value.as_concrete_TypeRef() as CFStringRef) };
    Some(string.to_string())
}

fn window_bounds(dict: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<WindowBounds> {
    let value = dict_value(dict, key)?;
    let bounds: CFDictionary<CFString, CFType> = unsafe {
        CFDictionary::wrap_under_get_rule(value.as_concrete_TypeRef() as CFDictionaryRef)
    };
    let component = |name: &str| -> Option<f64> {
        let value = bounds.find(&CFString::new(name))?;
        if !value.instance_of::<CFNumber>() {
            return None;
        }
        let number =
            unsafe { CFNumber::wrap_under_get_rule(value.as_concrete_TypeRef() as CFNumberRef) };
        number.to_f64()
    };
    Some(WindowBounds {
        x: component("X")?,
        y: component("Y")?,
        width: component("Width")?,
        height: component("Height")?,
    })
}
