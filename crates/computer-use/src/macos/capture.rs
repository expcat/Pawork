//! ScreenCaptureKit window capture (CU-04).

use super::*;

// ---------- ScreenCaptureKit window capture (CU-04) ----------

/// Raw-pixel ceilings for one capture, mirroring the isolated desktop's
/// framebuffer limits: at most 4096 on one edge and 8M pixels overall.
pub(super) const MAX_CAPTURE_EDGE: f64 = 4096.0;
pub(super) const MAX_CAPTURE_PIXELS: f64 = 8_000_000.0;

/// Capture pixel scale for a window of the given points: up to 2x (Retina
/// text stays legible after the downscale to the image budget), bounded by
/// the raw-pixel ceilings. None for degenerate bounds.
pub(super) fn capture_scale(width_pt: f64, height_pt: f64) -> Option<f64> {
    if !width_pt.is_finite() || !height_pt.is_finite() || width_pt <= 0.0 || height_pt <= 0.0 {
        return None;
    }
    let by_edge = MAX_CAPTURE_EDGE / width_pt.max(height_pt);
    let by_pixels = (MAX_CAPTURE_PIXELS / (width_pt * height_pt)).sqrt();
    Some(2.0f64.min(by_edge).min(by_pixels))
}

/// Final integer output size for one capture. The scaled size is floored
/// per axis — independent rounding-up can push the pixel product past the
/// budget (2000x2001pt rounded to 2828x2829 = 8,000,412 > 8,000,000) —
/// and both ceilings are re-checked on the integer result. None when the
/// window has degenerate bounds.
pub(super) fn capture_pixel_size(width_pt: f64, height_pt: f64) -> Option<(usize, usize)> {
    let scale = capture_scale(width_pt, height_pt)?;
    let width = (width_pt * scale).floor().clamp(1.0, MAX_CAPTURE_EDGE) as usize;
    let height = (height_pt * scale).floor().clamp(1.0, MAX_CAPTURE_EDGE) as usize;
    if width.checked_mul(height)? > MAX_CAPTURE_PIXELS as usize {
        return None;
    }
    Some((width, height))
}

/// BGRA frame → tightly packed RGB for the JPEG encoder; stride-aware.
/// None when the buffer cannot contain the announced layout.
pub(super) fn bgra_to_rgb(
    bgra: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || stride < width * 4 {
        return None;
    }
    let needed = stride.checked_mul(height - 1)?.checked_add(width * 4)?;
    if bgra.len() < needed {
        return None;
    }
    let mut rgb = Vec::with_capacity(width * height * 3);
    for row in 0..height {
        let base = row * stride;
        for column in 0..width {
            let pixel = base + column * 4;
            rgb.extend_from_slice(&[bgra[pixel + 2], bgra[pixel + 1], bgra[pixel]]);
        }
    }
    Some(rgb)
}

/// Downscale to the image budget and encode as baseline JPEG, stepping the
/// quality down until the byte budget holds (same ladder as the isolated
/// desktop). An image that cannot fit is a capture failure, not a smaller
/// truth: no caller ever receives an out-of-budget frame.
pub(super) fn jpeg_within_budget(raw: &RgbImage) -> Result<(Vec<u8>, u32, u32), TargetError> {
    let scale = (MAX_IMAGE_EDGE as f64 / raw.width().max(raw.height()) as f64).min(1.0);
    let scaled = image::imageops::resize(
        raw,
        (raw.width() as f64 * scale).round().max(1.0) as u32,
        (raw.height() as f64 * scale).round().max(1.0) as u32,
        FilterType::Triangle,
    );
    for quality in [75, 50, 30] {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, quality)
            .encode_image(&scaled)
            .map_err(|_| TargetError::ProbeUnavailable("JPEG encoding failed".to_string()))?;
        if jpeg.len() <= MAX_IMAGE_BYTES {
            return Ok((jpeg, scaled.width(), scaled.height()));
        }
    }
    Err(TargetError::ProbeUnavailable(
        "screenshot exceeds JPEG byte budget".to_string(),
    ))
}

/// ScreenCaptureKit single-frame window capture, the route CU-01 measured
/// for occluded, background and minimized windows of every application
/// family (no main-thread requirement). The framework is dlopen-loaded
/// like HIServices above and driven through Objective-C message sends;
/// completion handlers are awaited with a bounded timeout.
pub(super) mod sck {
    use super::*;

    /// One captured frame: BGRA pixels with the image-reported row stride
    /// (may exceed width * 4).
    pub struct SckImage {
        pub width: usize,
        pub height: usize,
        pub stride: usize,
        pub bgra: Vec<u8>,
    }

    /// Why a capture failed. WindowGone maps to WindowReplaced (the window
    /// left the shareable set between the liveness proof and the capture);
    /// everything else keeps its OS cause for diagnosis.
    #[derive(Debug)]
    pub enum CaptureFailure {
        WindowGone,
        Unavailable(String),
    }

    impl From<CaptureFailure> for TargetError {
        fn from(failure: CaptureFailure) -> Self {
            match failure {
                CaptureFailure::WindowGone => TargetError::WindowReplaced,
                CaptureFailure::Unavailable(reason) => TargetError::ProbeUnavailable(reason),
            }
        }
    }

    /// Bound for one completion handler (shareable content or the capture
    /// itself); a single frame is normally sub-second.
    const COMPLETION_TIMEOUT: Duration = Duration::from_secs(10);

    /// Load the framework and warm the WindowServer connection once.
    /// SCScreenshotManager asserts on an uninitialized CGS connection, so a
    /// CoreGraphics call runs first (CU-01 probe finding).
    fn ensure_sck() -> Result<(), CaptureFailure> {
        static READY: OnceLock<Result<(), String>> = OnceLock::new();
        READY
            .get_or_init(|| unsafe {
                CGMainDisplayID();
                let path =
                    b"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit\x00";
                let handle = libc::dlopen(path.as_ptr() as *const c_char, libc::RTLD_LAZY);
                if handle.is_null() {
                    return Err("dlopen ScreenCaptureKit failed".to_string());
                }
                for name in [
                    "SCShareableContent",
                    "SCContentFilter",
                    "SCStreamConfiguration",
                    "SCScreenshotManager",
                ] {
                    if objc::runtime::Class::get(name).is_none() {
                        return Err(format!(
                            "ScreenCaptureKit class {name} unavailable (needs macOS 14+)"
                        ));
                    }
                }
                Ok(())
            })
            .clone()
            .map_err(CaptureFailure::Unavailable)
    }

    /// Shared state between a completion handler and the waiting caller,
    /// with explicit ownership of the retained result pointer. The
    /// handler retains a successful object and stringifies any error
    /// before its autoreleasepool drains. Exactly one side releases the
    /// retained pointer: the waiter after a successful wait, the handler
    /// itself when the wait was already abandoned (timeout), or the
    /// waiter when the result landed between the timeout and the
    /// abandonment lock. A late frame therefore can never leak, and a
    /// timed-out wait never leaves filter/config or images behind.
    pub struct Completion {
        state: Mutex<CompletionState>,
        ready: Condvar,
        dispose: unsafe fn(usize),
    }

    #[derive(Default)]
    struct CompletionState {
        result: usize,
        error: Option<String>,
        done: bool,
        abandoned: bool,
    }

    pub fn completion(dispose: unsafe fn(usize)) -> Arc<Completion> {
        Arc::new(Completion {
            state: Mutex::new(CompletionState::default()),
            ready: Condvar::new(),
            dispose,
        })
    }

    /// Handler side, invoked exactly once per block call: stores the
    /// (already retained) result for the waiter, or releases it
    /// immediately when the waiter has given up. A result that arrives
    /// together with an error is released here instead of being handed
    /// out, so an error delivery never carries an owned pointer and the
    /// waiter's error path never has anything to release.
    pub fn deliver(pair: &Arc<Completion>, result: usize, error: Option<String>) {
        let release_now = {
            let mut state = pair.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.abandoned {
                result != 0
            } else if error.is_some() {
                state.result = 0;
                state.error = error;
                state.done = true;
                result != 0
            } else {
                state.result = result;
                state.done = true;
                false
            }
        };
        pair.ready.notify_one();
        if release_now {
            unsafe { (pair.dispose)(result) }
        }
    }

    /// Waiter side: on timeout the wait is abandoned; a result that
    /// landed between the deadline and the abandonment lock is released
    /// here before the error is returned.
    pub fn wait_completion(
        pair: &Arc<Completion>,
        timeout: Duration,
    ) -> Result<(usize, Option<String>), CaptureFailure> {
        let guard = pair.state.lock().unwrap_or_else(|e| e.into_inner());
        let (mut guard, elapsed) = pair
            .ready
            .wait_timeout_while(guard, timeout, |state| !state.done)
            .map_err(|_| CaptureFailure::Unavailable("completion lock poisoned".to_string()))?;
        if elapsed.timed_out() {
            guard.abandoned = true;
            let late = std::mem::take(&mut guard.result);
            drop(guard);
            if late != 0 {
                unsafe { (pair.dispose)(late) }
            }
            return Err(CaptureFailure::Unavailable(format!(
                "completion handler timed out after {}s",
                timeout.as_secs()
            )));
        }
        Ok((guard.result, guard.error.clone()))
    }

    /// Release hooks for the two retain conventions a handler can hold.
    unsafe fn release_objc_object(pointer: usize) {
        let _: () = msg_send![pointer as *mut Object, release];
    }

    unsafe fn release_cf_object(pointer: usize) {
        CFRelease(pointer as *const c_void);
    }

    fn error_description(error: *mut Object) -> String {
        if error.is_null() {
            return "unknown nil-context error".to_string();
        }
        unsafe { rs_str(msg_send![error, localizedDescription]) }
    }

    /// The current shareable content: every window of every app, including
    /// off-screen, occluded and minimized ones (onScreenWindowsOnly: false).
    fn shareable_content() -> Result<*mut Object, CaptureFailure> {
        ensure_sck()?;
        unsafe {
            let class = objc::runtime::Class::get("SCShareableContent").unwrap();
            let pair = completion(release_objc_object);
            let handler = pair.clone();
            let block = ConcreteBlock::new(move |content: *mut Object, error: *mut Object| {
                if !content.is_null() {
                    let _: *mut Object = msg_send![content, retain];
                }
                let description = if error.is_null() {
                    None
                } else {
                    Some(error_description(error))
                };
                deliver(&handler, content as usize, description);
            });
            let block = block.copy();
            let modern: BOOL = msg_send![
                class,
                respondsToSelector: sel!(getShareableContentExcludingDesktopWindows:onScreenWindowsOnly:completionHandler:)
            ];
            if modern == YES {
                let _: () = msg_send![class,
                    getShareableContentExcludingDesktopWindows: false
                    onScreenWindowsOnly: false
                    completionHandler: &*block];
            } else {
                let _: () = msg_send![class, getShareableContentWithCompletionHandler: &*block];
            }
            let (content, error) = wait_completion(&pair, COMPLETION_TIMEOUT)?;
            // deliver guarantees an error never carries an owned pointer.
            if let Some(description) = error {
                return Err(CaptureFailure::Unavailable(format!(
                    "SCShareableContent: {description}"
                )));
            }
            if content == 0 {
                return Err(CaptureFailure::Unavailable(
                    "SCShareableContent returned nil".to_string(),
                ));
            }
            Ok(content as *mut Object)
        }
    }

    /// The one SCWindow naming (window_id, pid): identity comes from both
    /// the window-server id and the owning process, so a stale id another
    /// process reused can never be captured for the authorized target.
    fn find_window(content: *mut Object, pid: i32, window_id: u32) -> Option<*mut Object> {
        unsafe {
            let windows: *mut Object = msg_send![content, windows];
            if windows.is_null() {
                return None;
            }
            let count: usize = msg_send![windows, count];
            for i in 0..count {
                let window: *mut Object = msg_send![windows, objectAtIndex: i];
                if window.is_null() {
                    continue;
                }
                let id: u32 = msg_send![window, windowID];
                let app: *mut Object = msg_send![window, owningApplication];
                let owner: i32 = if app.is_null() {
                    0
                } else {
                    msg_send![app, processID]
                };
                if id == window_id && owner == pid {
                    let _: *mut Object = msg_send![window, retain];
                    return Some(window);
                }
            }
            None
        }
    }

    /// Layout ceilings re-checked against the actually delivered frame
    /// before any pixel is copied: the capture is only trusted within the
    /// raw-pixel budget, whatever size was configured.
    pub fn checked_frame_layout(
        width: usize,
        height: usize,
        stride: usize,
    ) -> Result<(), CaptureFailure> {
        if width == 0 || height == 0 || stride < width.saturating_mul(4) {
            return Err(CaptureFailure::Unavailable(format!(
                "captured image has degenerate layout ({width}x{height} stride {stride})"
            )));
        }
        if width > MAX_CAPTURE_EDGE as usize
            || height > MAX_CAPTURE_EDGE as usize
            || width.saturating_mul(height) > MAX_CAPTURE_PIXELS as usize
        {
            return Err(CaptureFailure::Unavailable(format!(
                "captured image exceeds the pixel budget ({width}x{height})"
            )));
        }
        Ok(())
    }

    /// Copy the frame's BGRA bytes out of the CGImage. A frame with no
    /// pixels or a truncated provider is a capture failure, never a
    /// partial image.
    fn extract_pixels(image: *const c_void) -> Result<SckImage, CaptureFailure> {
        unsafe {
            let width = CGImageGetWidth(image);
            let height = CGImageGetHeight(image);
            let stride = CGImageGetBytesPerRow(image);
            checked_frame_layout(width, height, stride)?;
            let provider = CGImageGetDataProvider(image);
            if provider.is_null() {
                return Err(CaptureFailure::Unavailable(
                    "captured image has no data provider".to_string(),
                ));
            }
            let data = CGDataProviderCopyData(provider);
            if data.is_null() {
                return Err(CaptureFailure::Unavailable(
                    "captured image bytes unavailable".to_string(),
                ));
            }
            let length = CFDataGetLength(data);
            let needed = stride * (height - 1) + width * 4;
            let bytes = if length < 0 || (length as usize) < needed {
                None
            } else {
                let pointer = CFDataGetBytePtr(data);
                if pointer.is_null() {
                    None
                } else {
                    Some(std::slice::from_raw_parts(pointer, length as usize).to_vec())
                }
            };
            CFRelease(data);
            let bgra = bytes.ok_or_else(|| {
                CaptureFailure::Unavailable(format!(
                    "captured image truncated ({length} bytes for {width}x{height} stride {stride})"
                ))
            })?;
            Ok(SckImage {
                width,
                height,
                stride,
                bgra,
            })
        }
    }

    /// Capture one complete frame of the window. Errors never carry a
    /// partial image: the frame is either fully delivered or absent.
    pub fn capture(
        pid: i32,
        window_id: u32,
        width_pt: f64,
        height_pt: f64,
    ) -> Result<SckImage, CaptureFailure> {
        let (width_px, height_px) = capture_pixel_size(width_pt, height_pt).ok_or_else(|| {
            CaptureFailure::Unavailable("window has degenerate bounds".to_string())
        })?;
        autoreleasepool(|| unsafe {
            let content = shareable_content()?;
            let window = find_window(content, pid, window_id);
            let _: () = msg_send![content, release];
            let Some(window) = window else {
                return Err(CaptureFailure::WindowGone);
            };
            let filter: *mut Object = msg_send![class!(SCContentFilter), alloc];
            let filter: *mut Object = msg_send![filter, initWithDesktopIndependentWindow: window];
            let _: () = msg_send![window, release];
            if filter.is_null() {
                return Err(CaptureFailure::Unavailable(
                    "SCContentFilter init returned nil".to_string(),
                ));
            }
            let config: *mut Object = msg_send![class!(SCStreamConfiguration), alloc];
            let config: *mut Object = msg_send![config, init];
            let _: () = msg_send![config, setWidth: width_px];
            let _: () = msg_send![config, setHeight: height_px];
            // The window content must fill the whole configured frame:
            // without scalesToFit SCK draws it at its native size into the
            // top-left corner and leaves the rest black, which silently
            // breaks image->window coordinate mapping.
            let _: () = msg_send![config, setScalesToFit: true];
            let _: () = msg_send![config, setShowsCursor: false];
            // 'BGRA' little-endian fourcc.
            let _: () = msg_send![config, setPixelFormat: 0x4247_5241u32];
            let has_resolution: BOOL =
                msg_send![config, respondsToSelector: sel!(setCaptureResolution:)];
            if has_resolution == YES {
                // SCCaptureResolutionBest: honor the explicit pixel size.
                let _: () = msg_send![config, setCaptureResolution: 1i64];
            }
            let pair = completion(release_cf_object);
            let handler = pair.clone();
            let block = ConcreteBlock::new(move |image: *mut c_void, error: *mut Object| {
                let description = if error.is_null() {
                    None
                } else {
                    Some(error_description(error))
                };
                if !image.is_null() {
                    CFRetain(image);
                }
                deliver(&handler, image as usize, description);
            });
            let block = block.copy();
            let _: () = msg_send![class!(SCScreenshotManager),
                captureImageWithFilter: filter
                configuration: config
                completionHandler: &*block];
            // Filter and config are released on every path, including a
            // timed-out wait.
            let waited = wait_completion(&pair, COMPLETION_TIMEOUT);
            let _: () = msg_send![filter, release];
            let _: () = msg_send![config, release];
            let (image, error) = waited?;
            // deliver guarantees an error never carries an owned pointer.
            if let Some(description) = error {
                return Err(CaptureFailure::Unavailable(format!(
                    "capture: {description}"
                )));
            }
            if image == 0 {
                return Err(CaptureFailure::Unavailable(
                    "capture returned a nil image".to_string(),
                ));
            }
            let image = image as *const c_void;
            let extracted = extract_pixels(image);
            CFRelease(image);
            extracted
        })
    }
}
