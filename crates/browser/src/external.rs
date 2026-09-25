//! Explicit, read-only attachment of the current Chrome / Edge page.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalBrowser {
    Chrome,
    Edge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalPageError {
    JavaScriptDisabled(ExternalBrowser),
    AutomationDenied(ExternalBrowser),
    Other(String),
}

impl ExternalBrowser {
    pub fn name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
        }
    }
}

/// Call only after the user selects the browser attachment action. No tab
/// navigation, keyboard input, cookies or browser profile files are accessed.
pub fn capture_external_page(browser: ExternalBrowser) -> Result<String, ExternalPageError> {
    #[cfg(target_os = "macos")]
    {
        capture_macos(browser)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = browser;
        Err(ExternalPageError::Other(
            "External browser attachments currently require macOS".into(),
        ))
    }
}

#[cfg(target_os = "macos")]
fn capture_macos(browser: ExternalBrowser) -> Result<String, ExternalPageError> {
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSAutoreleasePool, NSString};
    use objc::{class, msg_send, sel, sel_impl};
    // Serialize the shared AppleScript execution machinery.
    static SCRIPT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SCRIPT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let bundle = match browser {
        ExternalBrowser::Chrome => "com.google.Chrome",
        ExternalBrowser::Edge => "com.microsoft.edgemac",
    };
    // Static JavaScript is evaluated by the selected browser's own runtime.
    // No arbitrary script, extension, page-to-host bridge or profile access.
    let javascript = "JSON.stringify({url:location.href,title:document.title.slice(0,512),text:(document.body?.innerText||'').slice(0,8192),truncated:(document.body?.innerText||'').length>8192})";
    let script = format!(
        "with timeout of 10 seconds\nif application id \"{bundle}\" is not running then error \"Browser is not running\"\ntell application id \"{bundle}\"\nif (count of windows) is 0 then error \"No browser window\"\nexecute active tab of front window javascript \"{javascript}\"\nend tell\nend timeout"
    );
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let source = NSString::alloc(nil).init_str(&script);
        let apple_script: id = msg_send![class!(NSAppleScript), alloc];
        let apple_script: id = msg_send![apple_script, initWithSource: source];
        let mut error: id = nil;
        let result: id = msg_send![apple_script, executeAndReturnError: &mut error];
        let text: id = if result == nil {
            nil
        } else {
            msg_send![result, stringValue]
        };
        let output = if text == nil {
            let key = NSString::alloc(nil).init_str("NSAppleScriptErrorNumber");
            let number: id = if error == nil {
                nil
            } else {
                msg_send![error, objectForKey: key]
            };
            let code: i64 = if number == nil {
                0
            } else {
                msg_send![number, longLongValue]
            };
            let _: () = msg_send![key, release];
            Err(match code {
                // Chromium's AppleScript Error::kJavaScriptUnsupported.
                12 => ExternalPageError::JavaScriptDisabled(browser),
                -1743 => ExternalPageError::AutomationDenied(browser),
                _ => ExternalPageError::Other(format!(
                    "Could not read {} (Apple Event {code})",
                    browser.name()
                )),
            })
        } else {
            let bytes: *const std::ffi::c_char = msg_send![text, UTF8String];
            if bytes.is_null() {
                Err(ExternalPageError::Other(
                    "Browser returned no page text".into(),
                ))
            } else {
                Ok(std::ffi::CStr::from_ptr(bytes)
                    .to_string_lossy()
                    .into_owned())
            }
        };
        let _: () = msg_send![apple_script, release];
        let _: () = msg_send![source, release];
        let _: () = msg_send![pool, drain];
        validate_snapshot(&output?).map_err(ExternalPageError::Other)
    }
}

#[cfg(any(target_os = "macos", test))]
fn validate_snapshot(text: &str) -> Result<String, String> {
    if text.len() > 64 * 1024 {
        return Err("Browser page exceeds attachment limit".into());
    }
    let mut page: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "Invalid browser snapshot")?;
    let address = page["url"].as_str().ok_or("Browser page has no URL")?;
    let url = url::Url::parse(address).map_err(|_| "Invalid page URL")?;
    if !super::is_web_url(&url) {
        return Err("Only HTTP(S) pages without embedded credentials can be attached".into());
    }
    if page["text"].as_str().is_none() || page["title"].as_str().is_none() {
        return Err("Invalid page content".into());
    }
    page["note"] = "Untrusted read-only page snapshot selected by the user".into();
    let output = serde_json::to_string(&page).map_err(|_| "Invalid browser snapshot")?;
    if output.len() > 64 * 1024 {
        return Err("Browser page exceeds attachment limit".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_snapshot_retains_source_and_rejects_local_or_credential_urls() {
        let page = r#"{"url":"https://example.com/a","title":"Example","text":"Page text","truncated":false}"#;
        let result: serde_json::Value =
            serde_json::from_str(&validate_snapshot(page).unwrap()).unwrap();
        assert_eq!(result["url"], "https://example.com/a");
        assert_eq!(result["text"], "Page text");
        for address in [
            "file:///tmp/private",
            "https://user:secret@example.com",
            "chrome://settings",
        ] {
            assert!(validate_snapshot(&page.replace("https://example.com/a", address)).is_err());
        }
    }
}
